//! The settings screen's data. Fields the CLI also reads (provider order,
//! poll interval, push, request timeout) live in the CLI's `config.toml`,
//! edited with `toml_edit` so the user's comments and other keys stay as they
//! were. The desktop-only ones (notifications, tray, start minimized) live in
//! `gui.json` next to it: tmpmail 0.2.0 and earlier refuse unknown config
//! keys (0.3.0 and later warn), so a `[gui]` table would stop them from
//! starting. `http_timeout` is new in 0.3.0: it is only written once the
//! timeout is changed from its default, and the screen says 0.2.0 and earlier
//! refuse it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tmpmail::config::{Config, DEFAULT_HTTP_TIMEOUT, DEFAULT_POLL_INTERVAL};
use tmpmail::provider;
use toml_edit::{Array, DocumentMut, Item, Value};

/// Bounds the settings screen enforces (the CLI clamps the interval at 2).
pub const POLL_INTERVAL: (u64, u64) = (2, 3600);
pub const HTTP_TIMEOUT: (u64, u64) = (1, 600);
/// The CLI's default for `push` (`Config::push` with nothing set).
fn default_push() -> bool {
    Config::default().push()
}

/// The environment variables that win over the config file's fields.
const OVERRIDES: &[&str] = &[
    "TMPMAIL_PROVIDER",
    "TMPMAIL_POLL_INTERVAL",
    "TMPMAIL_PUSH",
    "TMPMAIL_HTTP_TIMEOUT",
];

/// The desktop app's own settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gui {
    /// A notification for new mail while the window isn't focused.
    pub notifications: bool,
    /// A tray icon; closing the window then hides it.
    pub tray: bool,
    /// Start hidden in the tray (only with `tray`).
    pub start_minimized: bool,
}

impl Default for Gui {
    fn default() -> Self {
        Gui {
            notifications: true,
            tray: false,
            start_minimized: false,
        }
    }
}

/// `gui.json`, in the config file's directory.
pub fn gui_path() -> Option<PathBuf> {
    Config::path()?.parent().map(|d| d.join("gui.json"))
}

/// The desktop settings, or the defaults if there are none yet. A file that
/// can't be read is reported and the defaults are used: the app still starts.
pub fn load_gui() -> Gui {
    let Some(path) = gui_path() else {
        return Gui::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            eprintln!("tmpmail-gui: ignoring {}: {e}", path.display());
            Gui::default()
        }),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("tmpmail-gui: ignoring {}: {e}", path.display());
            }
            Gui::default()
        }
    }
}

fn save_gui(gui: &Gui) -> Result<(), String> {
    let path = gui_path().ok_or("there is no config directory on this system")?;
    let text = serde_json::to_string_pretty(gui).map_err(|e| e.to_string())? + "\n";
    write_replacing(&path, &text)
}

#[derive(Debug, Serialize)]
pub struct ProviderRow {
    id: &'static str,
    name: &'static str,
    enabled: bool,
}

/// What the settings screen shows.
#[derive(Debug, Serialize)]
pub struct View {
    config_path: Option<String>,
    config_exists: bool,
    /// Enabled providers in fallback order, then the others.
    providers: Vec<ProviderRow>,
    poll_interval: u64,
    push: bool,
    http_timeout: u64,
    gui: Gui,
    /// Environment variables that are set and win over the file.
    overrides: Vec<&'static str>,
    /// Whether a tray icon can be shown (Linux: a StatusNotifier host runs).
    tray_host: bool,
    os: &'static str,
}

/// The settings as they are in the files (not the environment's overrides).
pub fn view(gui: Gui, tray_host: bool) -> Result<View, String> {
    let path = Config::path();
    let (text, exists) = match &path {
        Some(p) => read_config(p)?,
        None => (String::new(), false),
    };
    let cfg = Config::parse(&text).map_err(|e| invalid(path.as_deref(), &e))?;
    let order = cfg.provider_order();
    let mut providers: Vec<ProviderRow> = order
        .iter()
        .filter_map(|id| provider::all().find(|known| known == id))
        .filter_map(|id| row(id, true))
        .collect();
    for id in provider::all() {
        if !order.iter().any(|o| o == id) {
            providers.extend(row(id, false));
        }
    }
    Ok(View {
        config_path: path.map(|p| p.display().to_string()),
        config_exists: exists,
        providers,
        // Within the bounds the screen accepts, so a value the CLI takes
        // (it clamps the interval) never blocks saving another field.
        poll_interval: cfg
            .poll_interval
            .unwrap_or(DEFAULT_POLL_INTERVAL)
            .clamp(POLL_INTERVAL.0, POLL_INTERVAL.1),
        push: cfg.push.unwrap_or_else(default_push),
        http_timeout: cfg
            .http_timeout
            .unwrap_or(DEFAULT_HTTP_TIMEOUT)
            .clamp(HTTP_TIMEOUT.0, HTTP_TIMEOUT.1),
        gui,
        overrides: OVERRIDES
            .iter()
            .copied()
            .filter(|v| std::env::var_os(v).is_some())
            .collect(),
        tray_host,
        os: std::env::consts::OS,
    })
}

