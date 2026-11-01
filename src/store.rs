//! Local state: the inboxes we created and which one is active. Credentials
//! live here, so the directory is 0700 and the file 0600, written atomically.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::provider::Inbox;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub inboxes: Vec<Inbox>,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    /// `$TMPMAIL_STATE_DIR`, else `$XDG_STATE_HOME/tmpmail` (or the platform
    /// equivalent). Never /tmp: other users could read the credentials.
    pub fn default_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("TMPMAIL_STATE_DIR") {
            return PathBuf::from(dir);
        }
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tmpmail")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn load(&self) -> Result<State> {
        match fs::read_to_string(self.path()) {
            Ok(s) => serde_json::from_str(&s)
                .with_context(|| format!("corrupt state file {}", self.path().display()))
                .map_err(Error::from),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(anyhow::Error::from(e)
                .context(format!("cannot read {}", self.path().display()))
                .into()),
        }
    }

    /// Applies `f` to the state on disk and saves it, holding a lock so a
    /// concurrent tmpmail (say a long `watch`) can't overwrite the change
    /// with an older copy. Returns the new state and what `f` returned.
    pub fn update<T>(&self, f: impl FnOnce(&mut State) -> T) -> Result<(State, T)> {
        create_private_dir(&self.dir)?;
        let _lock = self.lock()?;
        let mut state = self.load()?;
        let out = f(&mut state);
        self.save(&state)?;
        Ok((state, out))
    }

    /// Exclusive lock held until the returned file is dropped.
    fn lock(&self) -> Result<fs::File> {
        let path = self.dir.join(".lock");
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: flock on a file descriptor we own; released on close.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(anyhow::Error::from(std::io::Error::last_os_error())
                    .context(format!("lock {}", path.display()))
                    .into());
            }
        }
        Ok(file)
    }

    fn save(&self, state: &State) -> Result<()> {
        let tmp = self.dir.join(format!(".state.json.{}", std::process::id()));
        let data = serde_json::to_vec_pretty(state).context("serialize state")?;
        write_private(&tmp, &data)?;
        fs::rename(&tmp, self.path()).context("replace state file")?;
        Ok(())
    }
}

impl State {
    /// Index of the inbox with exactly this address (any case).
    pub fn position(&self, address: &str) -> Option<usize> {
        self.inboxes
            .iter()
            .position(|i| i.address.eq_ignore_ascii_case(address))
    }

    pub fn find(&self, address: &str) -> Option<usize> {
        self.position(address)
            .or_else(|| match self.matches(address).as_slice() {
                [one] => Some(*one),
                _ => None,
            })
    }

    /// The inbox with this address, else the inbox with this label, else
    /// every inbox with this local part (any case): shorthands, which only
    /// pick one when unique. A label beats a local part, so the inbox it
    /// names stays reachable by it whatever address another inbox gets.
    fn matches(&self, name: &str) -> Vec<usize> {
        if let Some(n) = self.position(name) {
            return vec![n];
        }
        let labeled = self.labeled(name);
        if !labeled.is_empty() {
            return labeled;
        }
        self.inboxes
            .iter()
            .enumerate()
            .filter(|(_, i)| i.local_part().eq_ignore_ascii_case(name))
            .map(|(n, _)| n)
            .collect()
    }

