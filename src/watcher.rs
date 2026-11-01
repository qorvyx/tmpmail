//! The background loop shared by the TUI and the desktop app: their worker
//! thread owns an [`App`] and a [`Poller`], carries out the front end's
//! requests, re-reads the state file so changes made by another tmpmail show
//! up, and lists every stored inbox on the same schedule as `watch` (push
//! where the provider has it). What a front end asks for and how it is told
//! about results is up to its [`Front`].

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::app::{App, Global};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::provider::{CreateOpts, Summary};
use crate::push::Poller;

/// How often the state file is re-read for inboxes another tmpmail added
/// or dropped.
const STATE_RELOAD: Duration = Duration::from_secs(5);
/// The longest nap between turns, so requests are picked up quickly.
const NAP: Duration = Duration::from_millis(100);

/// One front end's side of the loop.
pub trait Front {
    type Req;

    /// Whether `req` opens a message. Of several queued opens only the last
    /// is carried out: the user moved on.
    fn is_open(req: &Self::Req) -> bool;

    /// Carries out one request. Returns whether the inbox list changed.
    fn handle(&mut self, w: &mut Watcher, req: Self::Req) -> bool;

    /// The inbox list changed (or was asked for). Returns false once the
    /// front end is gone.
    fn inboxes(&mut self, app: &App) -> bool;

    /// An inbox was listed. `live` is whether its push stream is connected.
    /// Returns false once the front end is gone.
    fn listed(
        &mut self,
        address: String,
        result: std::result::Result<Vec<Summary>, String>,
        live: bool,
    ) -> bool;

    /// An inbox has expired and is no longer listed. Called once each time
    /// an inbox expires. Returns false once the front end is gone.
    fn expired(&mut self, _address: &str) -> bool {
        true
    }

    /// Called once per turn, after the requests.
    fn turn(&mut self) {}
}

/// The create option a [`Watcher::refused`] refusal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CreateField {
    /// The label.
    Label,
    /// The username (the part before `@`).
    User,
}

pub struct Watcher {
    pub app: App,
    pub poller: Poller,
    reloaded: Instant,
    /// Expired inboxes the front end was told about.
    expired: HashSet<String>,
}

impl Watcher {
    /// Loads the config and state, as `App::new` does.
    pub fn new(global: Global) -> Result<Self> {
        Ok(Self::with_app(App::new(global)?))
    }

    pub fn with_app(app: App) -> Self {
        let poller = Self::poller(&app.config);
        Watcher {
            app,
            poller,
            reloaded: Instant::now(),
            expired: HashSet::new(),
        }
    }

    fn poller(config: &Config) -> Poller {
        Poller::new(
            Duration::from_secs(config.poll_interval(None)),
            config.push(),
        )
    }

    /// Uses a new config (the desktop app's settings) from now on: the poll
    /// interval, push and the request timeout. When one of these changed,
    /// the old poller's push streams end (and their back-off with them) and
    /// every inbox is listed again on the next turn; otherwise polling goes
    /// on as it was.
    /// Only unknown keys the last config didn't have are reported, so
    /// saving the settings doesn't repeat warnings already shown.
    pub fn set_config(&mut self, config: Config) {
        crate::http::set_timeout(config.http_timeout);
        for w in config.new_warnings(&self.app.config) {
            self.app.note(&w);
        }
        let old = &self.app.config;
        if config.poll_interval(None) != old.poll_interval(None)
            || config.push() != old.push()
            || config.http_timeout != old.http_timeout
        {
            self.poller = Self::poller(&config);
        }
        self.app.config = config;
    }

