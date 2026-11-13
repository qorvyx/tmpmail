//! Linux AppImage only: keeps COLRv1 color fonts away from the WebKitGTK
//! the AppImage carries. It was built against an older FreeType (2.11) than
//! the system's, which it uses (the AppImage doesn't carry FreeType), and
//! FreeType 2.13 changed the layout of the COLRv1 color stops without a new
//! soname: Skia then reads the stops wrong and asserts
//! (`colrv1_configure_skpaint`: `Assertion '__n < this->size()' failed`),
//! so the web process dies and the window goes blank as soon as such a
//! glyph is drawn. An emoji in a subject did it with Fedora's color emoji
//! font (`Noto-COLRv1.ttf`), on every start while that mail was listed. The
//! deb and rpm use the system's own WebKitGTK, built against its FreeType,
//! and are left alone.
//!
//! Before GTK (and so WebKit and its web processes, which inherit the
//! environment) starts, fontconfig is pointed (`FONTCONFIG_FILE`) at a
//! config written to the cache folder: it includes the system's own config
//! and rejects COLRv1 fonts (`<selectfont><rejectfont>`), by file: any
//! named `*COLRv1*`/`*colrv1*`, and every color font whose `COLR` table is
//! version 1 (fontconfig has no property for COLRv1: `color` is true for
//! the bitmap CBDT emoji too, which draw fine, and `fontformat` is just
//! `TrueType`). Emoji then come from the next font that has them: a
//! black-and-white one, or a CBDT color one. A browser the app starts for a
//! link gets the user's own setting back ([`open_url`]).

use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The config file, in `tmpmail` in the cache folder.
const FILE: &str = "fonts.conf";
/// fontconfig's own config when `FONTCONFIG_FILE` and `FONTCONFIG_PATH`
/// don't say otherwise.
const SYSTEM_CONFIG: &str = "/etc/fonts/fonts.conf";
/// The first line of our config's comment, which tells it from others.
const MARK: &str = "Written by tmpmail's AppImage";
/// How long `fc-list` may take to list the color fonts. It reads
/// fontconfig's cache, so it takes milliseconds; the name patterns still
/// apply if it doesn't answer.
const LIST_TIMEOUT: Duration = Duration::from_secs(2);
/// File-name patterns of COLRv1 fonts (upstream Noto's and Fedora's
/// `Noto-COLRv1.ttf`, `Noto-COLRv1-noflags.ttf`). fontconfig's globs are
/// case-sensitive.
const NAME_GLOBS: &[&str] = &["*COLRv1*", "*colrv1*"];

/// Set once `FONTCONFIG_FILE` was changed: what it was before (None: unset),
/// for the programs the app starts.
static USERS: OnceLock<Option<OsString>> = OnceLock::new();

/// Sets `FONTCONFIG_FILE` as the module says, when running from an
/// AppImage. Call it first in `main`, while no other thread runs.
pub fn avoid_colrv1() {
    if crate::freedesktop::appimage_dir().is_none() {
        return;
    }
    let Some(cache) = crate::freedesktop::dirs_cache() else {
        return;
    };
    let ours = cache.join("tmpmail").join(FILE);
    let inherited = std::env::var_os("FONTCONFIG_FILE");
    let Some(base) = base_config(
        inherited.clone().map(PathBuf::from),
        std::env::var_os("FONTCONFIG_PATH"),
        &ours,
    ) else {
        return;
    };
    let rejected = colrv1_files(&list_color_fonts());
    if write_if_changed(&ours, &config(&base, &rejected)).is_ok() {
        // An inherited setting that is ours (a browser this app started,
        // starting the app again) isn't the user's.
        let users = inherited.filter(|f| !is_ours(Path::new(f), &ours));
        let _ = USERS.set(users);
        // SAFETY: called first in `main`, before any thread starts (GTK,
        // WebKit, the worker and the plugins all start later; `fc-list`'s
        // reader is joined), so nothing reads the environment meanwhile.
        unsafe { std::env::set_var("FONTCONFIG_FILE", &ours) };
    }
}

