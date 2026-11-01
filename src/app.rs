use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::mpsc::Sender;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::provider::{self, CreateOpts, Inbox, Provider, Summary};
use crate::render::{clean, clean_line};
use crate::store::{State, Store};

// Used by the CLI's commands (`commands.rs` takes these through `super::*`).
#[cfg(feature = "cli")]
use {
    crate::extract,
    crate::provider::{Capabilities, Message},
    crate::push::Poller,
    crate::render::{body_text, safe_filename, safe_html_document_inline},
    crate::system,
    regex::{Regex, RegexBuilder},
    std::sync::Arc,
    std::sync::atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "cli")]
mod commands;

/// How many read message ids each inbox remembers (`Inbox::read`).
const MAX_READ: usize = 500;

/// Options every command shares: the CLI's global flags.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "cli", derive(clap::Args))]
pub struct Global {
    /// Machine-readable JSON output
    #[cfg_attr(feature = "cli", arg(long, global = true))]
    pub json: bool,

    /// Inbox to use (address, local part or label); defaults to the active one
    #[cfg_attr(
        feature = "cli",
        arg(long, short = 'i', global = true, env = "TMPMAIL_INBOX")
    )]
    pub inbox: Option<String>,

    /// Disable colors (also honours NO_COLOR)
    #[cfg_attr(feature = "cli", arg(long, global = true))]
    pub no_color: bool,

    /// Only print essential output
    #[cfg_attr(feature = "cli", arg(long, short, global = true))]
    pub quiet: bool,

    /// Browser command for HTML and links (default: config `browser`, $BROWSER, then the system opener)
    #[cfg_attr(feature = "cli", arg(long, short = 'b', global = true))]
    pub browser: Option<String>,

    /// Clipboard command (default: config `clipboard_cmd`, else auto-detected)
    #[cfg_attr(feature = "cli", arg(long, global = true))]
    pub clipboard_cmd: Option<String>,
}

pub struct App {
    pub global: Global,
    pub config: Config,
    pub store: Store,
    pub state: State,
    color: bool,
    /// Built once per run so providers can reuse what they fetched.
    providers: HashMap<String, Box<dyn Provider>>,
    /// Where notes and warnings go instead of stderr (the TUI owns the screen).
    log: Option<Sender<String>>,
    /// Set from outside (the MCP server) to stop a running wait.
    #[cfg(feature = "cli")]
    cancel: Option<Arc<AtomicBool>>,
    /// The failing command has already told the user (and printed its one
    /// `--json` document), so the error isn't printed again.
    error_shown: bool,
}

/// What `--json` prints for an inbox. Credentials are never included.
#[derive(Serialize)]
pub struct InboxView<'a> {
    address: &'a str,
    provider: &'a str,
    label: Option<&'a str>,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    expired: bool,
    active: bool,
    private: bool,
}

/// Whether output gets colors: not asked to go without, a terminal on
/// stdout, and one that shows them (`ansi`, asked last: on Windows it turns
/// the console's escape-code processing on).
fn wants_color(global: &Global, no_color_env: bool, terminal: bool, ansi: fn() -> bool) -> bool {
    !global.no_color && !global.json && !no_color_env && terminal && ansi()
}