fn row(id: &'static str, enabled: bool) -> Option<ProviderRow> {
    let p = provider::get(id).ok()?;
    Some(ProviderRow {
        id,
        name: p.name(),
        enabled,
    })
}

/// What the settings screen saves.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// The enabled providers, in fallback order.
    pub providers: Vec<String>,
    pub poll_interval: u64,
    pub push: bool,
    pub http_timeout: u64,
    pub notifications: bool,
    pub tray: bool,
    pub start_minimized: bool,
}

impl Input {
    fn check(&self) -> Result<(), String> {
        if self.providers.is_empty() {
            return Err("Turn on at least one provider.".into());
        }
        for (n, id) in self.providers.iter().enumerate() {
            if !provider::all().any(|known| known == id) {
                return Err(format!(
                    "Unknown provider {:?}.",
                    tmpmail::render::clean_line(id)
                ));
            }
            if self.providers[..n].contains(id) {
                return Err(format!("{id} is listed twice."));
            }
        }
        let (lo, hi) = POLL_INTERVAL;
        if !(lo..=hi).contains(&self.poll_interval) {
            return Err(format!(
                "The poll interval must be between {lo} and {hi} seconds."
            ));
        }
        let (lo, hi) = HTTP_TIMEOUT;
        if !(lo..=hi).contains(&self.http_timeout) {
            return Err(format!(
                "The request timeout must be between {lo} and {hi} seconds."
            ));
        }
        Ok(())
    }

    pub fn gui(&self) -> Gui {
        Gui {
            notifications: self.notifications,
            tray: self.tray,
            start_minimized: self.tray && self.start_minimized,
        }
    }
}

/// Checks `input`, then writes `config.toml` (only if a field changed) and
/// `gui.json`. Returns the desktop settings now in force.
pub fn save(input: &Input) -> Result<Gui, String> {
    input.check()?;
    let path = Config::path().ok_or("there is no config directory on this system")?;
    let (text, exists) = read_config(&path)?;
    let edited = edit_config(&text, input).map_err(|e| invalid(Some(&path), &e))?;
    let changed = edited != text && (exists || !edited.trim().is_empty());
    if changed {
        // The CLI must still read it.
        Config::parse(&edited)
            .map_err(|e| format!("tmpmail would write a config it can't read: {e}"))?;
        write_replacing(&path, &edited)?;
    }
    let gui = input.gui();
    save_gui(&gui)?;
    Ok(gui)
}

fn read_config(path: &Path) -> Result<(String, bool), String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok((s, true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((String::new(), false)),
        Err(e) => Err(format!("Couldn't read {}: {e}", path.display())),
    }
}

fn invalid(path: Option<&Path>, e: &dyn std::fmt::Display) -> String {
    let name = path.map_or("the config".into(), |p| p.display().to_string());
    format!("{name} isn't valid: {e}. Fix it by hand, then open the settings again.")
}

/// The config text with the input's fields set. A field still at its
/// default is left out if the file doesn't have it, so the file doesn't grow
/// keys the user never chose.
fn edit_config(text: &str, input: &Input) -> Result<String, String> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    let default_order = provider::DEFAULT_ORDER
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let order = Array::from_iter(input.providers.iter().map(String::as_str));
    put(
        &mut doc,
        "providers",
        Value::Array(order),
        input.providers == default_order,
    );
    put(
        &mut doc,
        "poll_interval",
        Value::from(input.poll_interval as i64),
        input.poll_interval == DEFAULT_POLL_INTERVAL,
    );
    put(
        &mut doc,
        "push",
        Value::from(input.push),
        input.push == default_push(),
    );
    put(
        &mut doc,
        "http_timeout",
        Value::from(input.http_timeout as i64),
        input.http_timeout == DEFAULT_HTTP_TIMEOUT,
    );
    Ok(doc.to_string())
}

