//! Dragging an attachment out of the window into a file manager or another
//! app (Linux and macOS; best effort). A drag needs a real file, so the
//! attachment is first downloaded into a private temporary directory (0700,
//! one subdirectory per attachment, the sanitized name), when the pointer
//! reaches its drag handle. The page only ever names "attachment N of the
//! opened message", never a path. Files are deleted after a while and when
//! the app quits.
//!
//! On macOS this uses `drag` (the engine of the third-party
//! `tauri-plugin-drag`) directly from Rust: the plugin's own command takes
//! any path from the page, which the page must never choose. On Linux the
//! drag is GTK's own ([`gtk_drag`]): `drag` stops answering for the file as
//! soon as the drop is sent, which on X11 comes before the drop target asks
//! for it, so nothing landed there.

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

/// A file in each drag folder that its app keeps locked while it runs, so
/// another instance's start-up cleanup leaves the folder alone.
const LOCK: &str = ".lock";

#[derive(Default)]
pub struct Files {
    /// The folder, and its locked [`LOCK`] file (none where the file
    /// system can't lock: the folder is then kept by its age alone).
    dir: Mutex<Option<(TempDir, Option<std::fs::File>)>>,
    /// Prepared attachments by (message key, index): path and when.
    ready: Mutex<HashMap<(u64, usize), (PathBuf, Instant)>>,
}

/// Deletes the drag folders an earlier run of the app left in `temp` (it
/// crashed or was killed before it could): `tmpmail-drag-*` folders, not
/// links, the user's own, older than [`KEEP`], and not another running
/// instance's: one whose [`LOCK`] file is locked stays, however old. (The
/// age also covers an instance that made its folder and hasn't locked it
/// yet, one whose file system can't lock, and older versions, which made
/// none.) Best effort.
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
        if !(meta.is_dir() && mine && old) {
            continue;
        }
        // Held until the folder is gone: no instance takes a folder it
        // didn't make, so none can start using it meanwhile.
        let _lock = match open_lock(&entry.path().join(LOCK), false) {
            Ok(f) => match f.try_lock() {
                Ok(()) => Some(f),
                // A running instance's, or a lock this file system can't
                // tell: left alone.
                Err(_) => continue,
            },
            // None (an older version's folder, or one whose lock couldn't
            // be made): the age decides.
            Err(_) => None,
        };
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

/// Opens (or with `create`, makes) a drag folder's lock file. Never
/// through a link: a new one is made new, and an existing one must be a
/// plain file (in a folder of the user's own, which no one else can change).
fn open_lock(path: &std::path::Path, create: bool) -> std::io::Result<std::fs::File> {
    let mut open = std::fs::OpenOptions::new();
    // Writable: some file systems (NFS) lock only a file open for writing.
    open.read(true).write(true);
    if create {
        open.create_new(true);
    } else if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(std::io::Error::other("not a plain file"));
    }
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    open.open(path)
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
            // Locked for as long as this app runs, where the file system
            // can lock; without it the folder's age keeps it (see `write`).
            let lock = open_lock(&made.path().join(LOCK), true)
                .ok()
                .filter(|f| f.lock().is_ok());
            *dir = Some((made, lock));
        }
        let base = dir.as_ref().expect("just made").0.path();
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
        if let Some((dir, lock)) = self.dir.lock().expect("drag lock").take() {
            drop(lock);
            let _ = dir.close();
        }
    }
}

/// The drag's icon.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const DRAG_ICON: &[u8] = include_bytes!("../icons/32x32.png");

/// Starts dragging `path` out of `window`. Call on the main thread, while
/// the mouse button is down.
#[cfg(target_os = "macos")]
pub fn start(window: &tauri::WebviewWindow, path: PathBuf) -> Result<(), String> {
    let icon = drag::Image::Raw(DRAG_ICON.to_vec());
    let item = drag::DragItem::Files(vec![path]);
    let done = |_result, _position| {};
    drag::start_drag(window, item, icon, done, drag::Options::default())
        .map_err(|e| format!("Couldn't start the drag: {e}. Use Save instead."))
}

/// Starts dragging `path` out of `window`. Call on the main thread, while
/// the mouse button is down.
#[cfg(target_os = "linux")]
pub fn start(window: &tauri::WebviewWindow, path: PathBuf) -> Result<(), String> {
    let window = window.gtk_window().map_err(|e| e.to_string())?;
    gtk_drag(&window, &path).map_err(|e| format!("Couldn't start the drag: {e}. Use Save instead."))
}

/// The `file://` URI a drop target is given for `path`, percent-encoded
/// (a name with spaces or accents is no valid URI as it is).
#[cfg(target_os = "linux")]
fn file_uri(path: &std::path::Path) -> Result<String, String> {
    gtk::glib::filename_to_uri(path, None)
        .map(|u| u.to_string())
        .map_err(|e| e.to_string())
}