/// Opens `url` as tauri-plugin-opener does (the same openers, in the same
/// order: `xdg-open` first), but with the user's own `FONTCONFIG_FILE`, so
/// a browser it starts shows every font. None when [`avoid_colrv1`] changed
/// nothing: the plugin does it then.
pub fn open_url(url: &str) -> Option<std::io::Result<()>> {
    let users = USERS.get()?;
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no program to open it");
    for mut cmd in open::commands(url) {
        match users {
            Some(v) => cmd.env("FONTCONFIG_FILE", v),
            None => cmd.env_remove("FONTCONFIG_FILE"),
        };
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

/// Whether `file` is a config this module wrote: `ours` (also spelled
/// another way), or one that says so (another cache folder's).
fn is_ours(file: &Path, ours: &Path) -> bool {
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    same(file, ours)
        || std::fs::File::open(file).is_ok_and(|f| {
            let mut head = String::new();
            let _ = f.take(512).read_to_string(&mut head);
            head.contains(MARK)
        })
}

/// The config file fontconfig would read now, to include from ours: the
/// absolute `FONTCONFIG_FILE` if set (unless it is one of ours, [`is_ours`]:
/// a copy of the app started from a program this one started), else
/// `fonts.conf` in the first `FONTCONFIG_PATH` folder that has one, else
/// [`SYSTEM_CONFIG`]. None (change nothing) for a relative
/// `FONTCONFIG_FILE` (fontconfig looks it up in folders of its own), or a
/// file that isn't there: fonts that are found matter more than this.
fn base_config(file: Option<PathBuf>, path: Option<OsString>, ours: &Path) -> Option<PathBuf> {
    let base = match file.filter(|f| !f.as_os_str().is_empty() && !is_ours(f, ours)) {
        Some(f) if f.is_absolute() => f,
        Some(_) => return None,
        None => path
            .iter()
            .flat_map(std::env::split_paths)
            .filter(|d| d.is_absolute())
            .map(|d| d.join("fonts.conf"))
            .find(|f| f.is_file() && !is_ours(f, ours))
            .unwrap_or_else(|| PathBuf::from(SYSTEM_CONFIG)),
    };
    (base.is_file() && !is_ours(&base, ours)).then_some(base)
}

/// The files of the color fonts fontconfig knows (`fc-list :color=true`),
/// or none if it can't be run or takes too long.
fn list_color_fonts() -> Vec<PathBuf> {
    let Ok(mut child) = Command::new("fc-list")
        .args(["--format", "%{file}\\n", ":color=true"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    // Read to its end on a thread of its own, so `fc-list` never blocks on
    // a full pipe; joined below whatever happens (the pipe closes once
    // `fc-list` exits or is killed).
    let reader = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.by_ref().take(1 << 20).read_to_end(&mut buf);
            buf
        })
    });
    let started = Instant::now();
    let done = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < LIST_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader
        .and_then(|r| r.join().ok())
        .filter(|_| done)
        .unwrap_or_default();
    String::from_utf8_lossy(&out)
        .lines()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .collect()
}