impl App {
    pub fn new(global: Global) -> Result<Self> {
        let config = Config::load()?;
        crate::http::set_timeout(config.http_timeout);
        let store = Store::new(Store::default_dir());
        let state = store.load()?;
        let color = wants_color(
            &global,
            std::env::var_os("NO_COLOR").is_some(),
            std::io::stdout().is_terminal(),
            #[cfg(feature = "cli")]
            system::ansi_colors,
            // The desktop app prints no colored output.
            #[cfg(not(feature = "cli"))]
            || false,
        );
        let app = App {
            global,
            config,
            store,
            state,
            color,
            providers: HashMap::new(),
            log: None,
            #[cfg(feature = "cli")]
            cancel: None,
            error_shown: false,
        };
        // A key from a newer tmpmail (or a typo) is reported, not fatal:
        // once per process on stderr (the MCP server builds an App per
        // call), and again in a front end's log once it has one.
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            for w in app.config.warnings() {
                app.note(&w);
            }
        });
        Ok(app)
    }

    /// An app with the default config and an empty state kept in `dir`,
    /// for tests that must not read the user's files.
    #[cfg(test)]
    pub(crate) fn for_test(dir: std::path::PathBuf) -> Self {
        App {
            global: Global::default(),
            config: Config::default(),
            store: Store::new(dir),
            state: State::default(),
            color: false,
            providers: HashMap::new(),
            log: None,
            #[cfg(feature = "cli")]
            cancel: None,
            error_shown: false,
        }
    }

    /// Uses `p` for its provider id instead of the built-in one.
    #[cfg(test)]
    pub(crate) fn set_provider(&mut self, p: Box<dyn Provider>) {
        self.providers.insert(p.id().to_string(), p);
    }

    /// Whether the error the last command returned was already shown to
    /// the user, with what did work (the message before a failed
    /// `wait --delete`, say), so it shouldn't be printed again: its exit
    /// code still applies.
    pub fn error_shown(&self) -> bool {
        self.error_shown
    }

    /// Sends notes and warnings to `log` instead of printing them.
    pub fn log_to(&mut self, log: Sender<String>) {
        for w in self.config.warnings() {
            let _ = log.send(clean_line(&w));
        }
        self.log_to_quietly(log);
    }

    /// Like [`App::log_to`], without the config warnings (another `App`
    /// sharing the log has sent them).
    pub(crate) fn log_to_quietly(&mut self, log: Sender<String>) {
        self.log = Some(log);
    }

    #[cfg(feature = "cli")]
    /// Makes waits give up with `Error::Cancelled` once `flag` is set.
    pub(crate) fn cancel_on(&mut self, flag: Arc<AtomicBool>) {
        self.cancel = Some(flag);
    }

    #[cfg(feature = "cli")]
    fn check_cancel(&self) -> Result<()> {
        match &self.cancel {
            Some(c) if c.load(Ordering::Relaxed) => Err(Error::Cancelled),
            _ => Ok(()),
        }
    }

    // ---------- output helpers ----------

    /// Wraps `s` in an SGR color code when colors are on. Pad before
    /// painting: the escape codes would count toward the width.
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color && !s.trim().is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    #[cfg(feature = "cli")]
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }

    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }

    #[cfg(feature = "cli")]
    fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }

    #[cfg(feature = "cli")]
    fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }

    #[cfg(feature = "cli")]
    fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }

    #[cfg(feature = "cli")]
    fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }

    #[cfg(feature = "cli")]
    /// Prints one JSON document, with invisible and control characters
    /// escaped ([`crate::render::escape_json_invisible`]).
    fn json_out(&self, v: &impl Serialize) {
        let text = serde_json::to_string_pretty(v).expect("serializable");
        println!("{}", crate::render::escape_json_invisible(&text));
    }

    /// Messages can quote provider responses, so they are cleaned like mail.
    pub(crate) fn note(&self, msg: &str) {
        if let Some(log) = &self.log {
            let _ = log.send(clean_line(msg));
        } else if !self.global.quiet && !self.global.json {
            // Colors are decided for stdout; stderr may go elsewhere.
            let msg = clean(msg);
            if std::io::stderr().is_terminal() {
                eprintln!("{}", self.dim(&msg));
            } else {
                eprintln!("{msg}");
            }
        }
    }

    fn warn(&self, msg: &str) {
        if let Some(log) = &self.log {
            let _ = log.send(clean_line(msg));
        } else {
            eprintln!("tmpmail: {}", clean_line(msg));
        }
    }

    pub fn view<'a>(&self, inbox: &'a Inbox) -> InboxView<'a> {
        InboxView {
            address: &inbox.address,
            provider: &inbox.provider,
            label: inbox.label.as_deref(),
            created_at: inbox.created_at,
            expires_at: inbox.expires_at,
            expired: inbox.is_expired(),
            active: self.state.active.as_deref() == Some(inbox.address.as_str()),
            private: provider::get(&inbox.provider)
                .map(|p| p.caps().private)
                .unwrap_or(true),
        }
    }

    // ---------- inbox plumbing ----------

    #[cfg(feature = "cli")]
    /// Runs `f` against the selected inbox, then persists any credential
    /// refresh the provider made.
    fn with_inbox<T>(
        &mut self,
        f: impl FnOnce(&dyn Provider, &mut Inbox) -> Result<T>,
    ) -> Result<T> {
        let n = self.state.resolve(self.global.inbox.as_deref())?;
        self.with_inbox_at(n, f)
    }

    pub fn with_inbox_at<T>(
        &mut self,
        n: usize,
        f: impl FnOnce(&dyn Provider, &mut Inbox) -> Result<T>,
    ) -> Result<T> {
        let mut inbox = self.state.inboxes[n].clone();
        let p = self.provider(&inbox.provider)?;
        let result = f(p, &mut inbox);
        if inbox != self.state.inboxes[n] {
            self.state.inboxes[n] = inbox.clone();
            // Merge into the file as it is now: another tmpmail may have
            // changed other inboxes. One dropped meanwhile stays dropped.
            // The read list is the file's: another tmpmail may have just
            // marked a message read in this inbox.
            self.store.update(|s| {
                if let Some(k) = s.position(&inbox.address) {
                    let read = std::mem::take(&mut s.inboxes[k].read);
                    s.inboxes[k] = inbox;
                    s.inboxes[k].read = read;
                }
            })?;
        }
        result
    }

    fn provider(&mut self, id: &str) -> Result<&dyn Provider> {
        if !self.providers.contains_key(id) {
            self.providers.insert(id.to_string(), provider::get(id)?);
        }
        Ok(self.providers[id].as_ref())
    }

    #[cfg(feature = "cli")]
    /// Inbox indexes a command should poll: all stored ones with
    /// `--all-inboxes`, otherwise just the selected one.
    fn targets(&self, all: bool) -> Result<Vec<usize>> {
        if !all {
            return Ok(vec![self.state.resolve(self.global.inbox.as_deref())?]);
        }
        if self.global.inbox.is_some() {
            return Err(Error::Usage(
                "--all-inboxes and --inbox can't be combined".into(),
            ));
        }
        if self.state.inboxes.is_empty() {
            return Err(Error::NoInbox);
        }
        Ok((0..self.state.inboxes.len()).collect())
    }

    #[cfg(feature = "cli")]
    fn current(&self) -> Result<&Inbox> {
        let n = self.state.resolve(self.global.inbox.as_deref())?;
        Ok(&self.state.inboxes[n])
    }

    /// The inbox's messages, newest first; those opened in tmpmail are
    /// marked seen even where the provider doesn't track it.
    pub fn sorted_list(p: &dyn Provider, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        let mut list = p.list(inbox)?;
        list.sort_by_key(|s| std::cmp::Reverse(s.date));
        if !inbox.read.is_empty() {
            let read: std::collections::HashSet<&str> =
                inbox.read.iter().map(String::as_str).collect();
            for s in &mut list {
                s.seen |= read.contains(s.id.as_str());
            }
        }
        Ok(list)
    }

    #[cfg(feature = "cli")]
    /// Message `id` of inbox `n` (a list number, id or prefix; the newest
    /// when absent) and its id as listed.
    pub(crate) fn message_at(&mut self, n: usize, id: Option<&str>) -> Result<(Message, String)> {
        self.with_inbox_at(n, |p, inbox| {
            let id = Self::resolve_id(p, inbox, id)?;
            Ok((p.get(inbox, &id)?, id))
        })
    }

    /// [`App::mark_read`], noting a failure instead of returning it: the
    /// message was shown either way.
    pub fn mark_read_noting(&mut self, n: usize, id: &str) {
        if let Err(e) = self.mark_read(n, id) {
            self.note(&format!("couldn't mark the message read: {e}"));
        }
    }

    /// Remembers in the state file that message `id` of inbox `n` was read,
    /// so the CLI, the TUI and the desktop app all show it read. Keeps the
    /// newest [`MAX_READ`] ids.
    pub fn mark_read(&mut self, n: usize, id: &str) -> Result<()> {
        if self.state.inboxes[n].read.iter().any(|r| r == id) {
            return Ok(());
        }
        let address = self.state.inboxes[n].address.clone();
        let (_, read) = self.store.update(|s| {
            let k = s.position(&address)?;
            let read = &mut s.inboxes[k].read;
            if !read.iter().any(|r| r == id) {
                read.push(id.to_string());
                let extra = read.len().saturating_sub(MAX_READ);
                read.drain(..extra);
            }
            Some(read.clone())
        })?;
        if let Some(read) = read {
            self.state.inboxes[n].read = read;
        }
        Ok(())
    }

    #[cfg(feature = "cli")]
    /// Accepts an exact id, a list number from `ls`, or a unique id prefix.
    pub(crate) fn resolve_id(
        p: &dyn Provider,
        inbox: &mut Inbox,
        id: Option<&str>,
    ) -> Result<String> {
        let list = Self::sorted_list(p, inbox)?;
        let Some(id) = id else {
            return list
                .first()
                .map(|s| s.id.clone())
                .ok_or_else(|| Error::NotFound(format!("no messages in {} yet", inbox.address)));
        };
        // A list number first: Guerrilla's welcome mail has the id "1", and
        // `read 1` must still mean the newest message.
        if let Ok(n) = id.parse::<usize>() {
            if (1..=list.len()).contains(&n) {
                return Ok(list[n - 1].id.clone());
            }
        }
        if list.iter().any(|s| s.id == id) {
            return Ok(id.to_string());
        }
        let hits: Vec<&Summary> = list.iter().filter(|s| s.id.starts_with(id)).collect();
        match hits.as_slice() {
            [one] => Ok(one.id.clone()),
            [] => Err(Error::NotFound(format!(
                "no message {id:?} in {}",
                inbox.address
            ))),
            _ => Err(Error::Usage(format!(
                "{id:?} matches several messages, use more characters"
            ))),
        }
    }

    #[cfg(feature = "cli")]
    /// Runs `f` on the message `id` resolves to in the selected inbox.
    fn with_message<T>(
        &mut self,
        id: Option<&str>,
        f: impl FnOnce(&dyn Provider, &mut Inbox, &str) -> Result<T>,
    ) -> Result<T> {
        self.with_inbox(|p, inbox| {
            let id = Self::resolve_id(p, inbox, id)?;
            f(p, inbox, &id)
        })
    }

    /// Creates an inbox with `explicit`, or else the first configured
    /// provider (other than `exclude`) that supports `opts` and works.
    pub fn create_with_fallback(
        &self,
        explicit: Option<&str>,
        exclude: Option<&str>,
        opts: &CreateOpts,
    ) -> Result<Inbox> {
        if opts.user.as_deref().is_some_and(|u| u.trim().is_empty()) {
            return Err(Error::Usage("the username is empty".into()));
        }
        if opts.domain.as_deref().is_some_and(|d| d.trim().is_empty()) {
            return Err(Error::Usage("the domain is empty".into()));
        }
        // A chosen name that is another inbox's label is refused before any
        // request: `-i NAME` would keep picking the labeled inbox.
        if let Some(user) = &opts.user {
            let address = opts.domain.as_ref().map(|d| format!("{user}@{d}"));
            self.state.check_user(user, address.as_deref())?;
        }
        if let Some(id) = explicit {
            let p = provider::get(id)?;
            let inbox = checked_address(p.as_ref(), p.create(opts)?)?;
            self.note_unused_password(p.as_ref(), opts);
            return Ok(inbox);
        }
        let mut candidates = Vec::new();
        for id in self.config.provider_order() {
            if exclude == Some(id.as_str()) {
                continue;
            }
            let p = provider::get(&id)?;
            let caps = p.caps();
            if opts.user.is_some() && !caps.custom_user
                || opts.domain.is_some() && !caps.choose_domain
            {
                continue;
            }
            candidates.push(p);
        }
        let (mut failures, mut unavailable) = (Vec::new(), Vec::new());
        if let Some(domain) = &opts.domain {
            check_left_out_domain(&candidates, domain)?;
            candidates = order_by_domain(candidates, domain, &mut unavailable);
        }
        // A password (maybe just TMPMAIL_PASSWORD in the environment) only
        // matters to providers with accounts: try those first, not only those.
        candidates.sort_by_key(|p| opts.password.is_some() && !p.caps().login);

        for p in candidates {
            match p.create(opts) {
                Ok(inbox) => {
                    let inbox = checked_address(p.as_ref(), inbox)?;
                    if !failures.is_empty() {
                        self.note(&format!("using {} instead", p.name()));
                    }
                    self.note_unused_password(p.as_ref(), opts);
                    return Ok(inbox);
                }
                Err(e) if e.is_provider_failure() => {
                    self.warn(&format!("{e}, trying the next provider"));
                    failures.push((p.id(), e));
                }
                // A requested domain may belong to a later provider.
                Err(e @ Error::DomainUnavailable { .. }) => unavailable.push(e.to_string()),
                Err(e) => return Err(e),
            }
        }
        if !failures.is_empty() {
            // Those that answered don't have the domain: only the
            // unreachable ones might, so name them, not "all providers".
            let down: Vec<(&str, &str)> = failures
                .iter()
                .filter_map(|(id, e)| match e {
                    Error::Unreachable { detail, .. } => Some((*id, detail.as_str())),
                    _ => None,
                })
                .collect();
            if let (Some(domain), false, true) = (
                &opts.domain,
                unavailable.is_empty(),
                down.len() == failures.len(),
            ) {
                return Err(unknown_domain_error(domain, &down, &unavailable));
            }
            let errors: Vec<String> = failures.iter().map(|(_, e)| e.to_string()).collect();
            return Err(Error::Unreachable {
                provider: "all providers".into(),
                detail: errors.join("; "),
            });
        }
        // Every provider was up but none offers that domain: a usage error.
        if !unavailable.is_empty() {
            return Err(Error::Usage(unavailable.join("; ")));
        }
        Err(Error::Usage(
            "no configured provider supports these options (see `tmpmail providers`)".into(),
        ))
    }

    /// Notes when a new inbox (its label set) got, by chance, a name that
    /// is another inbox's label: `-i NAME` keeps picking the labeled one.
    /// Returns the note, which is also printed as a warning (so `--json`
    /// and `-q` users see it too).
    pub(crate) fn note_label_clash(&self, inbox: &Inbox) -> Option<String> {
        let name = inbox.local_part();
        // Its own label moves to it when it is stored (`rotate`).
        if inbox
            .label
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case(name))
        {
            return None;
        }
        let other = self.state.label_holder(name, Some(&inbox.address))?;
        let note = format!(
            "{name:?} is the label of {}, so it still picks that inbox; use the full address for {}",
            other.address, inbox.address
        );
        self.warn(&note);
        Some(note)
    }

    fn note_unused_password(&self, p: &dyn Provider, opts: &CreateOpts) {
        if opts.password.is_some() && !p.caps().login {
            self.note(&format!(
                "{} has no accounts, the password is not used",
                p.name()
            ));
        }
    }

    #[cfg(feature = "cli")]
    pub(crate) fn clipboard_cmd(&self) -> Option<String> {
        self.global
            .clipboard_cmd
            .clone()
            .or_else(|| self.config.clipboard_cmd.clone())
    }

    #[cfg(feature = "cli")]
    pub(crate) fn browser(&self) -> Option<String> {
        self.global
            .browser
            .clone()
            .or_else(|| self.config.browser.clone())
    }

    #[cfg(feature = "cli")]
    /// Creates a fresh inbox to replace the selected one, with the same
    /// provider if it works and its label. Returns it and the old inbox:
    /// the caller stores and shows the new one, then calls
    /// [`App::retire`] with the old one, so it is never gone before the
    /// new address has been shown.
    pub(crate) fn replace_current(&mut self) -> Result<(Inbox, Inbox)> {
        let old = self.current()?.clone();
        // Prefer the same provider, fall back like `new` does.
        let opts = CreateOpts::default();
        let created = {
            let p = self.provider(&old.provider)?;
            p.create(&opts).and_then(|i| checked_address(p, i))
        };
        let mut inbox = match created {
            Ok(i) => i,
            Err(e) if e.is_provider_failure() => {
                self.warn(&format!("{e}, trying other providers"));
                self.create_with_fallback(None, Some(&old.provider), &opts)?
            }
            Err(e) => return Err(e),
        };
        inbox.label = old.label.clone();
        let _ = self.note_label_clash(&inbox);
        Ok((inbox, old))
    }

    #[cfg(feature = "cli")]
    /// Drops the inbox a rotation replaced by `new` (already stored and
    /// shown), unless `keep`; storing `new` has already taken its label.
    /// The rotation itself worked, so a failure here only warns.
    pub(crate) fn retire(&mut self, old: &Inbox, new: &Inbox, keep: bool) {
        // A provider that handed out the same address again: nothing to drop.
        if old.address.eq_ignore_ascii_case(&new.address) {
            return;
        }
        if keep {
            if let Some(label) = &old.label {
                self.note(&format!(
                    "label {label:?} moved to the new inbox; {} keeps none",
                    old.address
                ));
            }
            return;
        }
        if let Ok(p) = self.provider(&old.provider) {
            let _ = p.delete_inbox(&mut old.clone());
        }
        if let Err(e) = self.forget(&old.address) {
            self.warn(&format!("could not forget {}: {e}", old.address));
        }
    }

    /// Removes a stored inbox (by exact address) from the state file.
    pub fn forget(&mut self, address: &str) -> Result<()> {
        self.state = self
            .store
            .update(|s| {
                if let Some(k) = s.position(address) {
                    s.remove(k);
                }
            })?
            .0;
        Ok(())
    }
}

