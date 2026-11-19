//! Linux AppImage only: the environment of the programs the app starts for
//! the user (the browser for a link, the mail client for a `mailto:` link).
//!
//! The AppImage runtime sets `APPIMAGE`, `APPDIR`, `OWD` and `ARGV0`, and
//! the AppRun linuxdeploy writes (its gtk hook, then AppImageKit's
//! `AppRun.wrapped`) points `PATH`, `LD_LIBRARY_PATH`, `XDG_DATA_DIRS`,
//! `GSETTINGS_SCHEMA_DIR`, `GI_TYPELIB_PATH`, `GIO_MODULE_DIR`, `GTK_PATH`,
//! `GTK_EXE_PREFIX`, `GTK_DATA_PREFIX`, `GTK_IM_MODULE_FILE`,
//! `GDK_PIXBUF_MODULE_FILE`, `PYTHONPATH`, `PYTHONHOME`, `PERLLIB`,
//! `QT_PLUGIN_PATH` and `GST_PLUGIN_SYSTEM_PATH(_1_0)` into the AppImage's
//! mount, sets `GTK_THEME` to Adwaita and `PYTHONDONTWRITEBYTECODE`. A
//! program started with all that would load the AppImage's libraries and
//! modules, and they are gone once the app quits (the mount goes with it).
//! So such a program gets the variables the runtime and AppRun set dropped,
//! and the entries under `$APPDIR` taken out of every variable that names
//! one (whatever its name: a path list or a single path), plus the user's
//! own `FONTCONFIG_FILE` back ([`crate::fonts`]). It also runs in the folder
//! the AppImage was started from (AppRun moves into the mount).
//!
//! The file manager (`reveal_config`) and notifications go over D-Bus: the
//! programs that answer are started by the bus, not by the app, so they
//! never had this environment.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Set by the AppImage runtime or by AppRun, and meaning nothing (or the
/// AppImage's choice, not the user's) to another program. `GTK_THEME` and
/// `PYTHONDONTWRITEBYTECODE` are set whatever the user had.
const DROP: &[&str] = &[
    "APPIMAGE",
    "APPDIR",
    "OWD",
    "ARGV0",
    "APPIMAGE_UUID",
    "GTK_THEME",
    "PYTHONDONTWRITEBYTECODE",
];

/// What to change in `vars` (the app's environment) for a program it
/// starts, when it runs from the AppImage mounted (or extracted) at
/// `appdir`: each name with its new value, or `None` to remove it.
/// Variables that need no change aren't listed.
pub fn changes(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    appdir: &Path,
) -> Vec<(OsString, Option<OsString>)> {
    let mut out = Vec::new();
    for (name, value) in vars {
        if DROP.iter().any(|d| OsStr::new(d) == name) {
            out.push((name, None));
            continue;
        }
        let inside = |entry: &[u8]| {
            !entry.is_empty() && Path::new(OsStr::from_bytes(entry)).starts_with(appdir)
        };
        let mut entries: Vec<&[u8]> = value.as_bytes().split(|&b| b == b':').collect();
        if !entries.iter().any(|e| inside(e)) {
            continue;
        }
        // AppRun adds its entries and `:` to the variable as it was, so one
        // that was unset ends in an empty entry (the current folder, for
        // `PATH` and `LD_LIBRARY_PATH`): that one goes. The user's own
        // empty entries elsewhere stay.
        if entries.last().is_some_and(|e| e.is_empty()) {
            entries.pop();
        }
        let mut kept: Vec<&[u8]> = Vec::new();
        let mut dropped_first = false;
        for entry in entries {
            if inside(entry) {
                dropped_first |= kept.is_empty();
                continue;
            }
            // The gtk hook puts `/usr/share` right after its own entry in
            // `XDG_DATA_DIRS` (for an unset one, GLib's default would have
            // had it): that one goes too.
            if name == "XDG_DATA_DIRS"
                && std::mem::take(&mut dropped_first)
                && kept.is_empty()
                && matches!(entry, b"/usr/share" | b"/usr/share/")
            {
                continue;
            }
            dropped_first = false;
            kept.push(entry);
        }
        let new = (!kept.is_empty()).then(|| OsString::from_vec(kept.join(&b':')));
        out.push((name, new));
    }
    out
}

/// The folder a program the app starts runs in: AppRun runs the app in
/// `$APPDIR/usr`, which goes away with the mount, so the folder the
/// AppImage was started from (`OWD`, set by the runtime) if it is outside,
/// else the home folder. None: leave it.
pub fn working_dir(owd: Option<&OsStr>, home: Option<&OsStr>, appdir: &Path) -> Option<PathBuf> {
    [owd, home]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|d| d.is_absolute() && !d.starts_with(appdir) && d.is_dir())
}

