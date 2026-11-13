//! Scheduling for the polling loops in `wait` and `watch`. Each inbox is
//! listed every `interval`; when its provider has a push stream (mail.tm,
//! DropMail), the stream runs on its own thread and wakes the loop as soon as
//! mail arrives, and polling slows to a safety net while it is connected.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::provider::{Inbox, Provider, PushEvent};

/// How often an inbox with a live push stream is still listed, in case the
/// stream misses something.
const SAFETY_POLL: Duration = Duration::from_secs(60);
/// Longest wait before reconnecting a stream that keeps failing.
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// A stream that stayed up this long counts as healthy when it closes.
const STABLE: Duration = Duration::from_secs(60);
/// A healthy stream that closes is reconnected after the first backoff step
/// (4 s), and its inbox is still reported live for this long, so a stream
/// the hub (or tmpmail, for a fresh token) ends doesn't make "live" blink.
const RECONNECT_GRACE: Duration = Duration::from_secs(30);

pub struct Poller {
    interval: Duration,
    push: bool,
    /// Next time each inbox (by address) should be listed.
    due: HashMap<String, Instant>,
    /// Inboxes a stream announced mail for since they were last listed.
    woken: HashSet<String>,
    streams: HashMap<String, Stream>,
    /// Inboxes whose provider rate-limited them, and until when they are
    /// left alone, push wake-ups included.
    held: HashMap<String, Instant>,
    tx: Sender<Signal>,
    rx: Receiver<Signal>,
    next_id: u64,
}

struct Stream {
    /// Tells signals from this stream apart from those of an earlier one.
    id: u64,
    running: bool,
    live: bool,
    /// Until when a healthy stream that closed still counts as live while
    /// it reconnects.
    grace_until: Option<Instant>,
    started: Instant,
    failures: u32,
    retry_at: Instant,
    /// The provider has no push for this inbox; don't ask again.
    unsupported: bool,
    stop: Arc<AtomicBool>,
}

struct Signal {
    address: String,
    id: u64,
    kind: Kind,
}

enum Kind {
    Connected,
    Mail,
    Closed,
}

impl Poller {
    pub fn new(interval: Duration, push: bool) -> Self {
        let (tx, rx) = channel();
        Poller {
            interval,
            push,
            due: HashMap::new(),
            woken: HashSet::new(),
            streams: HashMap::new(),
            held: HashMap::new(),
            tx,
            rx,
            next_id: 0,
        }
    }

    /// Whether `address` should be listed now.
    pub fn is_due(&self, address: &str) -> bool {
        let now = Instant::now();
        if self.held.get(address).is_some_and(|t| now < *t) {
            return false;
        }
        self.woken.contains(address) || self.due.get(address).is_none_or(|t| now >= *t)
    }

    /// [`Poller::polled`] for a listing's result: a rate limit also holds
    /// the inbox back for [`rate_limit_pause`] (`tries` refusals in a row
    /// before this one), push wake-ups included.
    pub fn polled_result<T>(
        &mut self,
        address: &str,
        result: &crate::error::Result<T>,
        tries: u32,
    ) {
        self.polled(address, result.is_ok());
        match result {
            Err(e) => {
                if let Some(pause) = rate_limit_pause(e, tries, self.interval) {
                    self.hold(address, pause);
                }
            }
            Ok(_) => {
                self.held.remove(address);
            }
        }
    }

    /// Lists `address` no sooner than `wait` from now, push wake-ups
    /// included (its provider asked to be left alone for a while).
    pub fn hold(&mut self, address: &str, wait: Duration) {
        let at = Instant::now() + wait;
        let held = self.held.entry(address.to_string()).or_insert(at);
        *held = (*held).max(at);
        let due = self.due.entry(address.to_string()).or_insert(at);
        *due = (*due).max(at);
    }

    /// Records that `address` was just listed (or failed to be) and when to
    /// list it next. After a push wake-up it is listed once more soon after,
    /// in case the provider announced the mail before it could be listed.
    pub fn polled(&mut self, address: &str, ok: bool) {
        let woken = self.woken.remove(address);
        if !ok {
            // Not live while the provider fails, reconnecting or not.
            if let Some(s) = self.streams.get_mut(address) {
                s.grace_until = None;
            }
        }
        let wait = if ok && !woken && self.is_connected(address) {
            SAFETY_POLL
        } else {
            self.interval + Duration::from_millis(rand::random_range(0..1000))
        };
        self.due.insert(address.to_string(), Instant::now() + wait);
    }