    /// Runs until the front end is gone or `rx` is closed.
    pub fn run<F: Front>(&mut self, rx: &Receiver<F::Req>, front: &mut F) {
        if !front.inboxes(&self.app) {
            return;
        }
        loop {
            let mut reqs = Vec::new();
            loop {
                match rx.try_recv() {
                    Ok(req) => reqs.push(req),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            let last = reqs.iter().rposition(F::is_open);
            let reqs = reqs
                .into_iter()
                .enumerate()
                .filter(|(i, r)| !F::is_open(r) || Some(*i) == last)
                .map(|(_, r)| r);
            for req in reqs {
                if front.handle(self, req) {
                    front.inboxes(&self.app);
                }
            }
            front.turn();

            if self.reloaded.elapsed() >= STATE_RELOAD {
                self.reloaded = Instant::now();
                if self.reload() && !front.inboxes(&self.app) {
                    return;
                }
            }

            if !self.poll_due(front) {
                return;
            }
            self.poller.sleep(Some(Instant::now() + NAP));
        }
    }

    /// Re-reads the state file, and stops following inboxes that are gone
    /// from it. Returns whether the inbox list (addresses, labels or the
    /// active inbox) changed.
    fn reload(&mut self) -> bool {
        let Ok(state) = self.app.store.load() else {
            return false;
        };
        let addresses = |s: &crate::store::State| -> Vec<(String, Option<String>)> {
            s.inboxes
                .iter()
                .map(|i| (i.address.clone(), i.label.clone()))
                .collect()
        };
        let changed = addresses(&state) != addresses(&self.app.state)
            || state.active != self.app.state.active;
        for old in &self.app.state.inboxes {
            match state.position(&old.address) {
                None => {
                    self.poller.forget(&old.address);
                    self.expired.remove(&old.address);
                }
                // Another tmpmail marked mail read: list it again so the
                // front end shows that now, not at the next check.
                Some(k) if state.inboxes[k].read != old.read => {
                    self.poller.poll_now(&old.address);
                }
                Some(_) => {}
            }
        }
        self.app.state = state;
        changed
    }

    /// Lists every inbox that is due. Returns false once the front end is
    /// gone.
    fn poll_due<F: Front>(&mut self, front: &mut F) -> bool {
        for n in 0..self.app.state.inboxes.len() {
            let address = self.app.state.inboxes[n].address.clone();
            if self.app.state.inboxes[n].is_expired() {
                if self.expired.insert(address.clone()) {
                    self.poller.forget(&address); // ends its push stream
                    if !front.expired(&address) {
                        return false;
                    }
                }
                continue;
            }
            // Renewed (another tmpmail refreshed it): report a new expiry.
            self.expired.remove(&address);
            if !self.poller.is_due(&address) {
                continue;
            }
            let poller = &mut self.poller;
            let result = self.app.with_inbox_at(n, |p, inbox| {
                let list = App::sorted_list(p, inbox)?;
                poller.subscribe(p, inbox);
                Ok(list)
            });
            self.poller.polled(&address, result.is_ok());
            let live = self.poller.is_live(&address);
            if !front.listed(address, result.map_err(|e| e.to_string()), live) {
                return false;
            }
        }
        true
    }

    /// Lists `address` on the next turn, keeping its push stream.
    pub fn poll_now(&mut self, address: &str) {
        self.poller.poll_now(address);
    }

    /// Creates an inbox at `provider` (or the first that works) and saves
    /// it as the active one. Returns its address, or what went wrong.
    pub fn create(&mut self, provider: Option<&str>) -> std::result::Result<String, String> {
        self.create_with(provider, &CreateOpts::default(), None)
    }

    /// Why the stored inboxes refuse a new inbox with these options, and
    /// which option it is about, so a form can show it by that field: a
    /// label another inbox has (or that is its address or local part), or
    /// a username that is another inbox's label. [`Watcher::create_with`]
    /// refuses them the same way, before any request. Checked against the
    /// state file: another tmpmail may have added a label since this one
    /// last read it.
    pub fn refused(&self, opts: &CreateOpts, label: Option<&str>) -> Option<(CreateField, String)> {
        let fresh = self.app.store.load().ok();
        let state = fresh.as_ref().unwrap_or(&self.app.state);
        if let Some(Err(e)) = label.map(|l| state.check_label(l, None)) {
            return Some((CreateField::Label, e.to_string()));
        }
        let user = opts.user.as_deref()?;
        let address = opts.domain.as_ref().map(|d| format!("{user}@{d}"));
        state
            .check_user(user, address.as_deref())
            .err()
            .map(|e| (CreateField::User, e.to_string()))
    }

    /// [`Watcher::create`] with options (a username, a domain) and a label.
    pub fn create_with(
        &mut self,
        provider: Option<&str>,
        opts: &CreateOpts,
        label: Option<String>,
    ) -> std::result::Result<String, String> {
        if let Some((_, why)) = self.refused(opts, label.as_deref()) {
            return Err(why);
        }
        let mut inbox = self
            .app
            .create_with_fallback(provider, None, opts)
            .map_err(|e| format!("could not create an inbox: {e}"))?;
        inbox.label = label;
        let _ = self.app.note_label_clash(&inbox);
        let address = inbox.address.clone();
        let (state, ()) = self
            .app
            .store
            .update(|s| s.add(inbox))
            .map_err(|e| format!("could not save the new inbox: {e}"))?;
        self.app.state = state;
        Ok(address)
    }

    /// Makes `address` the active inbox (the CLI's default).
    pub fn make_active(&mut self, address: &str) -> Result<()> {
        let (state, ()) = self
            .app
            .store
            .update(|s| s.active = Some(address.to_string()))?;
        self.app.state = state;
        Ok(())
    }

    /// Deletes a message at the provider, then lists the inbox again.
    /// `None` when `address` isn't stored.
    pub fn delete_message(&mut self, address: &str, id: &str) -> Option<Result<()>> {
        let n = self.app.state.position(address)?;
        let result = self
            .app
            .with_inbox_at(n, |p, inbox| p.delete_message(inbox, id));
        if result.is_ok() {
            self.poller.forget(address);
        }
        Some(result)
    }

    /// Deletes an inbox at the provider where it can be, and forgets it
    /// locally either way. Problems go to `warn` with what was being done.
    /// Returns false when `address` isn't stored.
    pub fn drop_inbox(&mut self, address: &str, mut warn: impl FnMut(&str, Error)) -> bool {
        let Some(n) = self.app.state.position(address) else {
            return false;
        };
        let mut inbox = self.app.state.inboxes[n].clone();
        if let Ok(p) = crate::provider::get(&inbox.provider) {
            if let Err(e) = p.delete_inbox(&mut inbox) {
                warn(
                    "could not delete it at the provider, forgetting it locally",
                    e,
                );
            }
        }
        if let Err(e) = self.app.forget(address) {
            warn("could not forget the inbox", e);
        }
        self.poller.forget(address);
        self.expired.remove(address);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Capabilities, Inbox, Message, Provider};
    use std::sync::mpsc::{Sender, channel};

    /// Lists one message per inbox and counts the listings.
    struct Fake {
        lists: Sender<String>,
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
        fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
            let _ = self.lists.send(inbox.address.clone());
            Ok(vec![Summary {
                id: "1".into(),
                subject: "hi".into(),
                ..Default::default()
            }])
        }
        fn get(&self, _: &mut Inbox, id: &str) -> Result<Message> {
            Ok(Message {
                id: id.into(),
                ..Default::default()
            })
        }
    }

    #[derive(Debug, PartialEq)]
    enum Ev {
        Inboxes(usize),
        Listed(String, usize),
        Expired(String),
        Opened(String),
    }

    enum Req {
        Open(String),
        Poke(String),
    }

    struct Test {
        tx: Sender<Ev>,
    }

    impl Front for Test {
        type Req = Req;
        fn is_open(req: &Req) -> bool {
            matches!(req, Req::Open(_))
        }
        fn handle(&mut self, w: &mut Watcher, req: Req) -> bool {
            match req {
                Req::Open(id) => {
                    let _ = self.tx.send(Ev::Opened(id));
                }
                Req::Poke(address) => w.poll_now(&address),
            }
            false
        }
        fn inboxes(&mut self, app: &App) -> bool {
            self.tx.send(Ev::Inboxes(app.state.inboxes.len())).is_ok()
        }
        fn listed(
            &mut self,
            address: String,
            result: std::result::Result<Vec<Summary>, String>,
            _live: bool,
        ) -> bool {
            let n = result.map_or(0, |l| l.len());
            self.tx.send(Ev::Listed(address, n)).is_ok()
        }
        fn expired(&mut self, address: &str) -> bool {
            self.tx.send(Ev::Expired(address.into())).is_ok()
        }
    }

    fn next(rx: &std::sync::mpsc::Receiver<Ev>) -> Ev {
        rx.recv_timeout(Duration::from_secs(5)).expect("an event")
    }

    #[test]
    fn a_taken_label_or_name_is_refused_by_field_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let w = Watcher::with_app(App::for_test(dir.path().to_path_buf()));
        // Another tmpmail adds a labelled inbox after this one loaded.
        let mut gm = Inbox::new(
            "guerrilla",
            "tvidfnke@sharklasers.com".into(),
            serde_json::Value::Null,
        );
        gm.label = Some("gm1".into());
        w.app.store.update(|s| s.add(gm)).unwrap();
        let opts = |user: Option<&str>| CreateOpts {
            user: user.map(Into::into),
            ..Default::default()
        };
        let (field, why) = w.refused(&opts(None), Some("GM1")).unwrap();
        assert_eq!(field, CreateField::Label);
        assert!(why.contains("tvidfnke@sharklasers.com"), "{why}");
        let (field, why) = w.refused(&opts(None), Some("tvidfnke")).unwrap();
        assert_eq!(field, CreateField::Label);
        assert!(why.contains("would also match"), "{why}");
        let (field, why) = w.refused(&opts(Some("gm1")), Some("shop")).unwrap();
        assert_eq!(field, CreateField::User);
        assert!(
            why.starts_with("the name \"gm1\" is already the label of"),
            "{why}"
        );
        assert!(w.refused(&opts(Some("otter")), Some("shop")).is_none());
        assert!(w.refused(&opts(None), None).is_none());
    }

