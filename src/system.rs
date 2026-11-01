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

/// Writes a viewer file for `browser` (as [`open`] runs it), readable only
/// by the user, and returns its path. It goes to `view` in the state
/// folder, except for a browser installed as a snap (see [`view_dir`]).
/// Copies left over from earlier views, in either place, are deleted once
/// they are a day old.
pub fn write_view(
    state: &Path,
    browser: Option<&str>,
    name: &str,
    contents: &str,
) -> crate::error::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    let snap = snap_browser(browser_cmd(browser).as_deref(), &Snapd::system());
    #[cfg(not(target_os = "linux"))]
    let snap: Option<String> = {
        let _ = browser;
        None
    };
    let home = dirs::home_dir();
    let views = view_dir(state, home.as_deref(), snap.as_deref());
    let path = write_view_file(&views, name, contents)?;
    for dir in view_dirs(state, home.as_deref()) {
        prune_views(&dir, VIEW_MAX_AGE);
    }
    Ok(path)
}

/// Where view files go: `view` in the state folder, or for a snap browser
/// (the Firefox of Ubuntu and Kubuntu) `~/snap/NAME/common/tmpmail-view`.
/// Snap confinement keeps a snap out of hidden folders in home (so
/// `~/.local/state`), it has its own `/tmp`, and sees only its own part of
/// `$XDG_RUNTIME_DIR`; its `~/snap/NAME/common` it may always read. (GNOME
/// hands a file to a snap through the document portal, so the state folder
/// worked there; KDE doesn't.)
fn view_dir(state: &Path, home: Option<&Path>, snap: Option<&str>) -> PathBuf {
    #[cfg(target_os = "linux")]
    if let (Some(home), Some(name)) = (home, snap) {
        return home.join("snap").join(name).join("common").join(SNAP_VIEWS);
    }
    let _ = (home, snap);
    state.join("view")
}

/// Every folder view files may have been written to.
fn view_dirs(state: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![state.join("view")];
    #[cfg(target_os = "linux")]
    if let Some(snaps) = home.map(|h| h.join("snap")) {
        for entry in std::fs::read_dir(&snaps).into_iter().flatten().flatten() {
            let dir = entry.path().join("common").join(SNAP_VIEWS);
            if no_links_below(&snaps, &dir) {
                dirs.push(dir);
            }
        }
    }
    let _ = home;
    dirs
}

/// Whether nothing from `base` (exclusive) down to `path` is a symbolic
/// link. A snap may write in its own folder: it must not be able to point
/// tmpmail's view folder somewhere else.
fn no_links_below(base: &Path, path: &Path) -> bool {
    let Ok(rest) = path.strip_prefix(base) else {
        return false;
    };
    let mut at = base.to_path_buf();
    rest.components().all(|c| {
        at.push(c);
        !std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink())
    })
}

/// Writes a viewer file readable only by the user into `views` (made 0700)
/// and returns its path. Neither the folder nor the file is followed if it
/// is a symbolic link: a snap browser can write in the folder its copies
/// go to.
fn write_view_file(views: &Path, name: &str, contents: &str) -> crate::error::Result<PathBuf> {
    let parent = views.parent().unwrap_or(views);
    std::fs::create_dir_all(parent).map_err(|e| anyhow!("create {}: {e}", parent.display()))?;
    match std::fs::create_dir(views) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(anyhow!("create {}: {e}", views.display()).into()),
    }
    private_real_dir(views)?;
    let path = views.join(name);
    // A copy of the same message, or anything put in its place, goes first;
    // then the file is created new, which never follows a link.
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path).map_err(|e| anyhow!("remove {}: {e}", path.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&path)
        .map_err(|e| anyhow!("open {}: {e}", path.display()))?;
    f.write_all(contents.as_bytes())
        .map_err(|e| anyhow!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Makes `dir` 0700 if it is a real folder, refusing a symbolic link (the
/// mode is set through the opened folder, so it can't be swapped for one
/// in between).
fn private_real_dir(dir: &Path) -> crate::error::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let f = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(dir)
            .map_err(|e| anyhow!("open {}: {e}", dir.display()))?;
        f.set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|e| anyhow!("chmod {}: {e}", dir.display()))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
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

/// Best effort: a file that can't be inspected or removed is left alone.
fn prune_views(views: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(views) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
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
    fn old_view_files_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let views = dir.path().join("view");
        let old = views.join("old.html");
        let first = write_view_file(&views, "old.html", "x").unwrap();
        assert_eq!(first, old);
        let day_ago = SystemTime::now() - VIEW_MAX_AGE - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(day_ago)
            .unwrap();
        let new = write_view_file(&views, "new.html", "y").unwrap();
        prune_views(&views, VIEW_MAX_AGE);
        assert!(!old.exists());
        assert!(new.exists());
        // Writing the same message again replaces its copy.
        write_view_file(&views, "new.html", "z").unwrap();
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "z");
    }

    #[test]
    #[cfg(unix)]
    fn view_files_are_private_and_never_follow_links() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let views = dir.path().join("view");
        let f = write_view_file(&views, "a.html", "x").unwrap();
        assert_eq!(mode(&views), 0o700);
        assert_eq!(mode(&f), 0o600);
        // A link put in place of a copy is replaced, its target untouched.
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        std::fs::remove_file(&f).unwrap();
        symlink(&victim, &f).unwrap();
        write_view_file(&views, "a.html", "new").unwrap();
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
        let linked = dir.path().join("linked");
        symlink(&elsewhere, &linked).unwrap();
        assert!(write_view_file(&linked, "a.html", "x").is_err());
        assert_eq!(mode(&elsewhere), 0o755);
        assert!(!elsewhere.join("a.html").exists());
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
        assert_eq!(view_dir(&state, Some(&home), None), state.join("view"));
        let snap = home.join("snap/firefox/common/tmpmail-view");
        assert_eq!(view_dir(&state, Some(&home), Some("firefox")), snap);
        // Both places are pruned; a snap's folder only while no link leads
        // out of `~/snap`.
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::create_dir_all(home.join("snap/evil")).unwrap();
        std::os::unix::fs::symlink(&state, home.join("snap/evil/common")).unwrap();
        let dirs = view_dirs(&state, Some(&home));
        assert!(
            dirs.contains(&state.join("view")) && dirs.contains(&snap),
            "{dirs:?}"
        );
        assert_eq!(dirs.len(), 2, "{dirs:?}");
    }
}