    /// Whether `address`'s push stream is up, or reconnecting after a
    /// healthy one closed (for a short while).
    pub fn is_live(&self, address: &str) -> bool {
        self.streams
            .get(address)
            .is_some_and(|s| s.live || s.grace_until.is_some_and(|t| Instant::now() < t))
    }

    /// Whether `address`'s push stream is connected right now.
    fn is_connected(&self, address: &str) -> bool {
        self.streams.get(address).is_some_and(|s| s.live)
    }

    /// Starts a push stream for `inbox` unless one is running, is backing
    /// off after failures, or the provider has none. Called after each
    /// successful listing, so a stream that died is restarted with the
    /// inbox's current credentials.
    pub fn subscribe(&mut self, p: &dyn Provider, inbox: &Inbox) {
        if !self.push {
            return;
        }
        let now = Instant::now();
        let (failures, grace_until) = match self.streams.get(&inbox.address) {
            Some(s) if s.unsupported || s.running || now < s.retry_at => return,
            Some(s) => (s.failures, s.grace_until),
            None => (0, None),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let Some(stream) = p.push(inbox) else {
            self.streams.insert(
                inbox.address.clone(),
                Stream {
                    id: 0,
                    running: false,
                    live: false,
                    grace_until: None,
                    started: now,
                    failures,
                    retry_at: now,
                    unsupported: true,
                    stop,
                },
            );
            return;
        };
        self.next_id += 1;
        let id = self.next_id;
        let (tx, address, flag) = (self.tx.clone(), inbox.address.clone(), stop.clone());
        std::thread::spawn(move || {
            let send = |kind| {
                let _ = tx.send(Signal {
                    address: address.clone(),
                    id,
                    kind,
                });
            };
            // Errors just close the stream; the poller decides when to retry.
            let _ = stream.run(&mut |event| {
                match event {
                    PushEvent::Connected => send(Kind::Connected),
                    PushEvent::Mail => send(Kind::Mail),
                    PushEvent::Alive => {}
                }
                !flag.load(Ordering::Relaxed)
            });
            send(Kind::Closed);
        });
        self.streams.insert(
            inbox.address.clone(),
            Stream {
                id,
                running: true,
                live: false,
                grace_until,
                started: now,
                failures,
                retry_at: now,
                unsupported: false,
                stop,
            },
        );
    }

    /// Makes `address` due now, keeping its push stream.
    pub fn poll_now(&mut self, address: &str) {
        self.due.remove(address);
    }

    /// Stops following `address` (it was dropped or rotated away).
    pub fn forget(&mut self, address: &str) {
        self.due.remove(address);
        self.held.remove(address);
        self.woken.remove(address);
        if let Some(s) = self.streams.remove(address) {
            // The thread notices at its next event and exits.
            s.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Sleeps until the next inbox is due, a stream announces mail, or
    /// `deadline` passes, whichever comes first.
    pub fn sleep(&mut self, deadline: Option<Instant>) {
        let now = Instant::now();
        let mut until = self
            .due
            .values()
            .min()
            .copied()
            .unwrap_or(now + self.interval);
        if let Some(d) = deadline {
            until = until.min(d);
        }
        // A wake-up for an inbox that is held back waits for the hold.
        if self
            .woken
            .iter()
            .any(|a| self.held.get(a).is_none_or(|t| now >= *t))
        {
            until = now;
        }
        // `self.tx` keeps the channel open, so this only ever times out.
        let first = self.rx.recv_timeout(until.saturating_duration_since(now));
        let signals: Vec<Signal> = first.into_iter().chain(self.rx.try_iter()).collect();
        for signal in signals {
            self.handle(signal);
        }
    }

    fn handle(&mut self, signal: Signal) {
        let Some(s) = self
            .streams
            .get_mut(&signal.address)
            .filter(|s| s.id == signal.id)
        else {
            return; // from a stream that was replaced or forgotten
        };
        let now = Instant::now();
        match signal.kind {
            Kind::Connected => {
                // A grace still running stays: it covers this stream
                // failing quickly too, and runs out on its own.
                s.live = true;
                s.started = now;
            }
            Kind::Mail => {
                self.woken.insert(signal.address);
            }
            Kind::Closed => {
                let stable = s.live && now.duration_since(s.started) >= STABLE;
                s.failures = if stable { 1 } else { s.failures + 1 };
                s.running = false;
                s.live = false;
                s.retry_at = now + backoff(s.failures);
                // A healthy stream is reconnected at the next listing, as
                // soon as the backoff allows, and stays "live" meanwhile
                // (through quick failures too, until the grace runs out).
                // Otherwise: back to normal polling until one is up again.
                if stable {
                    s.grace_until = Some(now + RECONNECT_GRACE);
                }
                let soon = if stable {
                    s.retry_at
                } else {
                    now + self.interval
                };
                self.due
                    .entry(signal.address)
                    .and_modify(|t| *t = (*t).min(soon));
            }
        }
    }
}

impl Drop for Poller {
    /// Streams don't end with their poller on their own, so a long-running
    /// `tmpmail mcp` would keep one connection open per finished wait.
    fn drop(&mut self) {
        for s in self.streams.values() {
            s.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// 4 s, 8 s, 16 s, ... up to five minutes.
/// How long to leave a provider that rate-limited an inbox alone, after
/// `tries` refusals in a row before this one: its `Retry-After` if it gave
/// one, else the poll interval doubled for each earlier refusal; at most
/// [`crate::http::MAX_RETRY_AFTER`], never less than the interval. `None`
/// if `e` isn't a rate limit.
pub(crate) fn rate_limit_pause(
    e: &crate::error::Error,
    tries: u32,
    interval: Duration,
) -> Option<Duration> {
    let crate::error::Error::RateLimited { provider } = e else {
        return None;
    };
    let interval = interval.max(Duration::from_secs(1));
    let pause = crate::http::retry_after(provider)
        .unwrap_or_else(|| interval.saturating_mul(1 << tries.min(8)));
    Some(pause.clamp(interval, crate::http::MAX_RETRY_AFTER.max(interval)))
}

fn backoff(failures: u32) -> Duration {
    Duration::from_secs(2u64 << failures.min(8)).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use crate::provider::{Capabilities, CreateOpts, Message, PushStream, Summary};

    /// A provider whose push stream announces `mails` mails and closes.
    struct Fake {
        mails: usize,
    }

    struct FakeStream {
        mails: usize,
    }

    impl PushStream for FakeStream {
        fn run(self: Box<Self>, event: &mut dyn FnMut(PushEvent) -> bool) -> Result<()> {
            event(PushEvent::Connected);
            for _ in 0..self.mails {
                event(PushEvent::Mail);
            }
            Ok(())
        }
    }

    impl Provider for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn website(&self) -> &'static str {
            ""
        }
        fn caps(&self) -> Capabilities {
            unimplemented!()
        }
        fn domains(&self) -> Result<Vec<String>> {
            unimplemented!()
        }
        fn create(&self, _: &CreateOpts) -> Result<Inbox> {
            unimplemented!()
        }
        fn list(&self, _: &mut Inbox) -> Result<Vec<Summary>> {
            unimplemented!()
        }
        fn get(&self, _: &mut Inbox, _: &str) -> Result<Message> {
            unimplemented!()
        }
        fn push(&self, _: &Inbox) -> Option<Box<dyn PushStream>> {
            Some(Box::new(FakeStream { mails: self.mails }))
        }
    }

    fn inbox() -> Inbox {
        Inbox::new("fake", "a@x.com".into(), serde_json::Value::Null)
    }

    /// Handles signals until the stream for a@x.com has closed.
    fn drain(poller: &mut Poller) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while poller.streams["a@x.com"].running && Instant::now() < deadline {
            poller.sleep(Some(Instant::now() + Duration::from_millis(50)));
        }
    }

    #[test]
    fn push_wakes_the_inbox_and_failures_back_off() {
        let mut poller = Poller::new(Duration::from_secs(5), true);
        let inbox = inbox();
        poller.polled(&inbox.address, true);
        assert!(!poller.is_due(&inbox.address));

        poller.subscribe(&Fake { mails: 1 }, &inbox);
        drain(&mut poller);
        assert!(poller.is_due(&inbox.address), "mail wakes the inbox");

        // The stream closed right after connecting: retry later, not now.
        let s = &poller.streams[&inbox.address];
        assert!(!s.live);
        assert_eq!(s.failures, 1);
        let retry_at = s.retry_at;
        assert!(retry_at > Instant::now());
        poller.subscribe(&Fake { mails: 0 }, &inbox);
        assert_eq!(poller.streams[&inbox.address].retry_at, retry_at);
    }

    /// Puts a running stream with id 7 in place for a@x.com.
    fn running(poller: &mut Poller) {
        let now = Instant::now();
        poller.streams.insert(
            "a@x.com".into(),
            Stream {
                id: 7,
                running: true,
                live: false,
                grace_until: None,
                started: now,
                failures: 0,
                retry_at: now,
                unsupported: false,
                stop: Arc::new(AtomicBool::new(false)),
            },
        );
    }

    fn signal(poller: &mut Poller, kind: Kind) {
        poller.handle(Signal {
            address: "a@x.com".into(),
            id: 7,
            kind,
        });
    }

    #[test]
    fn a_healthy_stream_that_closes_stays_live_while_it_reconnects() {
        let mut poller = Poller::new(Duration::from_secs(5), true);
        poller.polled("a@x.com", true);
        running(&mut poller);
        signal(&mut poller, Kind::Connected);
        assert!(poller.is_live("a@x.com"));
        poller.streams.get_mut("a@x.com").unwrap().started = Instant::now() - STABLE;
        signal(&mut poller, Kind::Closed);

        let s = &poller.streams["a@x.com"];
        assert!(!s.live && !s.running);
        assert!(poller.is_live("a@x.com"), "still live during the grace");
        assert!(!poller.is_connected("a@x.com"));
        // Listed (and so reconnected) when the backoff ends, not later.
        assert_eq!(poller.due["a@x.com"], s.retry_at);
        assert!(s.retry_at <= Instant::now() + backoff(1));

        // A quick failure of the reconnect keeps the grace.
        let retry_at = Instant::now();
        let s = poller.streams.get_mut("a@x.com").unwrap();
        (s.running, s.retry_at) = (true, retry_at);
        signal(&mut poller, Kind::Connected);
        signal(&mut poller, Kind::Closed);
        assert!(poller.is_live("a@x.com"));

        // The grace runs out if no stream connects.
        poller.streams.get_mut("a@x.com").unwrap().grace_until = Some(Instant::now());
        assert!(!poller.is_live("a@x.com"));
    }

    #[test]
    fn a_failed_listing_ends_the_grace() {
        let mut poller = Poller::new(Duration::from_secs(5), true);
        running(&mut poller);
        signal(&mut poller, Kind::Connected);
        poller.streams.get_mut("a@x.com").unwrap().started = Instant::now() - STABLE;
        signal(&mut poller, Kind::Closed);
        assert!(poller.is_live("a@x.com"));
        poller.polled("a@x.com", false);
        assert!(!poller.is_live("a@x.com"));
    }

    #[test]
    fn a_stream_that_fails_quickly_is_not_live() {
        let mut poller = Poller::new(Duration::from_secs(5), true);
        running(&mut poller);
        signal(&mut poller, Kind::Connected);
        signal(&mut poller, Kind::Closed);
        assert!(!poller.is_live("a@x.com"));
        assert_eq!(poller.streams["a@x.com"].grace_until, None);
    }

    #[test]
    fn disabled_push_starts_nothing() {
        let mut poller = Poller::new(Duration::from_secs(5), false);
        poller.subscribe(&Fake { mails: 1 }, &inbox());
        assert!(poller.streams.is_empty());
    }

    #[test]
    fn a_rate_limited_inbox_is_held_back_even_from_push() {
        // Review of retest F6: a push wake-up listed a rate-limited inbox
        // at once.
        let mut p = Poller::new(Duration::from_secs(5), true);
        let limited: crate::error::Result<()> = Err(crate::error::Error::RateLimited {
            provider: "nobody".into(),
        });
        p.polled_result("a@x.com", &limited, 0);
        p.woken.insert("a@x.com".into());
        assert!(!p.is_due("a@x.com"));
        // The loop doesn't spin on that wake-up meanwhile.
        p.held.insert(
            "a@x.com".into(),
            Instant::now() + Duration::from_millis(300),
        );
        let t = Instant::now();
        p.sleep(Some(Instant::now() + Duration::from_millis(200)));
        assert!(t.elapsed() >= Duration::from_millis(150));
        // Past the hold, the wake-up counts again; a listing clears it.
        p.held.insert("a@x.com".into(), Instant::now());
        assert!(p.is_due("a@x.com"));
        p.polled_result("a@x.com", &Ok(()), 0);
        assert!(p.held.is_empty());
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(4));
        assert_eq!(backoff(2), Duration::from_secs(8));
        assert_eq!(backoff(30), MAX_BACKOFF);
    }
}
