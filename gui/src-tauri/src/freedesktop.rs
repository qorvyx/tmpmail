//! Linux: notifications through the Freedesktop notification server, on
//! D-Bus ourselves rather than through `tauri-plugin-notification`, which
//! ignores the `ActivationToken` signal. A notification server (spec 1.2+:
//! KDE Plasma, GNOME) sends that token just before it reports a click
//! (`ActionInvoked`). It proves the user just acted, so the compositor lets
//! the app raise its window: without it KWin's focus-stealing prevention
//! only marks the window as wanting attention.
//!
//! The token is the server's words: only a short printable ASCII one is
//! kept (`clean_token`), and only from the server that showed that
//! notification. It goes to GTK, which hands it to the compositor
//! (`present`): over `xdg_activation_v1` on Wayland, as the user time of
//! the window (the token's `_TIME` part) on X11. Without one the window is
//! raised as before.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::Value;

use crate::notify::Target;

/// The bus name, object path and interface of the notification server.
const BUS: (&str, &str, &str) = (
    "org.freedesktop.Notifications",
    "/org/freedesktop/Notifications",
    "org.freedesktop.Notifications",
);
/// The name the server shows in the notification's header. (The plugin
/// sent the binary's name, `tmpmail-gui`.)
pub const APP_NAME: &str = crate::PROGRAM;
/// The installed desktop file, without `.desktop`, for the `desktop-entry`
/// hint: KDE and GNOME take the app's name, icon and notification settings
/// from it, and group the notifications under it.
pub const DESKTOP_ENTRY: &str = crate::PROGRAM;
/// The notification's icon, by name: the packages install it in the hicolor
/// theme (the desktop file's `Icon`).
pub const ICON: &str = "tmpmail-gui";

/// The notification's icon: [`ICON`] by name, except when running from an
/// AppImage. That installs no desktop file and no icon in the theme, so the
/// name leads nowhere and servers show a generic icon (GNOME did); a path
/// inside its mount would go when the app quits, and notification
/// histories keep the icon. So the app's own logo, built in, is written to
/// the cache folder and named by its `file://` URI.
fn icon() -> &'static str {
    static ICON_NOW: OnceLock<String> = OnceLock::new();
    ICON_NOW.get_or_init(|| {
        let cache = dirs_cache().map(|c| c.join("tmpmail").join("tmpmail-gui.png"));
        match (in_appimage(), cache) {
            (true, Some(path)) => logo_file(&path).unwrap_or_else(|| ICON.into()),
            _ => ICON.into(),
        }
    })
}

/// The logo the notifications of an AppImage show.
const LOGO: &[u8] = include_bytes!("../icons/128x128.png");

/// Whether this program runs from a mounted (or extracted) AppImage: its
/// own file is under `$APPDIR`. (`APPIMAGE` and `APPDIR` alone could come
/// from another AppImage this was started from.)
fn in_appimage() -> bool {
    let (Some(appdir), Ok(exe)) = (std::env::var_os("APPDIR"), std::env::current_exe()) else {
        return false;
    };
    std::env::var_os("APPIMAGE").is_some() && exe.starts_with(appdir)
}

fn dirs_cache() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
}

/// Writes [`LOGO`] to `path` (unless it is there already) and returns its
/// `file://` URI, or `None` if it couldn't be written.
fn logo_file(path: &Path) -> Option<String> {
    if std::fs::read(path).ok().as_deref() != Some(LOGO) {
        std::fs::create_dir_all(path.parent()?).ok()?;
        let tmp = path.with_extension("png.new");
        std::fs::write(&tmp, LOGO).ok()?;
        std::fs::rename(&tmp, path).ok()?;
    }
    let mut uri = String::from("file://");
    for b in path.to_str()?.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    Some(uri)
}

/// How many notifications remember what a click opens.
const KEEP: usize = 64;
/// Longest activation token kept, in bytes.
const TOKEN_MAX: usize = 512;
/// How long a call to the server may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// What a notification is shown with: the arguments of `Notify`.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub app_name: &'static str,
    pub icon: &'static str,
    pub summary: String,
    pub body: String,
    /// Pairs of action id and label: `default` is a click on the
    /// notification. Its label shows only where a server lists actions
    /// (dunst's menu, for one); Plasma's history shows its own "View"
    /// button for it, and GNOME doesn't show it.
    pub actions: Vec<&'static str>,
    pub hints: Vec<(&'static str, &'static str)>,
    /// -1: the server's default.
    pub timeout: i32,
}

