//! Notifications for new mail, shown while the window isn't focused (or is
//! hidden in the tray). The worker spots new mail in each listing; mail that
//! arrives within a couple of seconds is grouped into one notification.
//! Clicking it brings the window back and opens the message. Sender and
//! subject are the sender's words: they are cleaned (`clean_line`) and capped
//! before they reach the notification server.
//!
//! Windows reports a click to the app only while the toast is on screen;
//! from the notification center it can only start something. So the
//! installed app shows its toasts itself, launching a
//! `tmpmail-desktop://open?inbox=…&message=…` link the installer registers
//! for it: Windows starts the app with the link, and the single-instance
//! plugin hands it to the running one (`activation`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(not(target_os = "linux"))]
use tauri_plugin_notification::NotificationExt;
use tmpmail::extract;
use tmpmail::provider::Summary;
use tmpmail::render::clean_line;

use crate::Shared;

/// Mail arriving this soon after the first of a burst joins its notification.
const GROUP: Duration = Duration::from_secs(2);
/// Longest sender, subject and code shown, in characters.
const SENDER_MAX: usize = 48;
const SUBJECT_MAX: usize = 100;
const CODE_MAX: usize = 32;
/// How many notifications remember what a click opens.
#[cfg(not(target_os = "linux"))]
const KEEP_CLICKS: usize = 64;
/// The link scheme a Windows toast launches (registered by the installer,
/// `packaging/windows/hooks.nsh`).
#[cfg_attr(not(windows), allow(dead_code))]
pub const ACTIVATION_SCHEME: &str = "tmpmail-desktop";

/// A message that just arrived.
#[derive(Debug, Clone)]
pub struct Arrival {
    pub address: String,
    pub id: String,
    /// Display name (or address) of the sender, cleaned.
    pub sender: String,
    pub subject: String,
    /// The code `extract` found in the subject and preview, if any.
    pub code: Option<String>,
    pub date: Option<chrono::DateTime<chrono::Utc>>,
}

/// What a click on a notification opens.
#[derive(Debug, Clone)]
pub struct Target {
    pub address: String,
    pub id: Option<String>,
}

/// New-mail detection and grouping, owned by the worker.
#[derive(Default)]
pub struct Arrivals {
    /// Message ids of each inbox's last listing.
    known: HashMap<String, HashSet<String>>,
    pending: Vec<Arrival>,
    since: Option<Instant>,
}

impl Arrivals {
    /// [`Arrivals::listed_with_code`]'s new mail.
    #[cfg(test)]
    pub fn listed(&mut self, address: &str, list: &[Summary]) -> Vec<Arrival> {
        self.listed_with_code(address, list, chrono::Utc::now()).0
    }

    /// The messages in `list` that weren't in the inbox's last listing (an
    /// inbox's first listing only records what is already there), and for
    /// a first listing the newest code already in it ([`recent_code`]): the
    /// tray's "Copy code" offers it right after a start, before new mail.
    pub fn listed_with_code(
        &mut self,
        address: &str,
        list: &[Summary],
        now: chrono::DateTime<chrono::Utc>,
    ) -> (
        Vec<Arrival>,
        Option<(String, chrono::DateTime<chrono::Utc>)>,
    ) {
        let first = !self.known.contains_key(address);
        let arrived = self.listed_ids(address, list);
        let code = first
            .then(|| recent_code(list, now, crate::tray::CODE_FRESH))
            .flatten();
        (arrived, code)
    }

    fn listed_ids(&mut self, address: &str, list: &[Summary]) -> Vec<Arrival> {
        let ids: HashSet<String> = list.iter().map(|s| s.id.clone()).collect();
        let arrived = match self.known.get(address) {
            None => Vec::new(),
            Some(known) => list
                .iter()
                .filter(|s| !known.contains(&s.id))
                .map(|s| arrival(address, s))
                .collect(),
        };
        self.known.insert(address.to_string(), ids);
        arrived
    }

    /// Stops tracking inboxes that are gone.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.known.retain(|a, _| keep(a));
        self.pending.retain(|a| keep(&a.address));
    }

    /// Queues mail for the next notification.
    pub fn queue(&mut self, arrived: Vec<Arrival>) {
        if arrived.is_empty() {
            return;
        }
        self.since.get_or_insert_with(Instant::now);
        // Newest first, across inboxes too.
        self.pending.splice(0..0, arrived);
    }

    /// The queued mail once its burst is over.
    pub fn due(&mut self) -> Option<Vec<Arrival>> {
        if self.since?.elapsed() < GROUP {
            return None;
        }
        self.since = None;
        Some(std::mem::take(&mut self.pending))
    }
}