/// Refuses, with a hint, a domain that a provider the fallback won't try
/// says is its own (Maildrop's: left out because its inboxes are public,
/// and it can't be picked by domain), instead of asking every provider.
fn check_left_out_domain(candidates: &[Box<dyn Provider>], domain: &str) -> Result<()> {
    let probe = format!("x@{domain}");
    let owner = provider::all()
        .filter(|id| !candidates.iter().any(|c| c.id() == *id))
        .filter_map(|id| provider::get(id).ok())
        .find(|p| p.owns_address(&probe));
    let Some(p) = owner else {
        return Ok(());
    };
    let public = if p.caps().private {
        ""
    } else {
        ", whose inboxes are public, so it is only used when asked for"
    };
    Err(Error::Usage(format!(
        "{domain} belongs to {}{public}: choose the provider {} (`-p {}`)",
        p.name(),
        p.id(),
        p.id()
    )))
}

/// Asks every candidate for its domains at once and keeps those offering
/// `domain` first, then those that couldn't answer (they may be down). The
/// rest are left out and noted in `unavailable`, so `new -d` doesn't try each
/// provider in turn.
fn order_by_domain(
    candidates: Vec<Box<dyn Provider>>,
    domain: &str,
    unavailable: &mut Vec<String>,
) -> Vec<Box<dyn Provider>> {
    let lists: Vec<Result<Vec<String>>> = std::thread::scope(|s| {
        let lookups: Vec<_> = candidates
            .iter()
            .map(|p| {
                let id = p.id();
                s.spawn(move || provider::get(id)?.domains())
            })
            .collect();
        lookups
            .into_iter()
            .map(|l| l.join().expect("domain lookup panicked"))
            .collect()
    });
    let (mut offering, mut unknown) = (Vec::new(), Vec::new());
    for (p, list) in candidates.into_iter().zip(lists) {
        match list {
            Ok(l) if l.iter().any(|d| d == domain) => offering.push(p),
            Ok(l) => unavailable.push(provider::domain_unavailable(p.id(), domain, &l).to_string()),
            Err(_) => unknown.push(p),
        }
    }
    offering.extend(unknown);
    offering
}