/// The `Notify` arguments for a notification titled `summary` (made plain,
/// [`plain_summary`]); `body` is escaped already if the server reads markup.
pub fn request(summary: &str, body: &str) -> Request {
    Request {
        app_name: APP_NAME,
        icon: icon(),
        summary: plain_summary(summary),
        body: body.to_string(),
        actions: vec!["default", "Open"],
        hints: vec![("desktop-entry", DESKTOP_ENTRY)],
        timeout: -1,
    }
}

/// A shown notification.
struct Shown {
    /// Its id at the server.
    id: u32,
    /// The unique bus name of the server that showed it: only its signals count.
    server: String,
    target: Option<Target>,
    /// The token sent for the click that comes next.
    token: Option<String>,
}

/// A signal of the notification server, with what it says.
#[derive(Debug, PartialEq)]
enum Signal {
    Token(u32, String),
    Action(u32, String),
    Closed(u32),
}

/// What a click asks for: the target, and the token to raise the window with.
type Click = (Option<Target>, Option<String>);

pub struct Notifier {
    /// The connection notifications are sent on and signals arrive on: some
    /// servers send a notification's signals only to the connection that
    /// sent it, and GNOME closes them once it goes away, so it stays open.
    /// Made again by the next notification if it couldn't be made or its
    /// signals stopped (the flag, cleared when they do).
    connection: Mutex<Option<(Connection, Arc<AtomicBool>)>>,
    /// The newest last.
    shown: Mutex<VecDeque<Shown>>,
    on_click: Box<dyn Fn(Option<Target>, Option<String>) + Send + Sync>,
}

impl Notifier {
    pub fn new(
        on_click: impl Fn(Option<Target>, Option<String>) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Notifier {
            connection: Mutex::new(None),
            shown: Mutex::new(VecDeque::new()),
            on_click: Box::new(on_click),
        })
    }

    /// Shows a notification that opens `target` when clicked, on a thread
    /// of its own (the server may be slow to answer). `body` is escaped
    /// there if the server reads markup.
    pub fn show(self: &Arc<Self>, summary: &str, body: &str, target: Option<Target>) {
        let me = self.clone();
        let (summary, body) = (summary.to_string(), body.to_string());
        let spawned = std::thread::Builder::new()
            .name("notification".into())
            .spawn(move || {
                let request = request(&summary, &crate::notify::body_for_server(&body));
                if let Err(e) = me.send(&request, target) {
                    eprintln!("tmpmail-gui: could not show a notification: {e}");
                }
            });
        if let Err(e) = spawned {
            eprintln!("tmpmail-gui: could not show a notification: {e}");
        }
    }

    fn send(self: &Arc<Self>, request: &Request, target: Option<Target>) -> zbus::Result<()> {
        let connection = self.connection()?;
        let hints: HashMap<&str, Value<'_>> = request
            .hints
            .iter()
            .map(|(k, v)| (*k, Value::from(*v)))
            .collect();
        let (destination, path, interface) = BUS;
        let reply = connection.call_method(
            Some(destination),
            path,
            Some(interface),
            "Notify",
            &(
                request.app_name,
                0u32,
                request.icon,
                &request.summary,
                &request.body,
                &request.actions,
                hints,
                request.timeout,
            ),
        )?;
        let id: u32 = reply.body().deserialize()?;
        let server = reply
            .header()
            .sender()
            .map(|s| s.to_string())
            .unwrap_or_default();
        // Recorded once the server answered. (The lock isn't held over the
        // call, so the signals of other notifications are handled meanwhile;
        // one about this notification can only follow a click on it.)
        remember(
            &mut self.shown.lock().expect("notifications lock"),
            Shown {
                id,
                server,
                target,
                token: None,
            },
        );
        Ok(())
    }

    /// The open connection, or a new one.
    fn connection(self: &Arc<Self>) -> zbus::Result<Connection> {
        let mut slot = self.connection.lock().expect("connection lock");
        if let Some((connection, alive)) = slot.as_ref()
            && alive.load(Ordering::Relaxed)
        {
            return Ok(connection.clone());
        }
        let (connection, alive) = listen(Arc::downgrade(self))?;
        *slot = Some((connection.clone(), alive));
        Ok(connection)
    }

    /// Handles a signal from `sender`. Returns what a click asks for.
    fn handle(&self, sender: &str, signal: Signal) -> Option<Click> {
        let mut shown = self.shown.lock().expect("notifications lock");
        let id = match &signal {
            Signal::Token(id, _) | Signal::Action(id, _) | Signal::Closed(id) => *id,
        };
        let at = shown
            .iter()
            .position(|s| s.id == id && s.server == sender)?;
        let entry = &mut shown[at];
        match signal {
            Signal::Token(_, token) => {
                entry.token = clean_token(&token);
                None
            }
            // `default` is a click on the popup's body, or on KDE history's
            // "View" button; there are no other actions.
            Signal::Action(_, action) if action == "default" => {
                Some((entry.target.clone(), entry.token.take()))
            }
            Signal::Action(..) => None,
            Signal::Closed(_) => {
                shown.remove(at);
                None
            }
        }
    }
}

