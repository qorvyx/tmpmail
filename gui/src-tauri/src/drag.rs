//! Dragging an attachment out of the window into a file manager or another
//! app (Linux and macOS; best effort). A drag needs a real file, so the
//! attachment is first downloaded into a private temporary directory (0700,
//! one subdirectory per attachment, the sanitized name), when the pointer
//! reaches its drag handle. The page only ever names "attachment N of the
//! opened message", never a path. Files are deleted after a while and when
//! the app quits.
//!
//! This uses `drag` (the engine of the third-party `tauri-plugin-drag`)
//! directly from Rust: the plugin's own command takes any path from the page,
//! which the page must never choose.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Prepared files older than this are deleted when another is prepared
/// (the app that took the drop has copied it by then).
const KEEP: Duration = Duration::from_secs(10 * 60);

/// Whether this build can drag files out.
pub const SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "macos"));

#[derive(Default)]
pub struct Files {
    dir: Mutex<Option<TempDir>>,
    /// Prepared attachments by (message key, index): path and when.
    ready: Mutex<HashMap<(u64, usize), (PathBuf, Instant)>>,
}

/// Deletes the drag folders an earlier run of the app left in `temp` (it
/// crashed or was killed before it could): `tmpmail-drag-*` folders, not
/// links, the user's own, and older than [`KEEP`] (so another running
/// instance's files stay). Best effort.
pub fn clean_leftovers(temp: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    // The user's id, as the owner of a file made now.
    #[cfg(unix)]
    let me = {
        use std::os::unix::fs::MetadataExt;
        match tempfile::tempfile_in(temp).and_then(|f| f.metadata()) {
            Ok(m) => m.uid(),
            Err(_) => return,
        }
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with("tmpmail-drag-"))
        {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        #[cfg(unix)]
        let mine = {
            use std::os::unix::fs::MetadataExt;
            meta.uid() == me
        };
        #[cfg(not(unix))]
        let mine = true;
        let old = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > KEEP);
        if meta.is_dir() && mine && old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

impl Files {
    pub fn path(&self, key: u64, index: usize) -> Option<PathBuf> {
        self.ready
            .lock()
            .expect("drag lock")
            .get(&(key, index))
            .map(|(p, _)| p.clone())
    }

    /// Writes an attachment's bytes under its sanitized name and returns
    /// where. Old files go first.
    pub fn write(
        &self,
        key: u64,
        index: usize,
        name: &str,
        bytes: &[u8],
    ) -> Result<PathBuf, String> {
        self.sweep();
        let fail = |e: std::io::Error| format!("Couldn't prepare the file for dragging: {e}");
        let mut dir = self.dir.lock().expect("drag lock");
        if dir.is_none() {
            let made = tempfile::Builder::new()
                .prefix("tmpmail-drag-")
                .tempdir()
                .map_err(fail)?;
            // Only this user may look inside.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(made.path(), std::fs::Permissions::from_mode(0o700))
                    .map_err(fail)?;
            }
            *dir = Some(made);
        }
        let base = dir.as_ref().expect("just made").path();
        let sub = base.join(format!("{key}-{index}"));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&sub) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(fail(e)),
        }
        let path = sub.join(name);
        // A file from an earlier drag of it goes first; the new one is
        // created new, which never follows a link (as the CLI's `save`).
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(fail(e)),
        }
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
        let mut file = open.open(&path).map_err(fail)?;
        // Marked as from the internet (macOS), as Save marks it.
        tmpmail::store::mark_from_internet(&file, &path);
        if let Err(e) = std::io::Write::write_all(&mut file, bytes) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(fail(e));
        }
        // A folder in use looks new to another instance's start-up cleanup
        // (`clean_leftovers`), however long ago it was made.
        #[cfg(unix)]
        if let Ok(d) = std::fs::File::open(base) {
            let _ = d.set_modified(std::time::SystemTime::now());
        }
        drop(dir);
        self.ready
            .lock()
            .expect("drag lock")
            .insert((key, index), (path.clone(), Instant::now()));
        Ok(path)
    }

    /// Deletes prepared files older than `KEEP`.
    fn sweep(&self) {
        let mut ready = self.ready.lock().expect("drag lock");
        ready.retain(|_, (path, at)| {
            let keep = at.elapsed() < KEEP;
            if !keep && let Some(sub) = path.parent() {
                let _ = std::fs::remove_dir_all(sub);
            }
            keep
        });
    }

    /// Deletes every prepared file (the app is quitting).
    pub fn clear(&self) {
        self.ready.lock().expect("drag lock").clear();
        if let Some(dir) = self.dir.lock().expect("drag lock").take() {
            let _ = dir.close();
        }
    }
}

/// Starts dragging `path` out of `window`. Call on the main thread, while
/// the mouse button is down.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn start(window: &tauri::WebviewWindow, path: PathBuf) -> Result<(), String> {
    let icon = drag::Image::Raw(include_bytes!("../icons/32x32.png").to_vec());
    let item = drag::DragItem::Files(vec![path]);
    let done = |_result, _position| {};
    #[cfg(target_os = "linux")]
    let target = window.gtk_window().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let target = window.clone();
    drag::start_drag(&target, item, icon, done, drag::Options::default())
        .map_err(|e| format!("Couldn't start the drag: {e}. Use Save instead."))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn start(_window: &tauri::WebviewWindow, _path: PathBuf) -> Result<(), String> {
    Err("Dragging files out isn't available on this system. Use Save instead.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_files_are_private_and_deleted() {
        let files = Files::default();
        let path = files.write(3, 1, "report.pdf", b"%PDF").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF");
        assert_eq!(files.path(3, 1), Some(path.clone()));
        assert_eq!(path.file_name().unwrap(), "report.pdf");
        let base = path.parent().unwrap().parent().unwrap().to_path_buf();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&base), 0o700);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
            assert_eq!(mode(&path), 0o600);
        }
        // Old files go when another is prepared.
        files.ready.lock().unwrap().get_mut(&(3, 1)).unwrap().1 = Instant::now() - KEEP;
        files.write(4, 0, "b.txt", b"b").unwrap();
        assert!(!path.exists());
        assert!(files.path(3, 1).is_none());
        files.clear();
        assert!(!base.exists());
    }

    #[test]
    #[cfg(unix)]
    fn leftovers_of_an_earlier_run_are_removed() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("tmpmail-drag-old");
        let fresh = temp.path().join("tmpmail-drag-fresh");
        let other = temp.path().join("someone-else");
        for d in [&old, &fresh, &other] {
            std::fs::create_dir(d).unwrap();
            std::fs::write(d.join("f"), "x").unwrap();
        }
        let long_ago = std::time::SystemTime::now() - KEEP - Duration::from_secs(60);
        for d in [&old, &other] {
            std::fs::File::open(d)
                .unwrap()
                .set_modified(long_ago)
                .unwrap();
        }
        clean_leftovers(temp.path());
        assert!(!old.exists());
        assert!(fresh.exists() && other.exists());
    }
}