fn arrival(address: &str, s: &Summary) -> Arrival {
    Arrival {
        address: address.to_string(),
        id: s.id.clone(),
        sender: cap(&sender_name(&s.from), SENDER_MAX),
        subject: cap(&clean_line(&s.subject), SUBJECT_MAX),
        code: code_of(s),
        date: s.date,
    }
}

/// The newest code in an inbox's listing among mail of the last `fresh`
/// before `now` (mail dated later than now, a skewed or lying sender's,
/// doesn't count), with its mail's date. Codes are looked for in the
/// subject and preview, as for notifications.
pub fn recent_code(
    list: &[Summary],
    now: chrono::DateTime<chrono::Utc>,
    fresh: std::time::Duration,
) -> Option<(String, chrono::DateTime<chrono::Utc>)> {
    let fresh = chrono::Duration::from_std(fresh).ok()?;
    list.iter()
        .filter_map(|s| {
            let d = s.date?;
            let age = now.signed_duration_since(d);
            (age >= chrono::Duration::zero() && age < fresh)
                .then(|| code_of(s).map(|c| (c, d)))
                .flatten()
        })
        .max_by_key(|(_, d)| *d)
}

/// The code in a listed message's subject and preview, cleaned and capped.
fn code_of(s: &Summary) -> Option<String> {
    extract::otp(&s.subject, &s.intro, None).map(|c| cap(&clean_line(&c), CODE_MAX))
}

/// "GitHub" for `GitHub <noreply@github.com>`, else the address, cleaned.
pub fn sender_name(from: &str) -> String {
    let from = clean_line(from);
    let name = from
        .rsplit_once('<')
        .map(|(name, _)| name.trim().trim_matches('"').trim())
        .filter(|n| !n.is_empty());
    match name {
        Some(n) => n.to_string(),
        None => from
            .trim_matches(|c| c == '<' || c == '>')
            .trim()
            .to_string(),
    }
}

/// At most `max` characters, with an ellipsis when cut.
pub fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The title and body of the notification for `batch` (newest first).
pub fn text(batch: &[Arrival]) -> (String, String) {
    let subject = |a: &Arrival| {
        if a.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            a.subject.clone()
        }
    };
    let coded = batch.iter().find(|a| a.code.is_some());
    match batch {
        [one] => {
            let title = match &one.code {
                Some(code) => format!("Code {code} from {}", one.sender),
                None => one.sender.clone(),
            };
            (title, subject(one))
        }
        _ => {
            let n = batch.len();
            let title = match coded {
                Some(a) => format!(
                    "Code {} from {}, and {} more",
                    a.code.as_deref().unwrap_or_default(),
                    a.sender,
                    n - 1
                ),
                None => format!("{n} new messages"),
            };
            let mut lines: Vec<String> = batch
                .iter()
                .take(3)
                .map(|a| format!("{}: {}", a.sender, subject(a)))
                .collect();
            if n > 3 {
                lines.push(format!("and {} more", n - 3));
            }
            (title, lines.join("\n"))
        }
    }
}

/// What a click on the notification for `batch` opens: the newest message
/// with a code, else the newest message.
pub fn target(batch: &[Arrival]) -> Option<Target> {
    let a = batch.iter().find(|a| a.code.is_some()).or(batch.first())?;
    Some(Target {
        address: a.address.clone(),
        id: Some(a.id.clone()),
    })
}

/// Escapes the body for servers that read it as markup (most Linux ones),
/// so a subject like `<img src=…>` is shown, never loaded.
pub fn body_for_server(body: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        use std::sync::OnceLock;
        static MARKUP: OnceLock<bool> = OnceLock::new();
        let markup = *MARKUP.get_or_init(|| {
            notify_rust::get_capabilities()
                .map(|caps| caps.iter().any(|c| c == "body-markup"))
                // Unknown: escape, since the common servers parse markup.
                .unwrap_or(true)
        });
        if markup {
            return escape_markup(body);
        }
    }
    body.to_string()
}