    /// Every inbox with this label (any case): one, unless an older tmpmail
    /// stored a clash.
    fn labeled(&self, name: &str) -> Vec<usize> {
        self.inboxes
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                i.label
                    .as_deref()
                    .is_some_and(|l| l.eq_ignore_ascii_case(name))
            })
            .map(|(n, _)| n)
            .collect()
    }

    /// Refuses `label` for the inbox `address` (when known) if another inbox
    /// has it as its label, address or local part.
    pub(crate) fn check_label(&self, label: &str, address: Option<&str>) -> Result<()> {
        let other = |i: &&Inbox| address.is_none_or(|a| !i.address.eq_ignore_ascii_case(a));
        let (why, other) = if let Some(holder) = self.label_holder(label, address) {
            ("is already the label of", holder)
        } else if let Some(i) = self.inboxes.iter().filter(other).find(|i| {
            i.address.eq_ignore_ascii_case(label) || i.local_part().eq_ignore_ascii_case(label)
        }) {
            ("would also match", i)
        } else {
            return Ok(());
        };
        Err(Error::Usage(format!(
            "label {label:?} {why} {}; choose another one",
            other.address
        )))
    }

    /// Another inbox (not `address`) whose label is `name`: the one `-i NAME`
    /// picks, so a new address with `name` before its `@` couldn't be
    /// picked by it.
    pub(crate) fn label_holder(&self, name: &str, address: Option<&str>) -> Option<&Inbox> {
        self.labeled(name)
            .into_iter()
            .map(|n| &self.inboxes[n])
            .find(|i| address.is_none_or(|a| !i.address.eq_ignore_ascii_case(a)))
    }

    /// Refuses `user` as the name (the part before `@`) of a new or
    /// logged-in address `address` when another inbox has it as its label.
    /// An address already stored passes: logging in to it again, or
    /// creating it again, adds no clash.
    pub(crate) fn check_user(&self, user: &str, address: Option<&str>) -> Result<()> {
        if address.is_some_and(|a| self.position(a).is_some()) {
            return Ok(());
        }
        let user = user.trim();
        match self.label_holder(user, address) {
            Some(other) => Err(Error::Usage(format!(
                "the name {user:?} is already the label of {}; choose another one",
                other.address
            ))),
            None => Ok(()),
        }
    }

    /// Index of the inbox a command should act on: the explicit one, else the
    /// active one, else the most recent.
    pub fn resolve(&self, explicit: Option<&str>) -> Result<usize> {
        if let Some(addr) = explicit {
            return match self.matches(addr).as_slice() {
                [one] => Ok(*one),
                [] => Err(Error::NotFound(format!("no stored inbox matches {addr:?}"))),
                several => Err(Error::Usage(format!(
                    "{addr:?} matches several inboxes ({}); use the full address",
                    several
                        .iter()
                        .map(|&n| self.inboxes[n].address.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))),
            };
        }
        if let Some(active) = &self.active {
            if let Some(n) = self.find(active) {
                return Ok(n);
            }
        }
        if self.inboxes.is_empty() {
            return Err(Error::NoInbox);
        }
        Ok(self.inboxes.len() - 1)
    }

    /// Stores `inbox` as the active one, replacing a stored inbox with the
    /// same address (keeping the messages read there). Its label moves to
    /// it from any other inbox that had it (`rotate --keep`), so a label
    /// picks one inbox.
    pub fn add(&mut self, mut inbox: Inbox) {
        if let Some(label) = &inbox.label {
            for other in &mut self.inboxes {
                if other
                    .label
                    .as_deref()
                    .is_some_and(|l| l.eq_ignore_ascii_case(label))
                {
                    other.label = None;
                }
            }
        }
        if let Some(old) = self
            .inboxes
            .iter_mut()
            .find(|i| i.address.eq_ignore_ascii_case(&inbox.address))
            && inbox.read.is_empty()
        {
            inbox.read = std::mem::take(&mut old.read);
        }
        self.inboxes
            .retain(|i| !i.address.eq_ignore_ascii_case(&inbox.address));
        self.active = Some(inbox.address.clone());
        self.inboxes.push(inbox);
    }

    pub fn remove(&mut self, n: usize) -> Inbox {
        let inbox = self.inboxes.remove(n);
        self.fix_active();
        inbox
    }

    /// Forgets expired inboxes and returns how many were dropped.
    pub fn prune_expired(&mut self) -> usize {
        let before = self.inboxes.len();
        self.inboxes.retain(|i| !i.is_expired());
        self.fix_active();
        before - self.inboxes.len()
    }

    /// If the active inbox is gone, the most recent one becomes active.
    fn fix_active(&mut self) {
        let gone = self
            .active
            .as_ref()
            .is_some_and(|act| !self.inboxes.iter().any(|i| &i.address == act));
        if gone {
            self.active = self.inboxes.last().map(|i| i.address.clone());
        }
    }
}

pub fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod {}", dir.display()))?;
    }
    Ok(())
}

pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    f.write_all(data)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inbox(addr: &str) -> Inbox {
        Inbox::new("mailtm", addr.into(), json!({"password":"pw"}))
    }

    #[test]
    fn round_trip_with_private_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("state"));
        assert!(store.load().unwrap().inboxes.is_empty());
        let (state, ()) = store.update(|s| s.add(inbox("a@x.com"))).unwrap();
        assert!(!state.inboxes.is_empty());

        let loaded = store.load().unwrap();
        assert_eq!(loaded.inboxes, state.inboxes);
        assert_eq!(loaded.active.as_deref(), Some("a@x.com"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(store.dir()), 0o700);
            assert_eq!(mode(&store.dir().join("state.json")), 0o600);
        }
    }

    #[test]
    fn resolves_explicit_label_active_and_latest() {
        let mut s = State::default();
        assert!(matches!(s.resolve(None), Err(Error::NoInbox)));
        s.add(inbox("one@x.com"));
        let mut two = inbox("two@y.com");
        two.label = Some("github".into());
        s.add(two);
        assert_eq!(s.resolve(None).unwrap(), 1);
        assert_eq!(s.resolve(Some("ONE@x.com")).unwrap(), 0);
        assert_eq!(s.resolve(Some("github")).unwrap(), 1);
        assert_eq!(s.resolve(Some("one")).unwrap(), 0);
        assert!(s.resolve(Some("nope")).is_err());

        s.active = Some("one@x.com".into());
        assert_eq!(s.resolve(None).unwrap(), 0);
        s.remove(0);
        assert_eq!(s.active.as_deref(), Some("two@y.com"));
    }

    #[test]
    fn a_label_must_pick_one_inbox() {
        let mut s = State::default();
        let mut mt = inbox("rapidpebble528@fldubai.com");
        mt.label = Some("mt1".into());
        s.add(mt);
        s.add(inbox("c1old20815@guerrillamailblock.com"));
        assert!(s.check_label("MT1", None).is_err());
        assert!(s.check_label("c1old20815", None).is_err());
        assert!(
            s.check_label("c1old20815@guerrillamailblock.com", None)
                .is_err()
        );
        assert!(s.check_label("shop", None).is_ok());
        // Its own label and address are fine (a login of a stored address).
        assert!(
            s.check_label("mt1", Some("rapidpebble528@fldubai.com"))
                .is_ok()
        );

        // A clash made by an older tmpmail is named, not "no inbox".
        let mut other = inbox("ufxrcbby@guerrillamailblock.com");
        other.label = Some("mt1".into());
        s.inboxes.push(other);
        let err = s.resolve(Some("mt1")).unwrap_err().to_string();
        assert!(err.contains("several inboxes"), "{err}");
        assert!(err.contains("ufxrcbby@guerrillamailblock.com"), "{err}");
        assert!(matches!(s.resolve(Some("nope")), Err(Error::NotFound(_))));
    }

    #[test]
    fn a_label_beats_another_inboxs_local_part() {
        let mut s = State::default();
        let mut gm = inbox("tvidfnke@guerrillamailblock.com");
        gm.label = Some("gm1".into());
        s.add(gm);
        // A provider handed out an address named like the label.
        s.add(inbox("gm1@guerrillamailblock.com"));
        assert_eq!(s.resolve(Some("gm1")).unwrap(), 0);
        assert_eq!(s.resolve(Some("GM1")).unwrap(), 0);
        assert_eq!(s.find("gm1"), Some(0));
        assert_eq!(s.resolve(Some("gm1@guerrillamailblock.com")).unwrap(), 1);
        // Chosen names are refused instead.
        let err = s.check_user("GM1", None).unwrap_err().to_string();
        assert!(
            err.contains("is already the label of tvidfnke@guerrillamailblock.com"),
            "{err}"
        );
        assert!(s.check_user("gm2", None).is_ok());
        // Logging in again to the labeled inbox itself is fine.
        let mut own = State::default();
        let mut x = inbox("gm1@x.com");
        x.label = Some("gm1".into());
        own.add(x);
        assert!(own.check_user("gm1", Some("gm1@x.com")).is_ok());
        assert!(own.check_user(" gm1 ", Some("gm1@y.com")).is_err());
        // An address stored by chance (or by an older tmpmail) can be
        // logged in to or created again.
        assert!(
            s.check_user("gm1", Some("GM1@guerrillamailblock.com"))
                .is_ok()
        );
        // A label is still refused when another inbox has it as its local
        // part, though the label would win.
        assert!(s.check_label("tvidfnke", None).is_err());
    }

    #[test]
    fn an_added_inbox_takes_its_label_from_the_old_one() {
        let mut s = State::default();
        let mut old = inbox("old@x.com");
        old.label = Some("github".into());
        s.add(old.clone());
        let mut new = inbox("new@x.com");
        new.label = Some("GitHub".into());
        s.add(new);
        assert_eq!(s.inboxes[0].label, None);
        assert_eq!(s.resolve(Some("github")).unwrap(), 1);
        // Re-adding an inbox with its own label keeps it.
        s.add(old.clone());
        assert_eq!(s.inboxes.last().unwrap().label.as_deref(), Some("github"));
    }

    #[test]
    fn update_keeps_changes_made_by_another_process() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        store.update(|s| s.add(inbox("a@x.com"))).unwrap();
        // A long-running process holds this snapshot...
        let stale = store.load().unwrap();
        // ...while another adds an inbox.
        store.update(|s| s.add(inbox("b@x.com"))).unwrap();
        // The first one updating its inbox must not drop b@x.com.
        let mut changed = stale.inboxes[0].clone();
        changed.label = Some("x".into());
        let (state, ()) = store
            .update(|s| {
                let n = s.position(&changed.address).unwrap();
                s.inboxes[n] = changed;
            })
            .unwrap();
        assert_eq!(state.inboxes.len(), 2);
    }

    #[test]
    fn re_adding_an_address_replaces_it() {
        let mut s = State::default();
        s.add(inbox("a@x.com"));
        s.add(inbox("A@x.com"));
        assert_eq!(s.inboxes.len(), 1);
    }

    #[test]
    fn re_adding_an_address_keeps_its_read_marks() {
        let mut s = State::default();
        let mut a = inbox("a@x.com");
        a.read = vec!["1".into()];
        s.add(a);
        s.add(inbox("A@x.com"));
        assert_eq!(s.inboxes[0].read, ["1"]);
    }
}