    #[test]
    fn the_loop_lists_dedups_opens_and_reports_expired_inboxes_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let (lists_tx, lists) = channel();
        let (req_tx, req_rx) = channel();
        let (ev_tx, ev_rx) = channel();
        // Queued before the loop starts: only the last open is carried out.
        req_tx.send(Req::Open("1".into())).unwrap();
        req_tx.send(Req::Open("2".into())).unwrap();
        // `App` stays on its thread (providers aren't `Send`).
        let worker = std::thread::spawn(move || {
            let mut app = App::for_test(path);
            app.set_provider(Box::new(Fake { lists: lists_tx }));
            let mut old = Inbox::new("fake", "old@x.com".into(), serde_json::Value::Null);
            old.expires_at = Some(chrono::Utc::now() - chrono::Duration::minutes(1));
            let a = Inbox::new("fake", "a@x.com".into(), serde_json::Value::Null);
            // Saved, so a state reload on a slow machine changes nothing.
            let (state, ()) = app
                .store
                .update(|s| {
                    s.add(a);
                    s.add(old);
                })
                .unwrap();
            app.state = state;
            Watcher::with_app(app).run(&req_rx, &mut Test { tx: ev_tx });
        });

        assert_eq!(next(&ev_rx), Ev::Inboxes(2));
        assert_eq!(next(&ev_rx), Ev::Opened("2".into()));
        assert_eq!(next(&ev_rx), Ev::Listed("a@x.com".into(), 1));
        assert_eq!(next(&ev_rx), Ev::Expired("old@x.com".into()));
        assert_eq!(lists.recv().unwrap(), "a@x.com");

