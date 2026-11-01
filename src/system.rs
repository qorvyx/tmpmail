//! Talking to the desktop: clipboard, browser, notifications.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::{Context, anyhow, bail};

fn on_path(bin: &str) -> bool {
    // Windows runs `less` as `less.exe` (or another `PATHEXT` extension).
    let exts: Vec<String> = if cfg!(windows) {
        path_exts(std::env::var("PATHEXT").ok().as_deref())
    } else {
        Vec::new()
    };
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| in_dir(&dir, bin, &exts)))
}

/// The extensions to try after the bare name, from `PATHEXT`
/// (default `.COM;.EXE;.BAT;.CMD`).
fn path_exts(pathext: Option<&str>) -> Vec<String> {
    pathext
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `dir` holds `bin` as is or with one of `exts` appended.
fn in_dir(dir: &Path, bin: &str, exts: &[String]) -> bool {
    dir.join(bin).is_file()
        || exts
            .iter()
            .any(|ext| dir.join(format!("{bin}{ext}")).is_file())
}

fn split_cmd(cmd: &str) -> anyhow::Result<(String, Vec<String>)> {
    let mut parts = cmd.split_whitespace().map(str::to_string);
    let prog = parts.next().ok_or_else(|| anyhow!("empty command"))?;
    Ok((prog, parts.collect()))
}

/// Picks a clipboard command for this session: Wayland, X11, macOS, WSL.
pub fn clipboard_command(custom: Option<&str>) -> anyhow::Result<String> {
    if let Some(cmd) = custom {
        return Ok(cmd.to_string());
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    let candidates: &[(&str, &str, bool)] = &[
        ("wl-copy", "wl-copy", wayland),
        ("xclip", "xclip -selection clipboard", x11),
        ("xsel", "xsel --clipboard --input", x11),
        ("pbcopy", "pbcopy", true),
        ("clip.exe", "clip.exe", true),
        ("termux-clipboard-set", "termux-clipboard-set", true),
    ];
    candidates
        .iter()
        .find(|(bin, _, usable)| *usable && on_path(bin))
        .map(|(_, cmd, _)| cmd.to_string())
        .ok_or_else(|| {
            anyhow!("no clipboard tool found (wl-copy, xclip, xsel, pbcopy, clip.exe, termux-clipboard-set); install one or pass --clipboard-cmd")
        })
}

pub fn copy(text: &str, custom: Option<&str>) -> anyhow::Result<()> {
    let cmd = clipboard_command(custom)?;
    let (prog, args) = split_cmd(&cmd)?;
    let mut child = spawn(
        Command::new(&prog)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .with_context(|| format!("cannot run {prog}"))?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(text.as_bytes())?;
    // wl-copy and xclip fork to serve the selection; the parent exits quickly.
    let status = child.wait()?;
    if !status.success() {
        bail!("{prog} exited with {status}");
    }
    Ok(())
}

/// Opens a link from a mail in the browser: only a real `http(s)` URL
/// ([`crate::extract::is_web_link`]), never anything an opener could read
/// as a file path or an option, whatever the caller's list holds.
pub fn open_link(url: &str, browser: Option<&str>) -> anyhow::Result<()> {
    if !crate::extract::is_web_link(url) {
        bail!(
            "refusing to open {:?}: not an http or https link",
            crate::render::clean_line(url)
        );
    }
    open(url, browser)
}

/// Opens a file or URL with the configured browser, `$BROWSER`, or the
/// platform opener.
pub fn open(target: &str, browser: Option<&str>) -> anyhow::Result<()> {
    #[cfg(windows)]
    if chosen_browser(browser).is_none() {
        return windows_open(target);
    }
    let cmd = browser_cmd(browser)
        .ok_or_else(|| anyhow!("no browser found; set $BROWSER or pass --browser"))?;
    let (prog, args) = split_cmd(&cmd)?;
    // Text browsers (w3m, lynx) need the terminal, so wait for them.
    let interactive = matches!(
        prog.as_str(),
        "w3m" | "lynx" | "links" | "elinks" | "browsh"
    );
    let mut command = Command::new(&prog);
    command.args(&args).arg(target);
    if interactive {
        spawn(&mut command)
            .and_then(|mut child| child.wait())
            .with_context(|| format!("cannot run {prog}"))?;
    } else {
        spawn(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .with_context(|| format!("cannot run {prog}"))?;
    }
    Ok(())
}

/// The browser the user chose: `browser` (`--browser`, the config), else
/// `$BROWSER`.
fn chosen_browser(browser: Option<&str>) -> Option<String> {
    browser
        .map(str::to_string)
        .or_else(|| std::env::var("BROWSER").ok().filter(|b| !b.is_empty()))
}

/// The command [`open`] runs: the chosen browser, else the platform opener
/// on `PATH`.
fn browser_cmd(browser: Option<&str>) -> Option<String> {
    chosen_browser(browser).or_else(|| {
        ["xdg-open", "open", "wslview"]
            .into_iter()
            .find(|b| on_path(b))
            .map(str::to_string)
    })
}

/// Explorer hands the target to the shell's default handler, so it opens
/// URLs in the default browser and files with their default app. Unlike
/// `cmd /C start`, no shell parses the target, so `&`, `|`, `^` and `%`
/// in a mail's link are passed through as is.
#[cfg(windows)]
fn windows_open(target: &str) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new("explorer.exe");
    command
        .raw_arg(explorer_arg(target)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn(&mut command).context("cannot run explorer.exe")?;
    // Explorer exits with 1 even when it worked, so the status means nothing.
    Ok(())
}

/// Starts `command`. On Windows a child gets every inheritable handle of
/// this process, not only the three it is given, so a browser or Explorer
/// would keep a file that `read --html > out` writes to open (Explorer for a
/// moment, a browser for its whole session) and the next write to it fails.
/// There, this process's own standard handles are made non-inheritable while
/// the child starts; the child still gets its own standard handles, which
/// Rust passes as duplicates.
fn spawn(command: &mut Command) -> std::io::Result<std::process::Child> {
    #[cfg(windows)]
    {
        let _guard = NoInheritedStdHandles::new();
        command.spawn()
    }
    #[cfg(not(windows))]
    command.spawn()
}

/// Clears the inherit flag of this process's standard handles until
/// dropped (on unwind too). Holds a lock, so a spawn on another thread
/// can't put the flags back while this one starts its child.
#[cfg(windows)]
struct NoInheritedStdHandles {
    cleared: [windows_sys::Win32::Foundation::HANDLE; 3],
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(windows)]
impl NoInheritedStdHandles {
    fn new() -> Self {
        use windows_sys::Win32::Foundation::{
            GetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
        };
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cleared = [std::ptr::null_mut(); 3];
        for (slot, id) in
            cleared
                .iter_mut()
                .zip([STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE])
        {
            // SAFETY: reads and changes only the inherit flag of this
            // process's own standard handles; nothing is closed or replaced.
            unsafe {
                let handle = GetStdHandle(id);
                if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                    continue;
                }
                let mut flags = 0u32;
                if GetHandleInformation(handle, &mut flags) != 0
                    && flags & HANDLE_FLAG_INHERIT != 0
                    && SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0
                {
                    *slot = handle;
                }
            }
        }
        NoInheritedStdHandles {
            cleared,
            _lock: lock,
        }
    }
}

#[cfg(windows)]
impl Drop for NoInheritedStdHandles {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        for handle in self.cleared.into_iter().filter(|h| !h.is_null()) {
            // SAFETY: puts back the flag `new` cleared on this handle.
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
            }
        }
    }
}

/// Whether stdout shows ANSI colors. A Windows console only does once
/// virtual terminal processing is on (the classic conhost has it off): it is
/// turned on here, and colors stay off when that fails. A terminal that
/// isn't a console (mintty's pipes) passes the codes through as is.
#[cfg(windows)]
pub fn ansi_colors() -> bool {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        // SAFETY: console mode calls on this process's stdout handle.
        unsafe {
            let out = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0u32;
            if GetConsoleMode(out, &mut mode) == 0 {
                return true;
            }
            mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
                || SetConsoleMode(out, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
        }
    })
}