/// Sets a top-level key, keeping the comment on its line.
fn put(doc: &mut DocumentMut, key: &str, mut value: Value, is_default: bool) {
    match doc.get_mut(key) {
        Some(Item::Value(old)) => {
            if old.to_string().trim() == value.to_string().trim() {
                return;
            }
            *value.decor_mut() = old.decor().clone();
            *old = value;
        }
        Some(item) => *item = Item::Value(value),
        None if is_default => {}
        None => {
            doc.insert(key, Item::Value(value));
        }
    }
}

/// Writes through a temporary file in the same directory, then renames it,
/// so a crash never leaves half a file. Keeps the old file's permissions.
fn write_replacing(path: &Path, text: &str) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("Couldn't save {}: {e}", path.display());
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(fail)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".tmpmail-")
        .tempfile_in(dir)
        .map_err(fail)?;
    std::io::Write::write_all(&mut tmp, text.as_bytes()).map_err(fail)?;
    tmp.as_file().sync_all().map_err(fail)?;
    match std::fs::metadata(path) {
        Ok(old) => tmp
            .as_file()
            .set_permissions(old.permissions())
            .map_err(fail)?,
        Err(_) => {
            // A new file: readable by others, as editors would create it.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tmp.as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o644))
                    .map_err(fail)?;
            }
        }
    }
    tmp.persist(path).map_err(|e| fail(e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Input {
        Input {
            providers: provider::DEFAULT_ORDER
                .iter()
                .map(|s| s.to_string())
                .collect(),
            poll_interval: 5,
            push: true,
            http_timeout: 20,
            notifications: true,
            tray: false,
            start_minimized: true,
        }
    }

    #[test]
    fn inputs_are_checked() {
        assert!(input().check().is_ok());
        let bad = |f: fn(&mut Input)| {
            let mut i = input();
            f(&mut i);
            i.check().is_err()
        };
        assert!(bad(|i| i.providers.clear()));
        assert!(bad(|i| i.providers.push("nope".into())));
        assert!(bad(|i| i.providers.push("mailtm".into())));
        assert!(bad(|i| i.poll_interval = 1));
        assert!(bad(|i| i.poll_interval = 3601));
        assert!(bad(|i| i.http_timeout = 0));
        assert!(bad(|i| i.http_timeout = 601));
        // Start minimized only counts with the tray on.
        assert!(!input().gui().start_minimized);
    }

    #[test]
    fn defaults_add_nothing_to_a_missing_or_bare_file() {
        assert_eq!(edit_config("", &input()).unwrap(), "");
        let text = "# mine\nbrowser = \"firefox\"\n";
        assert_eq!(edit_config(text, &input()).unwrap(), text);
    }

    #[test]
    fn edits_keep_comments_and_other_keys() {
        let text = "# my tmpmail config\nbrowser = \"firefox\"  # for links\n\
                    poll_interval = 5 # check often\npush = true\n";
        let mut i = input();
        i.providers = vec!["guerrilla".into(), "mailtm".into()];
        i.poll_interval = 9;
        i.push = false;
        i.http_timeout = 45;
        let out = edit_config(text, &i).unwrap();
        assert!(out.starts_with("# my tmpmail config\nbrowser = \"firefox\"  # for links\n"));
        assert!(out.contains("poll_interval = 9 # check often\n"), "{out}");
        assert!(out.contains("push = false\n"), "{out}");
        assert!(
            out.contains("providers = [\"guerrilla\", \"mailtm\"]"),
            "{out}"
        );
        assert!(out.contains("http_timeout = 45"), "{out}");
        let cfg = Config::parse(&out).unwrap();
        assert_eq!(cfg.provider_order(), ["guerrilla", "mailtm"]);
        assert_eq!(cfg.browser.as_deref(), Some("firefox"));
        // Back to the defaults: the keys the file has stay, set to them.
        let back = edit_config(&out, &input()).unwrap();
        assert!(back.contains("poll_interval = 5 # check often\n"), "{back}");
        assert!(back.contains("push = true\n"), "{back}");
        assert!(Config::parse(&back).unwrap().http_timeout == Some(20));
    }

    #[test]
    fn a_broken_config_is_not_overwritten() {
        assert!(edit_config("providers = [", &input()).is_err());
    }

    #[test]
    fn files_are_replaced_whole_with_their_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        write_replacing(&path, "push = false\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "push = false\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            write_replacing(&path, "push = true\n").unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