/// Escapes the body for a Linux server that reads markup. Quotes stay: they
/// need no escape there, and some servers would show `&apos;` as is.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// [`escape_markup`] plus quotes, for the toast's XML (text and the
/// `launch` attribute). Not for Linux servers: some show `&apos;` as is.
#[cfg_attr(not(windows), allow(dead_code))]
fn escape_xml(s: &str) -> String {
    escape_markup(s)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The link a toast launches to open `target` (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn activation_link(target: &Target) -> String {
    let mut params = vec![("inbox", target.address.as_str())];
    if let Some(id) = &target.id {
        params.push(("message", id.as_str()));
    }
    tauri::Url::parse_with_params(&format!("{ACTIVATION_SCHEME}://open"), &params)
        .expect("a constant base URL")
        .into()
}

/// What an activation link asks to open: an inbox and maybe a message,
/// checked like any input (it could come from any program, or a web page
/// the user let open the link). `None` for anything else; the address must
/// still be a stored inbox, which `reveal` and the page check.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn activation(arg: &str) -> Option<Target> {
    if arg.len() > 2048 {
        return None;
    }
    let url = tauri::Url::parse(arg).ok()?;
    if url.scheme() != ACTIVATION_SCHEME
        || url.host_str() != Some("open")
        || !matches!(url.path(), "" | "/")
        || url.fragment().is_some()
        || url.username() != ""
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let (mut address, mut id) = (None, None);
    for (key, value) in url.query_pairs() {
        let slot = match &*key {
            "inbox" => &mut address,
            "message" => &mut id,
            _ => return None,
        };
        if slot.replace(value.into_owned()).is_some() {
            return None; // given twice
        }
    }
    let address = address.filter(|a| {
        a.len() <= 254 && a.contains('@') && !a.chars().any(|c| c.is_control() || c.is_whitespace())
    })?;
    if let Some(id) = &id
        && (id.is_empty() || id.len() > 512 || id.chars().any(char::is_control))
    {
        return None;
    }
    Some(Target { address, id })
}

/// Whether the installer registered the activation scheme for this very
/// exe: then it also made the Start menu shortcut that carries the app id
/// Windows needs to show a toast, and a click starts this app. Release
/// builds only (a debug build runs beside the installed app).
#[cfg(windows)]
fn installed() -> bool {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| {
        if cfg!(debug_assertions) {
            return false;
        }
        // Not Tauri's `current_exe`: it is canonicalized, so it starts with
        // `\\?\`, which the installer's path doesn't.
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let exe = exe.display().to_string();
        let exe = exe.strip_prefix(r"\\?\").unwrap_or(&exe);
        let expected = format!("\"{exe}\" \"%1\"");
        scheme_command().is_some_and(|c| c.eq_ignore_ascii_case(&expected))
    })
}

/// The command registered for the activation scheme, if any.
#[cfg(windows)]
fn scheme_command() -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CLASSES_ROOT, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;
    let key = HSTRING::from(format!("{ACTIVATION_SCHEME}\\shell\\open\\command"));
    let mut buf = [0u16; 1024];
    let mut len = u32::try_from(std::mem::size_of_val(&buf)).ok()?;
    // SAFETY: reads a string value into `buf`, whose size in bytes is
    // `len`; the call writes at most that and sets `len` to what it wrote.
    let read = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            &key,
            None,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if read.is_err() {
        return None;
    }
    let chars = (len as usize / 2).min(buf.len());
    let value = String::from_utf16(&buf[..chars]).ok()?;
    Some(value.trim_end_matches('\0').to_string())
}

/// The toast's XML: a click (on screen or in the notification center)
/// launches `link`. Title and body are the sender's words, escaped.
#[cfg_attr(not(windows), allow(dead_code))]
fn toast_xml(title: &str, body: &str, link: &str) -> String {
    let esc = escape_xml;
    format!(
        "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding template=\"ToastGeneric\">\
         <text>{}</text><text>{}</text></binding></visual></toast>",
        esc(link),
        esc(title),
        esc(body)
    )
}

/// Shows a toast that launches `link` when clicked, under the app's id.
#[cfg(windows)]
fn show_toast(app: &AppHandle, title: &str, body: &str, link: &str) -> windows::core::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::HSTRING;
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(toast_xml(title, body, link)))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    let id = HSTRING::from(app.config().identifier.as_str());
    ToastNotificationManager::CreateToastNotifierWithId(&id)?.Show(&toast)
}