/// Terminals elsewhere take ANSI colors as they are.
#[cfg(not(windows))]
pub fn ansi_colors() -> bool {
    true
}

/// Explorer's argument, always quoted so commas and spaces stay part of the
/// target. A quote can't be escaped there; URLs encode it as `%22` and
/// Windows paths can't contain one, so it is refused.
#[cfg(any(windows, test))]
fn explorer_arg(target: &str) -> anyhow::Result<String> {
    if target.contains('"') || target.chars().any(char::is_control) {
        bail!("cannot open {target:?}: it contains a quote or control character");
    }
    Ok(format!("\"{target}\""))
}

/// Columns and rows of the terminal on stdout, else `$COLUMNS` and `$LINES`
/// (rows 0 when unknown).
#[cfg(feature = "cli")]
pub fn term_size() -> Option<(usize, usize)> {
    use std::io::IsTerminal;
    // crossterm asks the terminal itself (/dev/tty, or the Windows console),
    // so only trust it when stdout is that terminal.
    if std::io::stdout().is_terminal() {
        if let Ok((cols, rows)) = ratatui::crossterm::terminal::size() {
            if cols > 0 {
                return Some((cols as usize, rows as usize));
            }
        }
    }
    let env = |key: &str| std::env::var(key).ok()?.parse::<usize>().ok();
    Some((env("COLUMNS")?, env("LINES").unwrap_or(0)))
}

/// Pipes `text` into a pager: `pager`, else `$PAGER`, else `less`. Like git,
/// sets `LESS=FRX` when unset so colors work and short text doesn't page.
/// Windows rarely has `less`, so there the text is printed when it's missing.
pub fn page(text: &str, pager: Option<&str>) -> anyhow::Result<()> {
    let cmd = pager
        .map(str::to_string)
        .or_else(|| std::env::var("PAGER").ok())
        .filter(|p| !p.trim().is_empty());
    if cmd.is_none() && cfg!(windows) && !on_path("less") {
        print!("{text}");
        return Ok(());
    }
    let cmd = cmd.unwrap_or_else(|| "less".into());
    let (prog, args) = split_cmd(&cmd)?;
    let mut command = Command::new(&prog);
    command.args(&args).stdin(Stdio::piped());
    if std::env::var_os("LESS").is_none() {
        command.env("LESS", "FRX");
    }
    let mut child = spawn(&mut command).with_context(|| format!("cannot run {prog}"))?;
    let written = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(text.as_bytes());
    child.wait()?;
    match written {
        // Quitting the pager before the end closes the pipe; that's fine.
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

pub fn notify(title: &str, body: &str) {
    if on_path("notify-send") {
        // The summary is plain text by the spec; only the body is markup.
        let body = notify_body(body, server_reads_markup());
        let _ = Command::new("notify-send")
            .args(["--app-name=tmpmail", "--", title, &body])
            .status();
    } else if on_path("osascript") {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "display notification \"{}\" with title \"{}\"",
                esc(body),
                esc(title)
            ))
            .status();
    } else {
        // Don't wait: PowerShell takes a second or so to start.
        #[cfg(windows)]
        let _ = spawn(
            windows_notify_command(title, body)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        );
    }
}

/// The body as the notification server should get it: escaped when the
/// server reads markup (most Linux ones), so a subject like `<b>x</b>` or a
/// `Name <addr>` sender is shown as typed, never rendered.
fn notify_body(body: &str, markup: bool) -> String {
    if markup {
        escape_markup(body)
    } else {
        body.to_string()
    }
}