/// The environment and folder of a program the user's desktop would have
/// started ([`changes`], [`working_dir`]), with the user's own
/// `FONTCONFIG_FILE`.
struct Host {
    changes: Vec<(OsString, Option<OsString>)>,
    dir: Option<PathBuf>,
}

impl Host {
    /// None when the app doesn't run from an AppImage.
    fn get() -> Option<Self> {
        let appdir = crate::freedesktop::appimage_dir()?;
        Some(Host {
            changes: changes(std::env::vars_os(), &appdir),
            dir: working_dir(
                std::env::var_os("OWD").as_deref(),
                std::env::var_os("HOME").as_deref(),
                &appdir,
            ),
        })
    }

    fn apply(&self, cmd: &mut Command) {
        for (name, value) in &self.changes {
            match value {
                Some(v) => cmd.env(name, v),
                None => cmd.env_remove(name),
            };
        }
        if let Some(dir) = &self.dir {
            cmd.current_dir(dir);
        }
        crate::fonts::users_config(cmd);
    }
}

/// Opens `url` as tauri-plugin-opener does (the same openers, in the same
/// order: `xdg-open` first), as the user's desktop would ([`Host`]). None
/// when the app doesn't run from an AppImage: the plugin does it then.
pub fn open_url(url: &str) -> Option<std::io::Result<()>> {
    let host = Host::get()?;
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no program to open it");
    for mut cmd in open::commands(url) {
        host.apply(&mut cmd);
        let spawned = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(mut child) => {
                // Reaped when it exits (xdg-open returns once the browser
                // has the link, or runs as the browser's parent).
                std::thread::spawn(move || child.wait());
                return Some(Ok(()));
            }
            Err(e) => last = e,
        }
    }
    Some(Err(last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }

    fn apply(pairs: &[(&str, &str)], appdir: &str) -> Vec<(String, Option<String>)> {
        let mut out: Vec<_> = changes(env(pairs), Path::new(appdir))
            .into_iter()
            .map(|(k, v)| {
                (
                    k.into_string().unwrap(),
                    v.map(|v| v.into_string().unwrap()),
                )
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn the_appimage_environment_is_taken_out() {
        // As the runtime, the gtk hook and AppRun.wrapped of tmpmail's
        // AppImage leave it, mounted at /tmp/.mount_tmpmaiXYZ.
        let a = "/tmp/.mount_tmpmaiXYZ";
        let got = apply(
            &[
                ("APPIMAGE", "/home/u/Apps/tmpmail.AppImage"),
                ("APPDIR", a),
                ("OWD", "/home/u"),
                ("ARGV0", "./tmpmail.AppImage"),
                ("GTK_THEME", "Adwaita:dark"),
                ("PYTHONDONTWRITEBYTECODE", "1"),
                (
                    "PATH",
                    &format!(
                        "{a}/usr/bin/:{a}/usr/sbin/:{a}/usr/games/:{a}/bin/:{a}/sbin/:/home/u/.local/bin:/usr/bin"
                    ),
                ),
                // Unset before: only AppRun's entries, and an empty one.
                (
                    "LD_LIBRARY_PATH",
                    &format!("{a}/usr/lib/:{a}/usr/lib/x86_64-linux-gnu/:{a}/lib64/:"),
                ),
                (
                    "XDG_DATA_DIRS",
                    &format!(
                        "{a}/usr/share/:{a}/usr/share:/usr/share:/home/u/.local/share/flatpak/exports/share:/usr/local/share:/usr/share"
                    ),
                ),
                (
                    "GSETTINGS_SCHEMA_DIR",
                    &format!("{a}/usr/share/glib-2.0/schemas/:{a}//usr/share/glib-2.0/schemas"),
                ),
                (
                    "GTK_IM_MODULE_FILE",
                    &format!("{a}//usr/lib/gtk-3.0/3.0.0/immodules.cache"),
                ),
                (
                    "GDK_PIXBUF_MODULE_FILE",
                    &format!("{a}//usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"),
                ),
                ("GIO_MODULE_DIR", &format!("{a}//usr/lib/gio/modules")),
                ("GTK_PATH", &format!("{a}//usr/lib/gtk-3.0")),
                ("GTK_EXE_PREFIX", &format!("{a}//usr")),
                ("GTK_DATA_PREFIX", a),
                ("GI_TYPELIB_PATH", &format!("{a}//usr/lib/girepository-1.0")),
                ("PYTHONHOME", &format!("{a}/usr/")),
                ("PYTHONPATH", &format!("{a}/usr/share/pyshared/:/home/u/py")),
                (
                    "PERLLIB",
                    &format!("{a}/usr/share/perl5/:{a}/usr/lib/perl5/:"),
                ),
                ("PERL5LIB", "/home/u/perl5/lib/perl5"),
                ("QT_PLUGIN_PATH", &format!("{a}/usr/lib/qt5/plugins/:")),
                (
                    "GST_PLUGIN_SYSTEM_PATH_1_0",
                    &format!("{a}/usr/lib/gstreamer-1.0:"),
                ),
                // The user's own, untouched.
                ("HOME", "/home/u"),
                ("WAYLAND_DISPLAY", "wayland-0"),
                // Not under the mount, though it starts the same.
                ("MANPATH", &format!("{a}-other/man:/usr/share/man")),
            ],
            a,
        );
        let removed = |k: &str| (k.to_string(), None);
        let set = |k: &str, v: &str| (k.to_string(), Some(v.to_string()));
        let mut want = vec![
            removed("APPIMAGE"),
            removed("APPDIR"),
            removed("OWD"),
            removed("ARGV0"),
            removed("GTK_THEME"),
            removed("PYTHONDONTWRITEBYTECODE"),
            set("PATH", "/home/u/.local/bin:/usr/bin"),
            removed("LD_LIBRARY_PATH"),
            set(
                "XDG_DATA_DIRS",
                "/home/u/.local/share/flatpak/exports/share:/usr/local/share:/usr/share",
            ),
            removed("GSETTINGS_SCHEMA_DIR"),
            removed("GTK_IM_MODULE_FILE"),
            removed("GDK_PIXBUF_MODULE_FILE"),
            removed("GIO_MODULE_DIR"),
            removed("GTK_PATH"),
            removed("GTK_EXE_PREFIX"),
            removed("GTK_DATA_PREFIX"),
            removed("GI_TYPELIB_PATH"),
            removed("PYTHONHOME"),
            set("PYTHONPATH", "/home/u/py"),
            removed("PERLLIB"),
            removed("QT_PLUGIN_PATH"),
            removed("GST_PLUGIN_SYSTEM_PATH_1_0"),
        ];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn the_users_own_empty_entries_stay() {
        let a = "/tmp/.mount_x";
        let got = apply(&[("PATH", &format!("{a}/usr/bin/:{a}/bin/::/usr/bin"))], a);
        assert_eq!(
            got,
            vec![("PATH".to_string(), Some(":/usr/bin".to_string()))]
        );
    }

    #[test]
    fn the_program_runs_outside_the_mount() {
        let dir = tempfile::tempdir().unwrap();
        let mount = dir.path().join(".mount_x");
        std::fs::create_dir_all(mount.join("usr")).unwrap();
        let owd = dir.path().join("Downloads");
        std::fs::create_dir(&owd).unwrap();
        let home = dir.path();
        let os = |p: &Path| p.as_os_str().to_owned();
        assert_eq!(
            working_dir(Some(&os(&owd)), Some(&os(home)), &mount),
            Some(owd.clone())
        );
        // Started from inside the mount, or from a folder that is gone.
        let inside = mount.join("usr");
        assert_eq!(
            working_dir(Some(&os(&inside)), Some(&os(home)), &mount),
            Some(home.to_path_buf())
        );
        let gone = dir.path().join("gone");
        assert_eq!(
            working_dir(Some(&os(&gone)), Some(&os(home)), &mount),
            Some(home.to_path_buf())
        );
        assert_eq!(working_dir(None, None, &mount), None);
    }

    #[test]
    fn an_unset_xdg_data_dirs_is_unset_again() {
        // The hook's `/usr/share` goes with its own entry, so GLib's
        // default (with /usr/local/share) applies again.
        let a = "/tmp/appimage_extracted_abc";
        let got = apply(
            &[(
                "XDG_DATA_DIRS",
                &format!("{a}/usr/share/:{a}/usr/share:/usr/share:"),
            )],
            a,
        );
        assert_eq!(got, vec![("XDG_DATA_DIRS".to_string(), None)]);
        // A `/usr/share` of the user's own, later in the list, stays.
        let got = apply(
            &[(
                "XDG_DATA_DIRS",
                &format!("{a}/usr/share:/opt/share:/usr/share"),
            )],
            a,
        );
        assert_eq!(
            got,
            vec![(
                "XDG_DATA_DIRS".to_string(),
                Some("/opt/share:/usr/share".to_string())
            )]
        );
    }
}