/// Rejects an address a provider returned that couldn't be a real one, such
/// as one carrying terminal escapes: it is printed and stored as is.
pub(crate) fn checked_address(p: &dyn Provider, inbox: Inbox) -> Result<Inbox> {
    let a = &inbox.address;
    // `clean_line` drops control and bidi characters and folds whitespace.
    let plausible = a.split('@').count() == 2
        && !a.starts_with('@')
        && !a.ends_with('@')
        && !a.contains(char::is_whitespace)
        && clean_line(a) == *a;
    if plausible {
        Ok(inbox)
    } else {
        Err(Error::BadResponse {
            provider: p.id().to_string(),
            detail: format!("invalid address {:?}", clean_line(a)),
            status: None,
        })
    }
}

#[cfg(feature = "cli")]
pub(crate) fn compile(pattern: &str) -> Result<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| Error::Usage(format!("invalid regex {pattern:?}: {e}")))
}

#[cfg(feature = "cli")]
pub(crate) fn compile_plain(pattern: &str) -> Result<Regex> {
    Regex::new(pattern).map_err(|e| Error::Usage(format!("invalid regex {pattern:?}: {e}")))
}

#[cfg(feature = "cli")]
pub(crate) fn message_body(m: &Message) -> String {
    body_text(m.text.as_deref(), m.html.as_deref(), 200)
}