/// `&`, `<` and `>` as entities. Quotes stay: they need no escape in the
/// body, and some servers would show `&apos;` as is (as the desktop app).
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Whether the session's notification server reads the body as markup
/// (its `body-markup` capability), asked through `gdbus`, else `dbus-send`.
/// An answer is kept for the rest of the run. Without one, markup is
/// assumed, as the desktop app does: the common servers (GNOME Shell, KDE
/// Plasma) read it, and showing `&amp;` on a plain-text server is better
/// than letting a sender's tags through.
fn server_reads_markup() -> bool {
    static MARKUP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    const QUERIES: &[(&str, &[&str])] = &[
        (
            "gdbus",
            &[
                "call",
                "--session",
                "--timeout",
                "2",
                "--dest",
                "org.freedesktop.Notifications",
                "--object-path",
                "/org/freedesktop/Notifications",
                "--method",
                "org.freedesktop.Notifications.GetCapabilities",
            ],
        ),
        (
            "dbus-send",
            &[
                "--session",
                "--print-reply",
                "--reply-timeout=2000",
                "--dest=org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications.GetCapabilities",
            ],
        ),
    ];
    if let Some(&known) = MARKUP.get() {
        return known;
    }
    // Only the first tool found: when it gets no answer, the bus has none.
    let answer = QUERIES
        .iter()
        .find(|(bin, _)| on_path(bin))
        .and_then(|(bin, args)| {
            Command::new(bin)
                .args(*args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
        })
        .map(|o| lists_body_markup(&String::from_utf8_lossy(&o.stdout)));
    match answer {
        Some(markup) => *MARKUP.get_or_init(|| markup),
        // Asked again next time: the server may not have started yet.
        None => true,
    }
}

/// Whether a `GetCapabilities` reply, as `gdbus` (`(['body', 'body-markup'],)`)
/// or `dbus-send` (`string "body-markup"`) prints it, lists `body-markup`.
fn lists_body_markup(reply: &str) -> bool {
    reply.contains("'body-markup'") || reply.contains("\"body-markup\"")
}

/// A toast through Windows PowerShell's WinRT bridge, shown under
/// PowerShell's app id since tmpmail has none registered. Mail subjects are
/// attacker-controlled, so the title and body travel in environment
/// variables and become XML text nodes: the script itself never changes.
#[cfg(any(windows, test))]
fn windows_notify_command(title: &str, body: &str) -> Command {
    const SCRIPT: &str = "$ErrorActionPreference = 'Stop'; \
        $null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]; \
        $m = [Windows.UI.Notifications.ToastNotificationManager]; \
        $xml = $m::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
        $text = $xml.GetElementsByTagName('text'); \
        $null = $text.Item(0).AppendChild($xml.CreateTextNode($env:TMPMAIL_NOTIFY_TITLE)); \
        $null = $text.Item(1).AppendChild($xml.CreateTextNode($env:TMPMAIL_NOTIFY_BODY)); \
        $app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe'; \
        $m::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($xml))";
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("TMPMAIL_NOTIFY_TITLE", title)
        .env("TMPMAIL_NOTIFY_BODY", body)
        .stdin(Stdio::null());
    cmd
}

/// How long a copy written for the browser is kept. The browser reads it
/// right away, but may reload it, so it isn't deleted at once.
const VIEW_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// The name of the folder a snap browser's view files go in, inside the
/// snap's own `~/snap/NAME/common`.
#[cfg(target_os = "linux")]
const SNAP_VIEWS: &str = "tmpmail-view";

/// The start of every view file's name. In a snap's folder only files
/// named so, and the first builds' copies ([`is_snap_view`]), are ever
/// deleted.
const VIEW_PREFIX: &str = "tmpmail-";

/// How every view file tmpmail writes starts
/// ([`crate::render::safe_html_document`]).
#[cfg(target_os = "linux")]
const VIEW_HEAD: &str = "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n\
     <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src ";

/// Whether a file in the snap view folder `dir` is a copy tmpmail wrote:
/// named `tmpmail-*.html`, or as the first builds that wrote there named
/// them, a message id and `.html` (letters, digits, `-` and `_`, with a
/// digit) that starts as tmpmail's copies do, so a user's `report-2024.html`
/// stays.
#[cfg(target_os = "linux")]
fn is_snap_view(dir: &ViewDir, name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".html") else {
        return false;
    };
    if name.starts_with(VIEW_PREFIX) {
        return true;
    }
    let id_shaped = !stem.is_empty()
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && stem.bytes().any(|b| b.is_ascii_digit());
    id_shaped && dir.starts_with(name, VIEW_HEAD.as_bytes())
}

/// Whether a file in the state folder's `view` is a copy tmpmail wrote:
/// everything there is, and versions before 0.3.5 named them `ID.html`.
fn is_state_view(name: &str) -> bool {
    name.ends_with(".html")
}

/// A view file written for the browser, and why it isn't where the
/// browser would read it best, if so.
pub struct View {
    pub path: PathBuf,
    pub note: Option<String>,
}

/// Writes a viewer file for `browser` (as [`open`] runs it), readable only
/// by the user, and returns its path, with a note to show the user when it
/// isn't where a snap browser reads it best. It goes to `view` in the state
/// folder, except for a browser installed as a snap (see [`write_view_in`]).
/// Copies left over from earlier views are deleted once they are a day old,
/// in the state folder and in the folder written to.
pub fn write_view(
    state: &Path,
    browser: Option<&str>,
    name: &str,
    contents: &str,
) -> crate::error::Result<View> {
    #[cfg(target_os = "linux")]
    let snap = snap_browser(browser_cmd(browser).as_deref(), &Snapd::system());
    #[cfg(not(target_os = "linux"))]
    let snap: Option<String> = {
        let _ = browser;
        None
    };
    write_view_in(
        state,
        dirs::home_dir().as_deref(),
        snap.as_deref(),
        name,
        contents,
    )
}