/// Adds a shown notification, forgetting the oldest beyond [`KEEP`]. (In
/// the order shown: a restarted server counts its ids from 1 again.)
fn remember(shown: &mut VecDeque<Shown>, entry: Shown) {
    shown.push_back(entry);
    while shown.len() > KEEP {
        shown.pop_front();
    }
}

/// The token, if it is one to pass on: printable ASCII, not empty, short.
pub fn clean_token(token: &str) -> Option<String> {
    let ok = !token.is_empty()
        && token.len() <= TOKEN_MAX
        && token.bytes().all(|b| (b' '..=b'~').contains(&b));
    ok.then(|| token.to_string())
}

/// The X server time of an X11 startup notification id: the number after
/// its last `_TIME`, as GTK reads it.
pub fn startup_time(token: &str) -> Option<u32> {
    let (_, rest) = token.rsplit_once("_TIME")?;
    let digits: &str = &rest[..rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len())];
    digits.parse().ok().filter(|t| *t != 0)
}

/// The notification server's signal in `message`, and who sent it.
fn parse(message: &zbus::Message) -> Option<(String, Signal)> {
    let header = message.header();
    let sender = header.sender()?.to_string();
    let body = message.body();
    let signal = match header.member()?.as_str() {
        "ActivationToken" => {
            let (id, token) = body.deserialize::<(u32, String)>().ok()?;
            Signal::Token(id, token)
        }
        "ActionInvoked" => {
            let (id, action) = body.deserialize::<(u32, String)>().ok()?;
            Signal::Action(id, action)
        }
        "NotificationClosed" => Signal::Closed(body.deserialize::<(u32, u32)>().ok()?.0),
        _ => return None,
    };
    Some((sender, signal))
}

/// Connects to the session bus and handles the server's signals on a thread
/// of its own. The subscription is in place when this returns, so no signal
/// of a later notification is missed.
/// The flag is cleared once the signals stop (the connection closed).
fn listen(notifier: Weak<Notifier>) -> zbus::Result<(Connection, Arc<AtomicBool>)> {
    let (_, path, interface) = BUS;
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(CALL_TIMEOUT)
        .build()?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(path)?
        .interface(interface)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &connection, None)?;
    let alive = Arc::new(AtomicBool::new(true));
    let flag = alive.clone();
    std::thread::Builder::new()
        .name("notification-signals".into())
        .spawn(move || {
            for message in signals.flatten() {
                let Some(notifier) = notifier.upgrade() else {
                    break;
                };
                if let Some((sender, signal)) = parse(&message)
                    && let Some((target, token)) = notifier.handle(&sender, signal)
                {
                    (notifier.on_click)(target, token);
                }
            }
            flag.store(false, Ordering::Relaxed);
        })
        .map_err(|e| zbus::Error::Failure(format!("no thread for the signals: {e}")))?;
    Ok((connection, alive))
}

/// Brings the window back with the activation token of a click, on the main
/// thread (GTK). False when that can't be done: then raise it as usual.
pub fn raise(app: &AppHandle, token: String) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    app.run_on_main_thread(move || match window.gtk_window() {
        Ok(gtk_window) => present(&gtk_window, &token),
        Err(_) => crate::notify::raise(window.app_handle()),
    })
    .is_ok()
}