        // Not due again for a while, unless asked; the expired inbox is
        // reported only once.
        req_tx.send(Req::Poke("a@x.com".into())).unwrap();
        assert_eq!(next(&ev_rx), Ev::Listed("a@x.com".into(), 1));
        assert!(
            ev_rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "nothing else is due"
        );
        assert_eq!(
            lists.try_iter().count(),
            1,
            "the expired inbox is never listed"
        );

        drop(req_tx);
        worker.join().unwrap();
    }

    #[test]
    fn polling_restarts_only_when_its_settings_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Watcher::with_app(App::for_test(dir.path().to_path_buf()));
        w.set_config(Config::parse("poll_interval = 60").unwrap());
        w.poller.polled("a@x.com", true);
        assert!(!w.poller.is_due("a@x.com"));
        // A save that only adds a key (or changes the provider order) keeps
        // the poller and its push streams.
        w.set_config(
            Config::parse("poll_interval = 60\nnewer = 1\nproviders = [\"mailgw\"]").unwrap(),
        );
        assert!(!w.poller.is_due("a@x.com"));
        // A new interval, push setting or timeout starts over: every inbox
        // is listed again.
        let mut starts_over = |text: &str| {
            w.poller.polled("a@x.com", true);
            w.set_config(Config::parse(text).unwrap());
            w.poller.is_due("a@x.com")
        };
        assert!(starts_over("poll_interval = 30"));
        assert!(starts_over("poll_interval = 30\npush = false"));
        assert!(starts_over(
            "poll_interval = 30\npush = false\nhttp_timeout = 60"
        ));
        assert!(!starts_over(
            "poll_interval = 30\npush = false\nhttp_timeout = 60"
        ));
    }

    #[test]
    fn unknown_keys_are_reported_only_when_they_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        let (log_tx, log) = channel();
        app.log_to(log_tx);
        let mut w = Watcher::with_app(app);
        let mut set = |text: &str| {
            w.set_config(Config::parse(text).unwrap());
            log.try_iter().collect::<Vec<_>>()
        };

        let first = set("gui = 1\npoll_interval = 9");
        assert_eq!(first.len(), 1);
        assert!(
            first[0].starts_with("tmpmail: unknown key \"gui\""),
            "{first:?}"
        );
        // Saving again with the same unknown key says nothing.
        assert!(set("gui = 2\npoll_interval = 10").is_empty());
        // Only a key that wasn't there before is reported.
        let added = set("gui = 2\nnewer = true");
        assert_eq!(added.len(), 1);
        assert!(added[0].contains("\"newer\""), "{added:?}");
        // Removing one doesn't repeat the other.
        assert!(set("gui = 2").is_empty());
        // Gone, and back: reported again.
        assert!(set("poll_interval = 5").is_empty());
        assert_eq!(set("gui = 1").len(), 1);
    }
}