/// [`write_view`] for a snap browser named `snap` (or none), with `home` as
/// the home folder. Snap confinement keeps a snap out of hidden folders in
/// home (so `~/.local/state`), it has its own `/tmp`, and sees only its own
/// part of `$XDG_RUNTIME_DIR`; its `~/snap/NAME/common` it may always read,
/// so a snap browser's copy goes to `~/snap/NAME/common/tmpmail-view`.
/// (GNOME hands a file to a snap through the document portal, so the state
/// folder worked there; KDE doesn't.)
///
/// The snap may write in `~/snap/NAME`, and swap any folder there for a
/// link at any time. So that folder is reached through open folders, never
/// by path: `~/snap` is opened (it may be the user's own link: no snap can
/// write in it), then each of `NAME`, `common` and `tmpmail-view` inside
/// the one before it, refusing a link, and the file is written, listed and
/// deleted inside the open `tmpmail-view` folder, so a link put there later
/// leads nowhere. If any of them is a link (or can't be opened or written
/// in), the copy goes to the state folder instead, with a note saying why.
fn write_view_in(
    state: &Path,
    home: Option<&Path>,
    snap: Option<&str>,
    name: &str,
    contents: &str,
) -> crate::error::Result<View> {
    let name = format!("{VIEW_PREFIX}{name}");
    #[cfg(target_os = "linux")]
    let note = match (home, snap) {
        (Some(home), Some(snap)) => {
            let snaps = home.join("snap");
            let tried = ViewDir::open(&snaps, &[snap, "common", SNAP_VIEWS]).and_then(|dir| {
                let path = dir.write(&name, contents)?;
                dir.prune(VIEW_MAX_AGE, |n| is_snap_view(&dir, n));
                Ok(path)
            });
            match tried {
                Ok(path) => {
                    prune_state_views(state);
                    prune_other_snaps(&snaps, Some(snap));
                    return Ok(View { path, note: None });
                }
                Err(e) => Some(format!(
                    "{e}; the copy is in the state folder instead, which the {snap} snap may \
                     not be able to read"
                )),
            }
        }
        _ => None,
    };
    // Copies an earlier browser (another snap) was given.
    #[cfg(target_os = "linux")]
    if let Some(home) = home {
        prune_other_snaps(&home.join("snap"), None);
    }
    #[cfg(not(target_os = "linux"))]
    let note = {
        let _ = (home, snap);
        None
    };
    std::fs::create_dir_all(state).map_err(|e| anyhow!("create {}: {e}", state.display()))?;
    let views = state.join("view");
    let dir =
        ViewDir::open(state, &["view"]).map_err(|e| anyhow!("open {}: {e}", views.display()))?;
    let path = dir.write(&name, contents).map_err(|e| anyhow!("{}", e))?;
    dir.prune(VIEW_MAX_AGE, is_state_view);
    Ok(View { path, note })
}

/// Prunes the state folder's view files, if it has any.
#[cfg(target_os = "linux")]
fn prune_state_views(state: &Path) {
    if let Ok(dir) = ViewDir::open_existing(state, &["view"]) {
        dir.prune(VIEW_MAX_AGE, is_state_view);
    }
}

/// Prunes the view files in every snap's `common/tmpmail-view` but
/// `except`'s, reached as [`write_view_in`] reaches its own: through open
/// folders from `snaps` (`~/snap`), refusing links, never creating any.
#[cfg(target_os = "linux")]
fn prune_other_snaps(snaps: &Path, except: Option<&str>) {
    let Ok(dir) = ViewDir::open_existing(snaps, &[]) else {
        return;
    };
    for name in dir.names() {
        if Some(name.as_str()) == except {
            continue;
        }
        if let Ok(views) = dir.open_below(&[&name, "common", SNAP_VIEWS]) {
            views.prune(VIEW_MAX_AGE, |n| is_snap_view(&views, n));
        }
    }
}

/// A folder view files go in, held open (on Unix) so that it can't be
/// swapped for a link while it is used.
struct ViewDir {
    path: PathBuf,
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

impl ViewDir {
    /// Opens `base` (the user's own home or state folder, followed if it is
    /// a link) and then each of `below` inside the one before, refusing a
    /// link. The last is created if missing and made 0700.
    fn open(base: &Path, below: &[&str]) -> std::io::Result<ViewDir> {
        Self::open_at(base, below, true)
    }

    /// [`ViewDir::open`] without creating anything or changing its mode.
    #[cfg(target_os = "linux")]
    fn open_existing(base: &Path, below: &[&str]) -> std::io::Result<ViewDir> {
        Self::open_at(base, below, false)
    }

    #[cfg(unix)]
    fn open_at(base: &Path, below: &[&str], create: bool) -> std::io::Result<ViewDir> {
        use rustix::fs::{CWD, Mode, OFlags, openat};
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let fd =
            openat(CWD, base, flags, Mode::empty()).map_err(|e| Self::failed("open", base, e))?;
        Self::walk(fd, base.to_path_buf(), below, create)
    }

    #[cfg(unix)]
    fn failed(what: &str, path: &Path, e: rustix::io::Errno) -> std::io::Error {
        std::io::Error::new(
            std::io::Error::from(e).kind(),
            format!("{what} {}: {}", path.display(), std::io::Error::from(e)),
        )
    }

    /// Opens each of `below` inside the one before, from the open folder
    /// `fd` (at `path`), refusing a link. With `create` the last is created
    /// if missing and made 0700.
    #[cfg(unix)]
    fn walk(
        mut fd: std::os::fd::OwnedFd,
        mut path: PathBuf,
        below: &[&str],
        create: bool,
    ) -> std::io::Result<ViewDir> {
        use rustix::fs::{Mode, OFlags, fchmod, mkdirat, openat};
        let failed = Self::failed;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        for (i, name) in below.iter().enumerate() {
            path.push(name);
            let last = i + 1 == below.len();
            if last && create {
                match mkdirat(&fd, *name, Mode::RWXU) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(e) => return Err(failed("create", &path, e)),
                }
            }
            fd = openat(&fd, *name, flags | OFlags::NOFOLLOW, Mode::empty()).map_err(|e| {
                if e == rustix::io::Errno::LOOP || e == rustix::io::Errno::NOTDIR {
                    std::io::Error::other(format!(
                        "{} is a symbolic link or not a folder",
                        path.display()
                    ))
                } else {
                    failed("open", &path, e)
                }
            })?;
        }
        if create {
            fchmod(&fd, Mode::RWXU).map_err(|e| failed("chmod", &path, e))?;
        }
        Ok(ViewDir { path, fd })
    }

    #[cfg(not(unix))]
    fn open_at(base: &Path, below: &[&str], create: bool) -> std::io::Result<ViewDir> {
        let mut path = base.to_path_buf();
        for name in below {
            path.push(name);
        }
        if create {
            std::fs::create_dir_all(&path)?;
        }
        Ok(ViewDir { path })
    }