/// Shows, unminimizes and focuses `window`, handing `token` to the
/// compositor the way GTK does for an app started with one.
fn present(window: &gtk::ApplicationWindow, token: &str) {
    use gtk::prelude::*;
    window.deiconify();
    let display = WidgetExt::display(window);
    if display.is::<gdkx11::X11Display>() {
        // The token is a startup notification id: its `_TIME` part is the
        // X server time of the click. KWin lets a window with a user time
        // newer than the active window's take the focus.
        // Set before a hidden window is mapped, so KWin judges the map by it.
        let time = startup_time(token);
        if let Some(time) = time
            && let Some(gdk_window) = window.window()
            && let Ok(x11) = gdk_window.downcast::<gdkx11::X11Window>()
        {
            x11.set_user_time(time);
        }
        if !window.is_visible() {
            window.show_all();
        }
        window.present_with_time(time.unwrap_or(0));
    } else if display.type_().is_a(wayland_display()) {
        // GDK sends it (`xdg_activation_v1.activate`) with the next focus
        // request, or, for a hidden window, when it is shown again; once.
        let Ok(token) = std::ffi::CString::new(token) else {
            return;
        };
        // SAFETY: `display` is a live GdkWaylandDisplay (checked above),
        // and GDK copies the string.
        unsafe {
            use gtk::glib::translate::ToGlibPtr;
            let raw: *mut gtk::gdk::ffi::GdkDisplay = display.to_glib_none().0;
            gdk_wayland_sys::gdk_wayland_display_set_startup_notification_id(
                raw.cast(),
                token.as_ptr(),
            );
        }
        if window.is_visible() {
            window.present();
        } else {
            window.show_all();
        }
    } else {
        window.show_all();
        window.present();
    }
}

/// The GType of GDK's Wayland display.
fn wayland_display() -> gtk::glib::Type {
    // SAFETY: a plain type lookup; GDK registers the type on first use.
    unsafe { gtk::glib::translate::from_glib(gdk_wayland_sys::gdk_wayland_display_get_type()) }
}