#[cfg(feature = "cli")]
pub(crate) fn message_links(m: &Message) -> Vec<String> {
    extract::links(
        m.text.as_deref().unwrap_or_default(),
        m.html.as_deref().unwrap_or_default(),
    )
}

#[cfg(feature = "cli")]
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(feature = "cli")]
pub(crate) fn relative(t: DateTime<Utc>) -> String {
    let secs = (Utc::now() - t).num_seconds();
    let (future, secs) = (secs < 0, secs.unsigned_abs());
    let text = match secs {
        0..=59 => {
            return if future {
                "in <1m".into()
            } else {
                "just now".into()
            };
        }
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    };
    if future {
        format!("in {text}")
    } else {
        format!("{text} ago")
    }
}

/// The error for a domain no reachable provider offers while the others
/// (`down`: id and reason) couldn't be reached: those might have it, so it
/// names them (exit 3) and adds what the others answered after the
/// parenthesis.
fn unknown_domain_error(domain: &str, down: &[(&str, &str)], unavailable: &[String]) -> Error {
    let ids: Vec<&str> = down.iter().map(|(id, _)| *id).collect();
    let why: Vec<String> = down
        .iter()
        .map(|(id, detail)| match ids.len() {
            1 => detail.to_string(),
            _ => format!("{id}: {detail}"),
        })
        .collect();
    Error::Unreachable {
        provider: ids.join(", "),
        detail: why.join("; "),
    }
    .with_note(format!(
        "the other providers don't offer {domain:?}: {}",
        unavailable.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;
    use serde_json::Value;

    /// Lists messages "1" and "2", neither seen at the provider; a listing
    /// renews the credential, as a token refresh would.
    struct Unseen;

    impl Provider for Unseen {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn website(&self) -> &'static str {
            ""
        }
        fn caps(&self) -> provider::Capabilities {
            unimplemented!()
        }
        fn domains(&self) -> Result<Vec<String>> {
            unimplemented!()
        }
        fn create(&self, _: &CreateOpts) -> Result<Inbox> {
            unimplemented!()
        }
        fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
            inbox.credential =
                Value::String(format!("{}+", inbox.credential.as_str().unwrap_or("")));
            Ok(["1", "2"]
                .map(|id| Summary {
                    id: id.into(),
                    ..Default::default()
                })
                .to_vec())
        }
        fn get(&self, _: &mut Inbox, id: &str) -> Result<Message> {
            Ok(Message {
                id: id.into(),
                ..Default::default()
            })
        }
    }

    fn app_with_inbox(dir: &std::path::Path) -> App {
        let mut app = App::for_test(dir.to_path_buf());
        app.set_provider(Box::new(Unseen));
        let inbox = Inbox::new("fake", "a@x.com".into(), Value::String("t".into()));
        app.state = app.store.update(|s| s.add(inbox)).unwrap().0;
        app
    }

    fn seen(app: &mut App) -> Vec<bool> {
        app.with_inbox_at(0, App::sorted_list)
            .unwrap()
            .iter()
            .map(|s| s.seen)
            .collect()
    }

    #[test]
    fn an_opened_message_is_read_everywhere() {
        let dir = tempfile::tempdir().unwrap();
        let mut gui = app_with_inbox(dir.path());
        // Another tmpmail (the CLI) with the state as it was.
        let mut cli = app_with_inbox(dir.path());
        cli.state = cli.store.load().unwrap();

        assert_eq!(seen(&mut gui), [false, false]);
        gui.mark_read(0, "1").unwrap();
        assert_eq!(seen(&mut gui), [true, false]);
        assert_eq!(cli.store.load().unwrap().inboxes[0].read, ["1"]);

        // The CLI's stale copy saves a new credential: the mark stays.
        assert_eq!(seen(&mut cli), [false, false], "not reloaded yet");
        let file = cli.store.load().unwrap();
        assert_eq!(file.inboxes[0].read, ["1"]);
        assert!(file.inboxes[0].credential.as_str().unwrap().ends_with('+'));
        cli.state = file;
        assert_eq!(seen(&mut cli), [true, false]);
    }

    #[test]
    fn the_read_list_keeps_the_newest_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_inbox(dir.path());
        for n in 0..MAX_READ + 2 {
            app.mark_read(0, &n.to_string()).unwrap();
        }
        app.mark_read(0, "5").unwrap();
        let read = &app.store.load().unwrap().inboxes[0].read;
        assert_eq!(read.len(), MAX_READ);
        assert_eq!(read[0], "2");
        assert_eq!(read.last().map(String::as_str), Some("501"));
        assert_eq!(*read, app.state.inboxes[0].read);
    }

    #[test]
    fn colors_need_a_terminal_that_shows_them() {
        fn yes() -> bool {
            true
        }
        fn no() -> bool {
            false
        }
        fn unasked() -> bool {
            panic!("the console is only set up for a terminal")
        }
        let plain = Global::default();
        assert!(wants_color(&plain, false, true, yes));
        // Escape-code processing couldn't be turned on (a Windows console).
        assert!(!wants_color(&plain, false, true, no));
        assert!(!wants_color(&plain, false, false, unasked));
        assert!(!wants_color(&plain, true, true, yes));
        let no_color = Global {
            no_color: true,
            ..Global::default()
        };
        assert!(!wants_color(&no_color, false, true, unasked));
        let json = Global {
            json: true,
            ..Global::default()
        };
        assert!(!wants_color(&json, false, true, unasked));
    }

    #[cfg(feature = "cli")]
    #[test]
    fn relative_times() {
        let now = Utc::now();
        assert_eq!(relative(now), "just now");
        assert_eq!(relative(now - chrono::Duration::minutes(5)), "5m ago");
        assert_eq!(
            relative(now + chrono::Duration::minutes(30) + chrono::Duration::seconds(5)),
            "in 30m"
        );
        assert_eq!(relative(now - chrono::Duration::days(3)), "3d ago");
    }

    #[test]
    fn an_unknown_domain_names_the_unreachable_provider_outside_its_parenthesis() {
        let down = |id| (id, "connection failed: refused");
        let unavailable = vec![
            "mailtm: domain \"x.example\" not available, choose one of: a.com".to_string(),
            "dropmail: domain \"x.example\" not available, choose one of: b.com".to_string(),
        ];
        let e = unknown_domain_error("x.example", &[down("mailgw")], &unavailable);
        assert_eq!(
            e.to_string(),
            "mailgw: unreachable (connection failed: refused); the other providers don't offer \
             \"x.example\": mailtm: domain \"x.example\" not available, choose one of: a.com; \
             dropmail: domain \"x.example\" not available, choose one of: b.com"
        );
        assert_eq!((e.kind(), e.exit_code()), ("unreachable", 3));
        let two = [down("mailgw"), down("mailtm")];
        let e = unknown_domain_error("x.example", &two, &unavailable[1..]);
        assert!(
            e.to_string().starts_with(
                "mailgw, mailtm: unreachable (mailgw: connection failed: refused; \
                 mailtm: connection failed: refused); the other"
            ),
            "{e}"
        );
    }

    #[test]
    fn domains_and_names_are_checked_before_any_request() {
        let app = App::for_test(std::env::temp_dir());
        let new = |user: Option<&str>, domain: Option<&str>| {
            let opts = CreateOpts {
                user: user.map(String::from),
                domain: domain.map(String::from),
                password: None,
            };
            app.create_with_fallback(None, None, &opts)
                .unwrap_err()
                .to_string()
        };
        let maildrop = new(Some("me"), Some("maildrop.cc"));
        assert!(maildrop.contains("`-p maildrop`"), "{maildrop}");
        let upper = new(None, Some("MailDrop.cc"));
        assert!(upper.contains("`-p maildrop`"), "{upper}");
        assert!(maildrop.contains("public"), "{maildrop}");
        assert_eq!(new(Some("foo"), Some("")), "the domain is empty");
        assert_eq!(new(Some(" "), Some("x.com")), "the username is empty");
    }

    #[test]
    fn provider_addresses_are_validated() {
        let p = provider::get("mailtm").unwrap();
        let ok = |a: &str| {
            checked_address(p.as_ref(), Inbox::new("mailtm", a.into(), Value::Null)).is_ok()
        };
        assert!(ok("otter1@maxxspace.com"));
        assert!(!ok("x\u{1b}]52;c;AAAA\u{7}@evil.com"));
        assert!(!ok("a\u{202e}b@x.com"));
        assert!(!ok("no-at-sign"));
        assert!(!ok("a b@x.com"));
    }

    #[cfg(feature = "cli")]
    #[test]
    fn truncation() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