    /// The names in the folder (`.` and `..` left out).
    #[cfg(target_os = "linux")]
    fn names(&self) -> Vec<String> {
        let Ok(mut entries) = rustix::fs::Dir::read_from(&self.fd) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        while let Some(Ok(entry)) = entries.read() {
            if let Ok(name) = entry.file_name().to_str()
                && name != "."
                && name != ".."
            {
                names.push(name.to_string());
            }
        }
        names
    }

    /// Opens each of `below` inside the one before, from this folder,
    /// refusing a link and creating nothing.
    #[cfg(target_os = "linux")]
    fn open_below(&self, below: &[&str]) -> std::io::Result<ViewDir> {
        Self::walk(rustix::io::dup(&self.fd)?, self.path.clone(), below, false)
    }

    /// Whether the regular file `name` here starts with `head` (read
    /// without following a link).
    #[cfg(target_os = "linux")]
    fn starts_with(&self, name: &str, head: &[u8]) -> bool {
        use rustix::fs::{Mode, OFlags, openat};
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let Ok(fd) = openat(&self.fd, name, flags, Mode::empty()) else {
            return false;
        };
        let mut f = std::fs::File::from(fd);
        if !f.metadata().is_ok_and(|m| m.is_file()) {
            return false;
        }
        let mut got = Vec::with_capacity(head.len());
        std::io::Read::read_to_end(
            &mut std::io::Read::take(&mut f, head.len() as u64),
            &mut got,
        )
        .is_ok()
            && got == head
    }

    /// Writes `name` new, readable only by the user, and returns its path.
    /// A copy of the same message, or anything put in its place, goes
    /// first; the file is then created new, which never follows a link.
    fn write(&self, name: &str, contents: &str) -> std::io::Result<PathBuf> {
        let path = self.path.join(name);
        let failed = |what: &str, e: std::io::Error| {
            std::io::Error::new(e.kind(), format!("{what} {}: {e}", path.display()))
        };
        let mut f = self.create_new(name).map_err(|e| failed("create", e))?;
        f.write_all(contents.as_bytes())
            .map_err(|e| failed("write", e))?;
        Ok(path)
    }

    #[cfg(unix)]
    fn create_new(&self, name: &str) -> std::io::Result<std::fs::File> {
        use rustix::fs::{AtFlags, Mode, OFlags, openat, unlinkat};
        match unlinkat(&self.fd, name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => {}
            Err(e) => return Err(e.into()),
        }
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = openat(&self.fd, name, flags, Mode::RUSR | Mode::WUSR)?;
        Ok(std::fs::File::from(fd))
    }

    #[cfg(not(unix))]
    fn create_new(&self, name: &str) -> std::io::Result<std::fs::File> {
        let path = self.path.join(name);
        if std::fs::symlink_metadata(&path).is_ok() {
            std::fs::remove_file(&path)?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
    }

    /// Deletes the regular files older than `max_age` whose names `ours`
    /// accepts. Best effort: a file that can't be inspected or removed is
    /// left alone.
    #[cfg(unix)]
    fn prune(&self, max_age: Duration, ours: impl Fn(&str) -> bool) {
        use rustix::fs::{AtFlags, Dir, FileType, statat, unlinkat};
        let Ok(mut entries) = Dir::read_from(&self.fd) else {
            return;
        };
        let now = SystemTime::now();
        while let Some(Ok(entry)) = entries.read() {
            let name = entry.file_name();
            if !name.to_str().is_ok_and(&ours) {
                continue;
            }
            let Ok(st) = statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) else {
                continue;
            };
            // A time before 1970 (only someone else sets one) counts as old.
            let old = u64::try_from(st.st_mtime)
                .ok()
                .and_then(|secs| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs)))
                .is_none_or(|modified| now.duration_since(modified).is_ok_and(|age| age > max_age));
            if FileType::from_raw_mode(st.st_mode) == FileType::RegularFile && old {
                let _ = unlinkat(&self.fd, name, AtFlags::empty());
            }
        }
    }

    #[cfg(not(unix))]
    fn prune(&self, max_age: Duration, ours: impl Fn(&str) -> bool) {
        let Ok(entries) = std::fs::read_dir(&self.path) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            if !entry.file_name().to_str().is_some_and(&ours) {
                continue;
            }
            let old = entry
                .metadata()
                .ok()
                .filter(|m| m.is_file())
                .and_then(|m| m.modified().ok())
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > max_age);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Where snapd puts things. Paths, so tests can point elsewhere.
#[cfg(target_os = "linux")]
struct Snapd {
    /// The folders of the snaps' commands (`/snap/bin/firefox`; Fedora and
    /// Arch use `/var/lib/snapd/snap/bin`).
    bins: Vec<PathBuf>,
    /// What those commands are links to.
    launcher: PathBuf,
    /// The snaps' desktop files (`firefox_firefox.desktop`).
    desktop: PathBuf,
}

#[cfg(target_os = "linux")]
impl Snapd {
    fn system() -> Snapd {
        Snapd {
            bins: vec!["/snap/bin".into(), "/var/lib/snapd/snap/bin".into()],
            launcher: "/usr/bin/snap".into(),
            desktop: "/var/lib/snapd/desktop/applications".into(),
        }
    }
}

/// The openers that hand a file to the desktop's default browser.
#[cfg(target_os = "linux")]
const OPENERS: [&str; 7] = [
    "xdg-open",
    "kde-open",
    "kde-open5",
    "kde-open6",
    "gio",
    "exo-open",
    "gnome-open",
];