/// A summary with no markup a server could read: the spec makes it plain
/// text, but some servers (dunst with `markup = full`) format it with the
/// body, so a sender's `<a href=...>` or `<img src=...>` would become a link
/// or an image fetch. Angle brackets are shown as look-alikes instead.
fn plain_summary(s: &str) -> String {
    s.replace('<', "\u{2039}").replace('>', "\u{203A}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(address: &str) -> Option<Target> {
        Some(Target {
            address: address.into(),
            id: Some("1".into()),
        })
    }

    fn notifier(entries: &[(u32, &str, &str)]) -> Arc<Notifier> {
        let n = Notifier::new(|_, _| {});
        let mut shown = n.shown.lock().unwrap();
        for (id, server, address) in entries {
            remember(
                &mut shown,
                Shown {
                    id: *id,
                    server: (*server).into(),
                    target: target(address),
                    token: None,
                },
            );
        }
        drop(shown);
        n
    }

    #[test]
    fn notify_sends_the_apps_name_icon_and_a_click_action() {
        let r = request("Code 482913 from GitHub", "Sign in");
        assert_eq!(
            r,
            Request {
                app_name: "tmpmail",
                icon: "tmpmail-gui",
                summary: "Code 482913 from GitHub".into(),
                body: "Sign in".into(),
                actions: vec!["default", "Open"],
                hints: vec![("desktop-entry", "tmpmail")],
                timeout: -1,
            }
        );
    }

    #[test]
    fn an_appimage_names_its_logo_by_a_file_uri_outside_its_mount() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my cache").join("tmpmail-gui.png");
        let uri = logo_file(&path).unwrap();
        assert!(
            uri.starts_with("file:///") && uri.ends_with("/my%20cache/tmpmail-gui.png"),
            "{uri}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), LOGO);
        // A changed or broken file is put right.
        std::fs::write(&path, "old").unwrap();
        assert_eq!(logo_file(&path).unwrap(), uri);
        assert_eq!(std::fs::read(&path).unwrap(), LOGO);
        // Not an AppImage here (a test binary isn't under any `$APPDIR`).
        assert!(!in_appimage());
    }

    #[test]
    fn a_click_carries_the_token_sent_just_before_it() {
        let n = notifier(&[(7, ":1.5", "a@x.com"), (8, ":1.5", "b@x.com")]);
        assert!(
            n.handle(":1.5", Signal::Token(7, "kwin-42".into()))
                .is_none()
        );
        let (t, token) = n
            .handle(":1.5", Signal::Action(7, "default".into()))
            .unwrap();
        assert_eq!(t.unwrap().address, "a@x.com");
        assert_eq!(token.as_deref(), Some("kwin-42"));
        // Used once: a second click (an older server) has none.
        let (_, token) = n
            .handle(":1.5", Signal::Action(7, "default".into()))
            .unwrap();
        assert_eq!(token, None);
        // Another notification's click doesn't take it.
        n.handle(":1.5", Signal::Token(7, "kwin-43".into()));
        let (t, token) = n
            .handle(":1.5", Signal::Action(8, "default".into()))
            .unwrap();
        assert_eq!((t.unwrap().address.as_str(), token), ("b@x.com", None));
        // Buttons (none are offered) and closed notifications do nothing.
        assert!(
            n.handle(":1.5", Signal::Action(7, "other".into()))
                .is_none()
        );
        assert!(n.handle(":1.5", Signal::Closed(7)).is_none());
        assert!(
            n.handle(":1.5", Signal::Action(7, "default".into()))
                .is_none()
        );
    }

    #[test]
    fn only_the_server_that_showed_it_counts() {
        let n = notifier(&[(7, ":1.5", "a@x.com")]);
        assert!(
            n.handle(":1.9", Signal::Token(7, "forged".into()))
                .is_none()
        );
        assert!(
            n.handle(":1.9", Signal::Action(7, "default".into()))
                .is_none()
        );
        assert!(n.handle(":1.9", Signal::Closed(7)).is_none());
        let (_, token) = n
            .handle(":1.5", Signal::Action(7, "default".into()))
            .unwrap();
        assert_eq!(token, None);
        // Unknown ids too.
        assert!(
            n.handle(":1.5", Signal::Action(99, "default".into()))
                .is_none()
        );
    }

    #[test]
    fn tokens_are_short_printable_ascii() {
        assert_eq!(clean_token("kwin-42").as_deref(), Some("kwin-42"));
        let x11 = "myhost;1234;5678;0_TIME123456";
        assert_eq!(clean_token(x11).as_deref(), Some(x11));
        for bad in ["", "a\nb", "a\u{0}b", "token-é", "a\u{7f}", "\u{1b}[2J"] {
            assert_eq!(clean_token(bad), None, "{bad:?}");
        }
        assert!(clean_token(&"x".repeat(TOKEN_MAX)).is_some());
        assert!(clean_token(&"x".repeat(TOKEN_MAX + 1)).is_none());
        // A rejected token isn't passed on, and the click still counts.
        let n = notifier(&[(7, ":1.5", "a@x.com")]);
        n.handle(":1.5", Signal::Token(7, "bad\ntoken".into()));
        let (t, token) = n
            .handle(":1.5", Signal::Action(7, "default".into()))
            .unwrap();
        assert!(t.is_some() && token.is_none());
    }

    #[test]
    fn the_x11_time_is_the_number_after_the_last_time_mark() {
        assert_eq!(startup_time("myhost;1234;5678;0_TIME123456"), Some(123_456));
        assert_eq!(startup_time("_TIME42"), Some(42));
        assert_eq!(startup_time("a_TIME1_TIME7x"), Some(7));
        assert_eq!(startup_time("kwin-42"), None);
        assert_eq!(startup_time("_TIME"), None);
        assert_eq!(startup_time("_TIME0"), None);
        assert_eq!(startup_time("_TIME99999999999"), None);
    }

    #[test]
    fn old_notifications_are_forgotten_in_the_order_shown() {
        let mut shown = VecDeque::new();
        for id in (1..=KEEP as u32 + 2).chain([1]) {
            let entry = Shown {
                id,
                server: ":1.5".into(),
                target: None,
                token: None,
            };
            remember(&mut shown, entry);
        }
        assert_eq!(shown.len(), KEEP);
        assert_eq!(shown.front().map(|s| s.id), Some(4));
        // The newest is kept even though its id is the smallest.
        assert_eq!(shown.back().map(|s| s.id), Some(1));
    }

    #[test]
    fn summaries_carry_no_markup() {
        // Security review C5.
        assert_eq!(
            plain_summary("<a href=\"https://x\">GitHub</a> <img src=x>"),
            "\u{2039}a href=\"https://x\"\u{203A}GitHub\u{2039}/a\u{203A} \u{2039}img src=x\u{203A}"
        );
        assert_eq!(plain_summary("Zo\u{eb} Smith"), "Zo\u{eb} Smith");
    }
}
