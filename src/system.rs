//! Talking to the desktop: clipboard, browser, notifications.

use std::io::Write;
use std::path::Path;
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
    let cmd = browser
        .map(str::to_string)
        .or_else(|| std::env::var("BROWSER").ok().filter(|b| !b.is_empty()));
    #[cfg(windows)]
    if cmd.is_none() {
        return windows_open(target);
    }
    let cmd = cmd
        .or_else(|| {
            ["xdg-open", "open", "wslview"]
                .into_iter()
                .find(|b| on_path(b))
                .map(str::to_string)
        })
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

/// Writes a viewer file readable only by the user and returns its path.
/// Copies left over from earlier views are deleted once they are a day old.
pub fn write_view_file(
    dir: &Path,
    name: &str,
    contents: &str,
) -> crate::error::Result<std::path::PathBuf> {
    let views = dir.join("view");
    crate::store::create_private_dir(&views)?;
    prune_views(&views, VIEW_MAX_AGE);
    let path = views.join(name);
    crate::store::write_private(&path, contents.as_bytes())?;
    Ok(path)
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
        let old = dir.path().join("view").join("old.html");
        let first = write_view_file(dir.path(), "old.html", "x").unwrap();
        assert_eq!(first, old);
        let day_ago = SystemTime::now() - VIEW_MAX_AGE - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(day_ago)
            .unwrap();
        let new = write_view_file(dir.path(), "new.html", "y").unwrap();
        assert!(!old.exists());
        assert!(new.exists());
    }
}