/// Those of `files` that are COLRv1 fonts ([`is_colrv1`]).
fn colrv1_files(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = files
        .iter()
        .filter(|f| std::fs::File::open(f).is_ok_and(|mut f| is_colrv1(&mut f)))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether the font (or a font of the collection) in `font` has a `COLR`
/// table of version 1 or later: COLRv1 color glyphs. Only its table
/// directory and that table's first two bytes are read.
fn is_colrv1(font: &mut (impl Read + Seek)) -> bool {
    let mut be = |at: u64, n: usize| -> Option<Vec<u8>> {
        font.seek(SeekFrom::Start(at)).ok()?;
        let mut buf = vec![0; n];
        font.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    let u16_at = |b: &[u8], i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
    let u32_at = |b: &[u8], i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let Some(head) = be(0, 12) else {
        return false;
    };
    // A collection (`ttcf`) lists its fonts' offsets; a font starts at 0.
    let offsets: Vec<u64> = if &head[..4] == b"ttcf" {
        let count = u32_at(&head, 8).min(64) as usize;
        match be(12, count * 4) {
            Some(b) => (0..count).map(|i| u64::from(u32_at(&b, i * 4))).collect(),
            None => return false,
        }
    } else {
        vec![0]
    };
    for offset in offsets {
        let Some(dir) = be(offset, 12) else {
            continue;
        };
        let tables = usize::from(u16_at(&dir, 4)).min(512);
        let Some(records) = be(offset + 12, tables * 16) else {
            continue;
        };
        for r in records.as_chunks::<16>().0 {
            if &r[..4] == b"COLR"
                && be(u64::from(u32_at(r, 8)), 2).is_some_and(|v| u16_at(&v, 0) >= 1)
            {
                return true;
            }
        }
    }
    false
}

/// The config: `base` included, then the COLRv1 fonts rejected by name
/// pattern and by file (`rejected`). A path with a glob character in it is
/// left to the name patterns (fontconfig would read it as a pattern).
fn config(base: &Path, rejected: &[PathBuf]) -> String {
    let mut globs: Vec<String> = NAME_GLOBS.iter().map(|g| (*g).to_string()).collect();
    globs.extend(
        rejected
            .iter()
            .filter_map(|p| p.to_str())
            .filter(|p| !p.contains(['*', '?', '[', ']']))
            .map(xml_escape),
    );
    let mut out = String::from(
        "<?xml version=\"1.0\"?>\n\
         <!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n\
         <!-- Written by tmpmail's AppImage at each start: the system's config, without\n     \
         COLRv1 color fonts, which the WebKitGTK it carries can't draw. -->\n\
         <fontconfig>\n",
    );
    out.push_str(&format!(
        "  <include ignore_missing=\"yes\">{}</include>\n  <selectfont>\n    <rejectfont>\n",
        xml_escape(&base.to_string_lossy())
    ));
    for g in globs {
        out.push_str(&format!("      <glob>{g}</glob>\n"));
    }
    out.push_str("    </rejectfont>\n  </selectfont>\n</fontconfig>\n");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Writes `contents` to `path` (through a temporary file, so fontconfig in
/// another copy of the app never reads half of it), unless it holds that.
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read(path).ok().as_deref() == Some(contents.as_bytes()) {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = path.with_extension(format!("conf.{}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::Cursor;

    /// A font with `tables` (tag, contents), laid out after the directory.
    fn font(tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0; 6]);
        let mut at = 12 + 16 * tables.len() as u32;
        let mut data: Vec<u8> = Vec::new();
        for (tag, contents) in tables {
            out.extend(*tag);
            out.extend([0; 4]);
            out.extend(at.to_be_bytes());
            out.extend((contents.len() as u32).to_be_bytes());
            at += contents.len() as u32;
            data.extend(*contents);
        }
        out.extend(data);
        out
    }

    #[test]
    fn colrv1_fonts_are_told_from_other_color_fonts() {
        // R6: COLRv1 (Fedora's Noto-COLRv1.ttf) crashes the AppImage's WebKit.
        let v1 = font(&[(b"cmap", b"xx"), (b"COLR", &[0, 1, 0, 0])]);
        assert!(is_colrv1(&mut Cursor::new(v1.clone())));
        // COLRv0 and bitmap (CBDT) color fonts draw fine and stay.
        let v0 = font(&[(b"COLR", &[0, 0, 0, 0]), (b"CPAL", b"")]);
        assert!(!is_colrv1(&mut Cursor::new(v0)));
        let cbdt = font(&[(b"CBDT", b"...."), (b"CBLC", b"....")]);
        assert!(!is_colrv1(&mut Cursor::new(cbdt)));
        // Cut short, or not a font at all.
        assert!(!is_colrv1(&mut Cursor::new(v1[..20].to_vec())));
        assert!(!is_colrv1(&mut Cursor::new(b"not a font".to_vec())));
        // A collection whose second font is COLRv1.
        let plain = font(&[(b"glyf", b"")]);
        let mut ttc = b"ttcf\0\x01\0\0\0\0\0\x02".to_vec();
        let first = 12 + 8;
        ttc.extend((first as u32).to_be_bytes());
        ttc.extend(((first + plain.len()) as u32).to_be_bytes());
        ttc.extend(&plain);
        // The second font's table offsets are from the file's start.
        let mut second = font(&[(b"COLR", &[0, 1])]);
        let shift = (first + plain.len()) as u32;
        let offset = u32::from_be_bytes(second[20..24].try_into().unwrap()) + shift;
        second[20..24].copy_from_slice(&offset.to_be_bytes());
        ttc.extend(second);
        assert!(is_colrv1(&mut Cursor::new(ttc)));
    }

    #[test]
    fn the_config_includes_the_systems_and_rejects_colrv1_by_file() {
        let conf = config(
            Path::new("/etc/fonts/fonts.conf"),
            &[
                PathBuf::from("/usr/share/fonts/emoji/Color & Co.ttf"),
                PathBuf::from("/odd/name*.ttf"),
            ],
        );
        assert!(conf.contains("<include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include>"));
        assert!(conf.contains("<glob>*COLRv1*</glob>"));
        assert!(conf.contains("<glob>/usr/share/fonts/emoji/Color &amp; Co.ttf</glob>"));
        assert!(!conf.contains("name*"));
        assert!(conf.find("<include").unwrap() < conf.find("<selectfont>").unwrap());
    }

    #[test]
    fn the_base_config_is_the_one_fontconfig_would_read() {
        let dir = tempfile::tempdir().unwrap();
        let ours = dir.path().join("cache/tmpmail/fonts.conf");
        let own = dir.path().join("own.conf");
        std::fs::write(&own, "<fontconfig/>").unwrap();
        let in_path = dir.path().join("etc");
        std::fs::create_dir(&in_path).unwrap();
        std::fs::write(in_path.join("fonts.conf"), "<fontconfig/>").unwrap();
        let path = std::env::join_paths([dir.path().join("none"), in_path.clone()]).unwrap();
        // The user's FONTCONFIG_FILE first.
        assert_eq!(base_config(Some(own.clone()), None, &ours), Some(own));
        // Then fonts.conf in FONTCONFIG_PATH.
        assert_eq!(
            base_config(None, Some(path.clone()), &ours),
            Some(in_path.join("fonts.conf"))
        );
        // Ours, inherited by a copy of the app a browser it started started
        // again, also spelled another way or from another cache folder:
        // never included (it would include itself), the next one is.
        std::fs::create_dir_all(ours.parent().unwrap()).unwrap();
        std::fs::write(&ours, config(Path::new(SYSTEM_CONFIG), &[])).unwrap();
        let other_cache = dir.path().join("other.conf");
        std::fs::write(&other_cache, config(Path::new(SYSTEM_CONFIG), &[])).unwrap();
        let spelled = dir.path().join("cache/./tmpmail/../tmpmail/fonts.conf");
        for inherited in [&ours, &spelled, &other_cache] {
            assert_eq!(
                base_config(Some(inherited.clone()), Some(path.clone()), &ours),
                Some(in_path.join("fonts.conf"))
            );
        }
        // A relative or missing file: unchanged.
        assert_eq!(base_config(Some("fonts.conf".into()), None, &ours), None);
        assert_eq!(
            base_config(Some(dir.path().join("gone.conf")), None, &ours),
            None
        );
    }

    #[test]
    fn fontconfig_takes_the_config_and_drops_the_colrv1_fonts() {
        // Where fontconfig's tools and a COLRv1 font are installed (Fedora's
        // color emoji), check the real thing; elsewhere there is nothing to see.
        let _spawning = crate::test_support::SPAWNING
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let color = list_color_fonts();
        let rejected = colrv1_files(&color);
        if rejected.is_empty() || !Path::new(SYSTEM_CONFIG).is_file() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        write_if_changed(&path, &config(Path::new(SYSTEM_CONFIG), &rejected)).unwrap();
        let run = |args: &[&str]| {
            let out = Command::new(args[0])
                .args(&args[1..])
                .env("FONTCONFIG_FILE", &path)
                .output()
                .unwrap();
            assert!(out.status.success());
            assert!(
                out.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        let left = run(&["fc-list", "--format", "%{file}\\n", ":color=true"]);
        for f in &rejected {
            assert!(!left.contains(f.to_str().unwrap()), "{left}");
        }
        let emoji = run(&["fc-match", "--format", "%{file}", "emoji"]);
        assert!(!rejected.iter().any(|f| f.to_str() == Some(emoji.as_str())));
        // The other fonts are all still there.
        let files = |s: &str| s.lines().map(str::to_string).collect::<HashSet<_>>();
        let after = files(&run(&["fc-list", "--format", "%{file}\\n"]));
        let before = Command::new("fc-list")
            .args(["--format", "%{file}\\n"])
            .output()
            .unwrap();
        let before = files(&String::from_utf8_lossy(&before.stdout));
        let gone: Vec<_> = before.difference(&after).collect();
        assert!(
            !gone.is_empty()
                && gone
                    .iter()
                    .all(|f| rejected.iter().any(|r| r.to_str() == Some(f.as_str()))),
            "{gone:?}"
        );
    }
}