/// A GTK drag of the file at `path` from `window`, as a copy, offered as
/// `text/uri-list`. The window answers the
/// drop target's requests for the file's URI until GTK ends the drag
/// (`drag-end`, after the drop and its data transfer, or a cancel). Under
/// X11 (XDND) the target asks for the data only after the drop
/// (`XdndDrop`, then a selection request), and on Wayland at the drop too,
/// so the answer must outlive the drop itself. Only this drag's own
/// requests are answered: a drag of the page's (the webview's) isn't
/// touched.
#[cfg(target_os = "linux")]
fn gtk_drag(window: &gtk::ApplicationWindow, path: &std::path::Path) -> Result<(), String> {
    use gtk::prelude::*;
    use gtk::{gdk, gdk_pixbuf};
    use std::cell::RefCell;
    use std::rc::Rc;

    let uri = file_uri(path)?;
    let targets = gtk::TargetList::new(&[]);
    targets.add_uri_targets(0);
    let ours: Rc<RefCell<Option<gdk::DragContext>>> = Rc::default();
    let handlers: Rc<RefCell<Vec<gtk::glib::SignalHandlerId>>> = Rc::default();
    let is_ours = {
        let ours = ours.clone();
        move |ctx: &gdk::DragContext| ours.borrow().as_ref() == Some(ctx)
    };
    let data = {
        let is_ours = is_ours.clone();
        window.connect_drag_data_get(move |_, ctx, data, _, _| {
            if is_ours(ctx) {
                data.set_uris(&[uri.as_str()]);
            }
        })
    };
    let end = {
        let (ours, handlers) = (ours.clone(), handlers.clone());
        window.connect_drag_end(move |window, ctx| {
            if is_ours(ctx) {
                ours.borrow_mut().take();
                for id in handlers.borrow_mut().drain(..) {
                    window.disconnect(id);
                }
            }
        })
    };
    handlers.borrow_mut().extend([data, end]);
    let started = window.drag_begin_with_coordinates(
        &targets,
        gdk::DragAction::COPY,
        // The button that drags (the first), not a modifier mask.
        1,
        None,
        -1,
        -1,
    );
    let Some(ctx) = started else {
        for id in handlers.borrow_mut().drain(..) {
            window.disconnect(id);
        }
        return Err("the window couldn't start it".into());
    };
    *ours.borrow_mut() = Some(ctx.clone());
    let loader = gdk_pixbuf::PixbufLoader::new();
    if loader.write(DRAG_ICON).and_then(|_| loader.close()).is_ok()
        && let Some(icon) = loader.pixbuf()
    {
        ctx.drag_set_icon_pixbuf(&icon, 0, 0);
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn start(_window: &tauri::WebviewWindow, _path: PathBuf) -> Result<(), String> {
    Err("Dragging files out isn't available on this system. Use Save instead.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn dropped_files_are_named_by_a_valid_uri() {
        // Retest F8 (the URI went out unescaped as well).
        assert_eq!(
            file_uri(std::path::Path::new(
                "/tmp/tmpmail-drag-x/1-0/résumé été ✓ #1.txt"
            ))
            .unwrap(),
            "file:///tmp/tmpmail-drag-x/1-0/r%C3%A9sum%C3%A9%20%C3%A9t%C3%A9%20%E2%9C%93%20%231.txt"
        );
        assert!(file_uri(std::path::Path::new("relative.txt")).is_err());
    }

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

    #[test]
    #[cfg(unix)]
    fn another_running_instance_keeps_its_folder() {
        // Review of the security round: a second start deleted the drag
        // folder of an app that had run for more than ten minutes, and
        // drag-out failed in it until a restart.
        let _spawning = crate::test_support::SPAWNING
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let long_ago = std::time::SystemTime::now() - KEEP - Duration::from_secs(60);
        let folder = |name: &str, at: std::time::SystemTime| {
            let d = temp.path().join(name);
            std::fs::create_dir(&d).unwrap();
            let lock = open_lock(&d.join(LOCK), true).unwrap();
            std::fs::File::open(&d).unwrap().set_modified(at).unwrap();
            (d, lock)
        };
        let (running, held) = folder("tmpmail-drag-running", long_ago);
        held.lock().unwrap();
        // A crashed run's lock is free.
        let (crashed, free) = folder("tmpmail-drag-crashed", long_ago);
        drop(free);
        // One just made, not locked yet, stays by its age.
        let (fresh, _unlocked) = folder("tmpmail-drag-fresh", std::time::SystemTime::now());
        clean_leftovers(temp.path());
        assert!(running.exists() && fresh.exists());
        assert!(!crashed.exists());
        drop(held);
        clean_leftovers(temp.path());
        assert!(!running.exists());
    }
}