/// The snap's name if `cmd` (as [`browser_cmd`] gives it) opens view files
/// in a browser installed as a snap: the command itself, or for an opener
/// such as `xdg-open` the desktop's default for HTML files (`xdg-mime query
/// default text/html`, what `xdg-open` goes by).
#[cfg(target_os = "linux")]
fn snap_browser(cmd: Option<&str>, snapd: &Snapd) -> Option<String> {
    let (prog, _) = split_cmd(cmd?).ok()?;
    let base = Path::new(&prog).file_name()?.to_str()?;
    if !OPENERS.contains(&base) {
        return snap_of_program(&prog, snapd);
    }
    let out = Command::new("xdg-mime")
        .args(["query", "default", "text/html"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    snap_of_desktop_id(String::from_utf8_lossy(&out.stdout).trim(), snapd)
}

/// A snap's name from one of its commands' names (`firefox`, or
/// `chromium.chromedriver` for an app of the `chromium` snap): lower-case
/// letters, digits and dashes.
#[cfg(target_os = "linux")]
fn snap_name(command: &str) -> Option<String> {
    let s = command.split('.').next()?;
    let ok = !s.is_empty()
        && s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| s.to_string())
}

/// The snap a browser command runs: a command in a snap `bin` folder, a
/// link to one (or to the snap launcher, under the command's name), or a
/// small script that `exec`s one (Ubuntu's `/usr/bin/firefox` is
/// `exec /snap/bin/firefox "$@"`).
#[cfg(target_os = "linux")]
fn snap_of_program(prog: &str, snapd: &Snapd) -> Option<String> {
    let mut path = if prog.contains('/') {
        PathBuf::from(prog)
    } else {
        // As `Command` finds it: the first executable file on PATH.
        let paths = std::env::var_os("PATH")?;
        std::env::split_paths(&paths)
            .map(|d| d.join(prog))
            .find(|p| executable(p))?
    };
    let in_snap_bin = |p: &Path| {
        p.parent()
            .is_some_and(|d| snapd.bins.iter().any(|b| b == d))
    };
    // A few links deep at most.
    for _ in 0..8 {
        if in_snap_bin(&path) {
            return snap_name(path.file_name()?.to_str()?);
        }
        let Ok(target) = std::fs::read_link(&path) else {
            break;
        };
        let target = path.parent().map_or(target.clone(), |d| d.join(&target));
        if target == snapd.launcher {
            return snap_name(path.file_name()?.to_str()?);
        }
        path = target;
    }
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 8192 {
        return None;
    }
    let text = std::fs::read(&path).ok()?;
    let text = String::from_utf8_lossy(&text);
    if !text.starts_with("#!") {
        return None;
    }
    // Only a script whose every `exec` runs the same snap command.
    let mut found: Option<String> = None;
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("exec ") else {
            continue;
        };
        let target = Path::new(rest.split_whitespace().next()?);
        let name = in_snap_bin(target)
            .then(|| snap_name(target.file_name()?.to_str()?))
            .flatten()?;
        if found.as_ref().is_some_and(|f| *f != name) {
            return None;
        }
        found = Some(name);
    }
    found
}