/// Shows a notification that opens `target` when clicked.
pub fn show(app: &AppHandle, shared: &Shared, title: &str, body: &str, target: Option<Target>) {
    #[cfg(windows)]
    if let Some(t) = &target
        && installed()
    {
        match show_toast(app, title, body, &activation_link(t)) {
            Ok(()) => return,
            Err(e) => eprintln!("tmpmail-gui: could not show a toast ({e}), trying the plugin"),
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        match shared.freedesktop.get() {
            Some(notifier) => notifier.show(title, body, target),
            None => eprintln!("tmpmail-gui: notifications aren't set up yet"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    show_with_plugin(app, shared, title, body, target);
}

/// Windows (debug or not installed) and macOS: through the plugin.
#[cfg(not(target_os = "linux"))]
fn show_with_plugin(
    app: &AppHandle,
    shared: &Shared,
    title: &str,
    body: &str,
    target: Option<Target>,
) {
    let id = shared.next_notification.fetch_add(1, Ordering::Relaxed);
    if let Some(t) = target {
        let mut clicks = shared.clicks.lock().expect("clicks lock");
        clicks.insert(id, t);
        while clicks.len() > KEEP_CLICKS {
            let Some(oldest) = clicks.keys().min().copied() else {
                break;
            };
            clicks.remove(&oldest);
        }
    }
    let shown = app
        .notification()
        .builder()
        .id(id)
        .title(title)
        .body(body_for_server(body))
        .show();
    if let Err(e) = shown {
        eprintln!("tmpmail-gui: could not show a notification: {e}");
    }
}

/// Whether new mail should be announced: the setting is on and the window
/// isn't focused (a hidden or minimized window never is).
pub fn wanted(shared: &Shared) -> bool {
    shared.gui.lock().expect("gui lock").notifications && !shared.focused.load(Ordering::Relaxed)
}

/// What a notification click asked for before the page could take it.
#[derive(Default)]
pub struct Pending {
    /// The page has called `init` (so it listens for `reveal`).
    pub page_ready: bool,
    pub target: Option<Target>,
}

/// Handles the command line of a second start of the app (Windows: a toast
/// was clicked, or the app was started again): brings the window back and
/// opens what an activation link asks for, if it names a stored inbox.
/// Before the page is up, the target waits for `init` (the page checks it
/// against its inboxes). A start without a link focuses the window at most
/// once a second, so a flood of starts can't keep taking the focus.
#[cfg(all(windows, not(debug_assertions)))]
pub fn relaunched(app: &AppHandle, shared: &Shared, args: &[String]) {
    static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    let target = args.iter().skip(1).find_map(|a| activation(a));
    let page_ready = {
        let mut pending = shared.pending.lock().expect("pending lock");
        if !pending.page_ready && target.is_some() {
            // The inboxes aren't known yet: the page checks it.
            pending.target = target.clone();
        }
        pending.page_ready
    };
    let target = target.filter(|t| {
        !page_ready
            || shared
                .inboxes
                .lock()
                .expect("inboxes lock")
                .contains_key(&t.address)
    });
    // Nothing to open (no link, or not a stored inbox): only focus, at most
    // once a second.
    if target.is_none() {
        let mut last = LAST.lock().expect("relaunch lock");
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        *last = Some(Instant::now());
    }
    reveal(app, if page_ready { target } else { None });
}

/// Brings the window back (from the tray, minimized or behind others) and
/// tells the page what to open.
pub fn reveal(app: &AppHandle, target: Option<Target>) {
    raise(app);
    open(app, target);
}

/// Shows, unminimizes and focuses the window. (A compositor may refuse
/// the focus: on Linux a notification click brings a token for it.)
pub fn raise(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Tells the page what to open.
fn open(app: &AppHandle, target: Option<Target>) {
    if let Some(t) = target {
        let _ = app.emit("reveal", json!({ "address": t.address, "id": t.id }));
    }
}

/// Linux: shows notifications through the notification server directly
/// (`freedesktop.rs`). A click brings the window back with the activation
/// token the server sent for it, or as before without one (an older server).
#[cfg(target_os = "linux")]
pub fn register(app: &AppHandle, shared: Arc<Shared>) {
    let handle = app.clone();
    let notifier = crate::freedesktop::Notifier::new(move |target, token| {
        if !token.is_some_and(|t| crate::freedesktop::raise(&handle, t)) {
            raise(&handle);
        }
        open(&handle, target);
    });
    let _ = shared.freedesktop.set(notifier);
}

/// Routes clicks on notifications. Registered before the first notification
/// is shown: the plugin reports clicks only for notifications shown while a
/// handler exists.
#[cfg(not(target_os = "linux"))]
pub fn register(app: &AppHandle, shared: Arc<Shared>) {
    let handle = app.clone();
    let registered = app.notification().on_action(move |performed| {
        // `tap` is a click on the notification itself; it has no buttons.
        if performed.action_id() != "tap" && performed.action_id() != "default" {
            return;
        }
        let target = performed.notification().and_then(|n| {
            shared
                .clicks
                .lock()
                .expect("clicks lock")
                .get(&n.id())
                .cloned()
        });
        reveal(&handle, target);
    });
    if let Err(e) = registered {
        eprintln!("tmpmail-gui: notification clicks won't open the app: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, from: &str, subject: &str, intro: &str) -> Summary {
        Summary {
            id: id.into(),
            from: from.into(),
            subject: subject.into(),
            intro: intro.into(),
            ..Default::default()
        }
    }

    #[test]
    fn only_mail_after_the_first_listing_is_new() {
        let mut a = Arrivals::default();
        let one = summary("1", "A <a@x.com>", "hi", "");
        assert!(a.listed("me@x.com", std::slice::from_ref(&one)).is_empty());
        assert!(a.listed("me@x.com", std::slice::from_ref(&one)).is_empty());
        let two = summary(
            "2",
            "GitHub <noreply@github.com>",
            "Your code",
            "Use 482913 to sign in",
        );
        let new = a.listed("me@x.com", &[two, one]);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].id, "2");
        assert_eq!(new[0].sender, "GitHub");
        assert_eq!(new[0].code.as_deref(), Some("482913"));
        a.retain(|_| false);
        assert!(a.known.is_empty());
    }

    #[test]
    fn headers_the_core_decoded_are_not_decoded_again() {
        // The core decodes RFC 2047 once (0.3.3); what is left, even text
        // that looks encoded, is the sender's own and shown as is.
        let s = summary(
            "1",
            "\"Zoë Ünicode\" <hello@example.org>",
            "Café résumé ✓ =?utf-8?q?not_decoded_again?=",
            "Code 551177",
        );
        let a = arrival("me@x.com", &s);
        assert_eq!(a.sender, "Zoë Ünicode");
        assert_eq!(a.subject, "Café résumé ✓ =?utf-8?q?not_decoded_again?=");
        assert_eq!(a.code.as_deref(), Some("551177"));
    }

    #[test]
    fn the_newest_recent_code_is_found_in_a_listing() {
        let now = chrono::Utc::now();
        let at = |mins: i64, subject: &str| {
            let mut s = summary("x", "A <a@b.c>", subject, "");
            s.date = Some(now - chrono::Duration::minutes(mins));
            s
        };
        let fifteen = std::time::Duration::from_secs(15 * 60);
        let code = |list: &[Summary]| recent_code(list, now, fifteen).map(|c| c.0);
        // Round 2's case (N1): the code came 11 minutes before the start.
        let list = [
            at(20, "Your code is 111111"),
            at(11, "Code 222222"),
            at(12, "Your code is 333333"),
        ];
        assert_eq!(code(&list), Some("222222".into()));
        assert_eq!(code(&[at(20, "Your code is 111111")]), None);
        assert_eq!(code(&[at(1, "No code here")]), None);
        // Mail dated in the future doesn't count.
        assert_eq!(code(&[at(-60, "Your code is 444444")]), None);

        // Only an inbox's first listing gives it.
        let mut arrivals = Arrivals::default();
        let (new, first) = arrivals.listed_with_code("a@x.com", &list, now);
        assert!(new.is_empty());
        assert_eq!(first.map(|c| c.0), Some("222222".into()));
        assert_eq!(arrivals.listed_with_code("a@x.com", &list, now).1, None);
    }

    #[test]
    fn bursts_wait_for_the_group_window() {
        let mut a = Arrivals::default();
        assert!(a.due().is_none());
        a.queue(vec![arrival(
            "me@x.com",
            &summary("1", "a@x.com", "one", ""),
        )]);
        a.queue(vec![arrival(
            "me@x.com",
            &summary("2", "b@x.com", "two", ""),
        )]);
        assert!(a.due().is_none(), "the burst is still open");
        a.since = Some(Instant::now() - GROUP);
        let batch = a.due().unwrap();
        assert_eq!(
            batch.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
            ["2", "1"]
        );
        assert!(a.due().is_none());
    }

    #[test]
    fn text_names_the_code_sender_and_subject() {
        let a = |id: &str, from: &str, subject: &str, intro: &str| {
            arrival("me@x.com", &summary(id, from, subject, intro))
        };
        let code = a(
            "1",
            "GitHub <noreply@github.com>",
            "Sign in",
            "Your code is 482913",
        );
        assert_eq!(
            text(std::slice::from_ref(&code)),
            ("Code 482913 from GitHub".into(), "Sign in".into())
        );
        let plain = a("2", "shop@x.com", "", "Thanks");
        assert_eq!(
            text(std::slice::from_ref(&plain)),
            ("shop@x.com".into(), "(no subject)".into())
        );
        let batch = [plain.clone(), code.clone(), plain.clone(), plain];
        let (title, body) = text(&batch);
        assert_eq!(title, "Code 482913 from GitHub, and 3 more");
        assert_eq!(body.lines().count(), 4);
        assert!(body.ends_with("and 1 more"));
        assert_eq!(target(&batch).unwrap().id.as_deref(), Some("1"));
    }

    #[test]
    fn activation_links_round_trip_and_nothing_else_passes() {
        let t = Target {
            address: "otter+x@mail.tm".into(),
            id: Some("6a/b c&d=e".into()),
        };
        let link = activation_link(&t);
        assert!(link.starts_with("tmpmail-desktop://open?inbox=otter%2Bx%40mail.tm&message="));
        let back = activation(&link).unwrap();
        assert_eq!((back.address, back.id), (t.address.clone(), t.id.clone()));
        let inbox_only = activation_link(&Target {
            address: t.address.clone(),
            id: None,
        });
        assert_eq!(activation(&inbox_only).unwrap().id, None);

        for bad in [
            "tmpmail-desktop://open",
            "tmpmail-desktop://open?message=1",
            "tmpmail-desktop://delete?inbox=a%40b.c",
            "tmpmail-desktop://open/x?inbox=a%40b.c",
            "tmpmail-desktop://open?inbox=a%40b.c&inbox=c%40d.e",
            "tmpmail-desktop://open?inbox=a%40b.c&run=1",
            "tmpmail-desktop://open?inbox=a%40b.c&message=",
            "tmpmail-desktop://open?inbox=a%40b.c&message=%1B[2J",
            "tmpmail-desktop://open?inbox=not-an-address",
            "tmpmail-desktop://open?inbox=a%40b.c#x",
            "tmpmail-desktop://u:p@open?inbox=a%40b.c",
            "https://open?inbox=a%40b.c",
            "--inbox=a@b.c",
        ] {
            assert!(activation(bad).is_none(), "{bad}");
        }
        let long = format!(
            "tmpmail-desktop://open?inbox=a%40b.c&message={}",
            "x".repeat(2100)
        );
        assert!(activation(&long).is_none());
    }

    #[test]
    fn the_toast_xml_escapes_the_senders_words() {
        let xml = toast_xml(
            "Code 1 from <b>\"Evil\"</b>",
            "a & b's",
            "tmpmail-desktop://open?inbox=a%40b.c&message=1",
        );
        assert_eq!(
            xml,
            "<toast activationType=\"protocol\" launch=\"tmpmail-desktop://open?inbox=a%40b.c&amp;message=1\">\
             <visual><binding template=\"ToastGeneric\"><text>Code 1 from &lt;b&gt;&quot;Evil&quot;&lt;/b&gt;</text>\
             <text>a &amp; b&apos;s</text></binding></visual></toast>"
        );
    }

    #[test]
    fn sender_words_are_cleaned_and_capped() {
        let s = summary(
            "1",
            "\u{202e}Evil\u{1b}[31m <e@x.com>",
            &format!("{}\u{7}", "long ".repeat(40)),
            "",
        );
        let a = arrival("me@x.com", &s);
        assert!(!a.sender.contains('\u{202e}') && !a.sender.contains('\u{1b}'));
        assert!(a.subject.chars().count() <= SUBJECT_MAX);
        assert!(a.subject.ends_with('…'));
        assert!(!a.subject.contains('\u{7}'));
        assert_eq!(
            escape_markup("<img src=x> & <b>"),
            "&lt;img src=x&gt; &amp; &lt;b&gt;"
        );
        assert_eq!(escape_markup(r#""a's""#), r#""a's""#);
        assert_eq!(escape_xml(r#"<"a's">"#), "&lt;&quot;a&apos;s&quot;&gt;");
    }
}