#[cfg(target_os = "linux")]
fn executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The snap a desktop file belongs to: snapd names them `SNAP_APP.desktop`
/// and keeps them in its own folder.
#[cfg(target_os = "linux")]
fn snap_of_desktop_id(id: &str, snapd: &Snapd) -> Option<String> {
    let (name, _) = id.split_once('_')?;
    if !id.ends_with(".desktop") || id.contains('/') {
        return None;
    }
    snapd
        .desktop
        .join(id)
        .is_file()
        .then(|| snap_name(name))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathext_lookup_finds_tools_without_their_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("less.EXE"), "").unwrap();
        std::fs::write(dir.path().join("clip.exe"), "").unwrap();
        let exts = path_exts(Some(".COM;.EXE; ;.BAT"));
        assert_eq!(exts, [".COM", ".EXE", ".BAT"]);
        assert!(in_dir(dir.path(), "less", &exts));
        assert!(in_dir(dir.path(), "clip.exe", &exts));
        assert!(!in_dir(dir.path(), "less", &[]));
        assert!(!in_dir(dir.path(), "more", &exts));
        assert_eq!(path_exts(None), [".COM", ".EXE", ".BAT", ".CMD"]);
        assert_eq!(path_exts(Some("")), path_exts(None));
    }

    #[test]
    fn explorer_arg_is_quoted_and_refuses_quotes() {
        let url = "https://x.com/v?a=1&b=%PATH%,2^|3";
        assert_eq!(explorer_arg(url).unwrap(), format!("\"{url}\""));
        let path = r"C:\Users\Jo Smith\AppData\Local\tmpmail\view\a.html";
        assert_eq!(explorer_arg(path).unwrap(), format!("\"{path}\""));
        assert!(explorer_arg("https://x.com/\" & calc").is_err());
        assert!(explorer_arg("https://x.com/\n").is_err());
    }

    #[test]
    fn windows_notify_keeps_mail_text_out_of_the_script() {
        let title = "tmpmail: a@x.com";
        let body = "'; Start-Process calc; $(calc) \"`";
        let cmd = windows_notify_command(title, body);
        assert!(
            cmd.get_args()
                .all(|a| !a.to_string_lossy().contains("calc"))
        );
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.contains(&("TMPMAIL_NOTIFY_TITLE".as_ref(), Some(title.as_ref()))));
        assert!(envs.contains(&("TMPMAIL_NOTIFY_BODY".as_ref(), Some(body.as_ref()))));
    }

    #[test]
    fn notification_bodies_are_escaped_for_markup_servers() {
        let body = "Hostile <b>x</b> & <a href=\"https://e.x\">y</a> 'q'";
        assert_eq!(
            notify_body(body, true),
            "Hostile &lt;b&gt;x&lt;/b&gt; &amp; &lt;a href=\"https://e.x\"&gt;y&lt;/a&gt; 'q'"
        );
        assert_eq!(notify_body(body, false), body);
        assert!(lists_body_markup("(['actions', 'body', 'body-markup'],)\n"));
        assert!(lists_body_markup(
            "   array [\n      string \"body-markup\"\n   ]"
        ));
        assert!(!lists_body_markup(
            "(['actions', 'body', 'body-hyperlinks'],)"
        ));
    }

    #[test]
    fn only_web_links_reach_the_opener() {
        // Security review B1: a "link" that is a relative path never reaches
        // the opener (here a browser command that would fail if it ran).
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let browser = format!("touch {}", marker.display());
        for bad in [
            "http/../../etc/hostname",
            "http\\..\\calc.exe",
            "httpx:y",
            "-x",
        ] {
            let err = open_link(bad, Some(&browser)).unwrap_err().to_string();
            assert!(err.contains("not an http or https link"), "{err}");
        }
        assert!(!marker.exists());
    }

    /// Sets a file's modification time a day and a minute back.
    fn age(path: &Path) {
        let day_ago = SystemTime::now() - VIEW_MAX_AGE - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(day_ago)
            .unwrap();
    }

    #[test]
    fn old_view_files_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let first = write_view_in(&state, None, None, "old.html", "x").unwrap();
        assert_eq!(first.path, state.join("view/tmpmail-old.html"));
        assert!(first.note.is_none());
        // A copy named as earlier versions named them goes too.
        let legacy = state.join("view/old.html");
        std::fs::write(&legacy, "x").unwrap();
        let keep = state.join("view/notes.txt");
        std::fs::write(&keep, "x").unwrap();
        for p in [&first.path, &legacy, &keep] {
            age(p);
        }
        let new = write_view_in(&state, None, None, "new.html", "y").unwrap();
        assert!(!first.path.exists() && !legacy.exists());
        assert!(new.path.exists() && keep.exists());
        // Writing the same message again replaces its copy.
        write_view_in(&state, None, None, "new.html", "z").unwrap();
        assert_eq!(std::fs::read_to_string(&new.path).unwrap(), "z");
    }

    #[test]
    #[cfg(unix)]
    fn view_files_are_private_and_never_follow_links() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let views = ViewDir::open(dir.path(), &["view"]).unwrap();
        let f = views.write("a.html", "x").unwrap();
        assert_eq!(mode(&dir.path().join("view")), 0o700);
        assert_eq!(mode(&f), 0o600);
        // A link put in place of a copy is replaced, its target untouched.
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        std::fs::remove_file(&f).unwrap();
        symlink(&victim, &f).unwrap();
        views.write("a.html", "new").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert!(
            !std::fs::symlink_metadata(&f)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        // A link put in place of the folder is refused, its target's mode
        // left alone.
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&elsewhere, dir.path().join("linked")).unwrap();
        let err = ViewDir::open(dir.path(), &["linked"]).err().unwrap();
        assert!(err.to_string().contains("symbolic link"), "{err}");
        assert_eq!(mode(&elsewhere), 0o755);
    }

    #[test]
    #[cfg(unix)]
    fn a_folder_swapped_for_a_link_after_opening_leads_nowhere() {
        // Security review A1, made deterministic: the snap swaps its folder
        // for a link to another folder right after tmpmail opened it (the
        // race the review won in a few rounds). Every write, listing and
        // delete still happens in the folder that was opened.
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let common = dir.path().join("snap/evil/common");
        std::fs::create_dir_all(common.join("tmpmail-view")).unwrap();
        let docs = dir.path().join("Documents");
        std::fs::create_dir(&docs).unwrap();
        // The user's files, named as tmpmail names its copies, and old.
        let theirs = docs.join("tmpmail-thesis.html");
        std::fs::write(&theirs, "my thesis").unwrap();
        age(&theirs);
        let decoy = common.join("tmpmail-view").join("tmpmail-thesis.html");
        std::fs::write(&decoy, "decoy").unwrap();
        age(&decoy);

        let views = ViewDir::open(dir.path(), &["snap", "evil", "common", "tmpmail-view"]).unwrap();
        let moved = common.join("moved");
        std::fs::rename(common.join("tmpmail-view"), &moved).unwrap();
        symlink(&docs, common.join("tmpmail-view")).unwrap();

        views.write("tmpmail-new.html", "mail").unwrap();
        views.prune(VIEW_MAX_AGE, |n| n.starts_with(VIEW_PREFIX));
        assert_eq!(std::fs::read_to_string(&theirs).unwrap(), "my thesis");
        assert!(!docs.join("tmpmail-new.html").exists());
        assert!(moved.join("tmpmail-new.html").exists());
        assert!(
            !moved.join("tmpmail-thesis.html").exists(),
            "the decoy went"
        );
    }

    #[cfg(target_os = "linux")]
    fn fake_snapd(dir: &Path) -> Snapd {
        let snapd = Snapd {
            bins: vec![dir.join("snap-bin"), dir.join("var-snap-bin")],
            launcher: dir.join("usr-bin-snap"),
            desktop: dir.join("apps"),
        };
        for d in snapd.bins.iter().chain([&snapd.desktop]) {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&snapd.launcher, "\x7fELF").unwrap();
        snapd
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snap_browsers_are_recognised() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let snapd = fake_snapd(dir.path());
        // Kubuntu's default browser: the snap's desktop file.
        std::fs::write(snapd.desktop.join("firefox_firefox.desktop"), "").unwrap();
        let desktop = |id| snap_of_desktop_id(id, &snapd);
        assert_eq!(
            desktop("firefox_firefox.desktop").as_deref(),
            Some("firefox")
        );
        assert_eq!(desktop("firefox.desktop"), None);
        assert_eq!(desktop("chromium_chromium.desktop"), None);
        assert_eq!(desktop(""), None);
        // `-b /snap/bin/firefox`, an app of a snap, Fedora's and Arch's
        // folder, and links to them or to the launcher.
        let prog = |p: &Path| snap_of_program(p.to_str().unwrap(), &snapd);
        let ff = snapd.bins[0].join("firefox");
        symlink(&snapd.launcher, &ff).unwrap();
        symlink(&snapd.launcher, snapd.bins[0].join("chromium.chromedriver")).unwrap();
        symlink(&snapd.launcher, snapd.bins[1].join("brave")).unwrap();
        assert_eq!(prog(&ff).as_deref(), Some("firefox"));
        assert_eq!(
            prog(&snapd.bins[0].join("chromium.chromedriver")).as_deref(),
            Some("chromium")
        );
        assert_eq!(prog(&snapd.bins[1].join("brave")).as_deref(), Some("brave"));
        let my_link = dir.path().join("ff");
        symlink(&ff, &my_link).unwrap();
        assert_eq!(prog(&my_link).as_deref(), Some("firefox"));
        // Ubuntu's `/usr/bin/firefox` wrapper.
        let wrapper = dir.path().join("firefox-wrapper");
        let script = format!("#!/bin/sh\nexec {} \"$@\"\n", ff.display());
        std::fs::write(&wrapper, script).unwrap();
        assert_eq!(prog(&wrapper).as_deref(), Some("firefox"));
        // A script that only may run a snap, or mentions one, isn't one.
        let fallback = dir.path().join("fallback");
        let script = format!(
            "#!/bin/sh\n# see {}\n[ -x {ff} ] && exec {ff} \"$@\"\nexec /usr/bin/chrome \"$@\"\n",
            snapd.bins[0].display(),
            ff = ff.display()
        );
        std::fs::write(&fallback, script).unwrap();
        assert_eq!(prog(&fallback), None);
        let plain = dir.path().join("w3m");
        std::fs::write(&plain, "\x7fELF").unwrap();
        assert_eq!(prog(&plain), None);
        assert_eq!(prog(Path::new("/nonexistent/firefox")), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn view_files_go_where_the_browser_can_read_them() {
        let dir = tempfile::tempdir().unwrap();
        let (state, home) = (dir.path().join("state"), dir.path().join("home"));
        let plain = write_view_in(&state, Some(&home), None, "1.html", "x").unwrap();
        assert_eq!(plain.path, state.join("view/tmpmail-1.html"));
        let snap = home.join("snap/firefox/common");
        std::fs::create_dir_all(&snap).unwrap();
        let ff = write_view_in(&state, Some(&home), Some("firefox"), "2.html", "y").unwrap();
        assert_eq!(ff.path, snap.join("tmpmail-view/tmpmail-2.html"));
        assert!(ff.note.is_none());
        // In the snap's folder only tmpmail's own copies are pruned.
        let other = snap.join("tmpmail-view/thesis.html");
        std::fs::write(&other, "z").unwrap();
        age(&other);
        age(&ff.path);
        write_view_in(&state, Some(&home), Some("firefox"), "3.html", "y").unwrap();
        assert!(other.exists() && !ff.path.exists());
        // A link anywhere below `~/snap` means the state folder, with a
        // note saying why.
        std::fs::create_dir_all(home.join("snap/evil")).unwrap();
        std::os::unix::fs::symlink(&state, home.join("snap/evil/common")).unwrap();
        let evil = write_view_in(&state, Some(&home), Some("evil"), "4.html", "y").unwrap();
        assert_eq!(evil.path, state.join("view/tmpmail-4.html"));
        let note = evil.note.unwrap();
        assert!(note.contains("symbolic link"), "{note}");
        assert!(!state.join("tmpmail-view").exists());
        // So does one the snap can't be written in, and a copy dated
        // before 1970 is pruned, not a crash.
        std::fs::create_dir(snap.join("tmpmail-view/tmpmail-6.html")).unwrap();
        let blocked = write_view_in(&state, Some(&home), Some("firefox"), "6.html", "y").unwrap();
        assert_eq!(blocked.path, state.join("view/tmpmail-6.html"));
        assert!(blocked.note.is_some());
        let ancient = snap.join("tmpmail-view/tmpmail-7.html");
        std::fs::write(&ancient, "x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&ancient)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH - Duration::from_secs(86400))
            .unwrap();
        write_view_in(&state, Some(&home), Some("firefox"), "8.html", "y").unwrap();
        assert!(!ancient.exists());
        // `~/snap` itself may be the user's own link (no snap can write
        // in it).
        let (home2, data) = (dir.path().join("home2"), dir.path().join("data"));
        std::fs::create_dir_all(data.join("firefox/common")).unwrap();
        std::fs::create_dir(&home2).unwrap();
        std::os::unix::fs::symlink(&data, home2.join("snap")).unwrap();
        let linked = write_view_in(&state, Some(&home2), Some("firefox"), "9.html", "y").unwrap();
        assert!(linked.note.is_none(), "{:?}", linked.note);
        assert!(
            data.join("firefox/common/tmpmail-view/tmpmail-9.html")
                .exists()
        );
        // A snap that never ran (no `common` folder) too.
        let none = write_view_in(&state, Some(&home), Some("chromium"), "5.html", "y").unwrap();
        assert_eq!(none.path, state.join("view/tmpmail-5.html"));
        assert!(none.note.is_some());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn old_and_other_snaps_view_files_are_pruned() {
        // Review of the security round: copies the first builds named
        // `ID.html`, and copies in another snap's folder (the browser
        // changed), were never deleted.
        let dir = tempfile::tempdir().unwrap();
        let (state, home) = (dir.path().join("state"), dir.path().join("home"));
        let views = |snap: &str| {
            let d = home.join("snap").join(snap).join("common/tmpmail-view");
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let (ff, chromium) = (views("firefox"), views("chromium"));
        let legacy = ff.join("6650a1b2c3d4e5f6a7b8c9d0.html");
        let mine = ff.join("thesis.html");
        let elsewhere = chromium.join("tmpmail-1.html");
        let elsewhere_legacy = chromium.join("12345.html");
        let theirs = chromium.join("notes.html");
        let view = crate::render::safe_html_document("s", "<p>x</p>", false);
        let report = ff.join("report-2024.html");
        for f in [
            &legacy,
            &mine,
            &elsewhere,
            &elsewhere_legacy,
            &theirs,
            &report,
        ] {
            let ours = [&legacy, &elsewhere_legacy].contains(&f);
            std::fs::write(f, if ours { view.as_str() } else { "x" }).unwrap();
            age(f);
        }
        // Another snap's folder through a link is left alone.
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(outside.join("common/tmpmail-view")).unwrap();
        let bait = outside.join("common/tmpmail-view/tmpmail-2.html");
        std::fs::write(&bait, "x").unwrap();
        age(&bait);
        std::os::unix::fs::symlink(&outside, home.join("snap/linked")).unwrap();
        write_view_in(&state, Some(&home), Some("firefox"), "3.html", "y").unwrap();
        assert!(!legacy.exists() && mine.exists() && report.exists());
        assert!(!elsewhere.exists() && !elsewhere_legacy.exists() && theirs.exists());
        assert!(bait.exists());
        // With a browser that isn't a snap, the snaps' copies go too.
        let last = ff.join("tmpmail-3.html");
        age(&last);
        write_view_in(&state, Some(&home), None, "4.html", "y").unwrap();
        assert!(!last.exists() && mine.exists());
        assert!(bait.exists());
    }
}
