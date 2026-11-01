//! The CLI's commands, on top of the core in `app`.

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::error::unsupported;

use super::*;
use crate::cli::*;

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// How often a cancellable wait checks its flag while sleeping.
const CANCEL_CHECK: Duration = Duration::from_millis(250);

/// One line of `tmpmail providers`.
#[derive(Serialize)]
struct ProviderRow {
    id: &'static str,
    name: &'static str,
    website: &'static str,
    in_fallback_order: Option<usize>,
    capabilities: Capabilities,
    status: Option<Health>,
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl App {
    pub fn run(&mut self, command: Command) -> Result<()> {
        match command {
            Command::New(a) => self.new_inbox(a),
            Command::Ls(a) => self.ls(a),
            Command::Read(a) => self.read(a),
            Command::Wait(a) => self.wait_cmd(a),
            Command::Otp(a) => self.otp(a),
            Command::Links(a) => self.links(a),
            Command::Watch(a) => self.watch(a),
            Command::Attachments(a) => self.attachments(a),
            Command::Save(a) => self.save(a),
            Command::Delete(a) => self.delete(a),
            Command::Whoami => self.whoami(),
            Command::Copy => self.copy(),
            Command::Qr => self.qr(),
            Command::Domains(a) => self.domains(a),
            Command::Inboxes(a) => self.inboxes(a),
            Command::Use { address } => self.use_inbox(&address),
            Command::Login(a) => self.login(a),
            Command::Drop(a) => self.drop_inbox(a),
            Command::Rotate(a) => self.rotate(a),
            Command::Providers(a) => self.providers(a),
            Command::Tui => crate::tui::run(self),
            Command::Mcp => crate::mcp::serve(self),
            Command::Completions { .. } | Command::Man { .. } => unreachable!("handled in main"),
        }
    }

    fn fetch(&mut self, msg: &MessageRef) -> Result<Message> {
        self.with_message(msg.id.as_deref(), |p, inbox, id| p.get(inbox, id))
    }

    /// Stores `inbox` as the active one and prints it, then copies it if
    /// asked. A failed copy only warns: the inbox exists and its address
    /// was printed. Returns whether it was copied.
    fn activate(&mut self, inbox: &Inbox, copy: bool) -> Result<bool> {
        let added = inbox.clone();
        self.state = self.store.update(|s| s.add(added))?.0;
        if self.global.json {
            self.json_out(&self.view(inbox));
        } else {
            println!("{}", clean_line(&inbox.address));
        }
        Ok(copy && self.copy_new_address(&inbox.address))
    }

    /// Copies a new address, warning when that fails.
    fn copy_new_address(&self, address: &str) -> bool {
        let _ = std::io::stdout().flush();
        match system::copy(address, self.clipboard_cmd().as_deref()) {
            Ok(()) => true,
            Err(e) => {
                self.warn(&format!("could not copy the address: {e:#}"));
                false
            }
        }
    }

    /// Warns that anyone can read an inbox at `p` (Maildrop).
    fn warn_if_public(&self, p: &dyn Provider) {
        if !p.caps().private {
            self.warn(&format!(
                "warning: {} inboxes are public, anyone who knows this address can read its mail",
                p.name()
            ));
        }
    }

    // ---------- commands ----------

    fn new_inbox(&mut self, a: NewArgs) -> Result<()> {
        let (mut user, mut domain) = (a.user, a.domain);
        if let Some(addr) = &a.address {
            let (u, d) = addr
                .split_once('@')
                .ok_or_else(|| Error::Usage(format!("{addr:?} is not an email address")))?;
            user = Some(u.to_string());
            domain = Some(d.to_string());
        }
        if let Some(label) = &a.label {
            let address = match (&user, &domain) {
                (Some(u), Some(d)) => Some(format!("{u}@{d}")),
                _ => None,
            };
            self.state.check_label(label, address.as_deref())?;
        }
        let opts = CreateOpts {
            user,
            domain,
            password: a.password,
        };
        let mut inbox = self.create_with_fallback(a.provider.as_deref(), None, &opts)?;
        // Creating a stored address again keeps its label, as `login` does.
        inbox.label = a.label.or_else(|| {
            let n = self.state.position(&inbox.address)?;
            self.state.inboxes[n].label.clone()
        });
        let _ = self.note_label_clash(&inbox);
        let p = provider::get(&inbox.provider)?;
        let copied = self.activate(&inbox, a.copy)?;
        if !self.global.json {
            self.warn_if_public(p.as_ref());
            let mut info = format!("via {} ({})", p.name(), p.caps().lifetime);
            if copied {
                info.push_str(", copied to clipboard");
            }
            self.note(&info);
        }
        Ok(())
    }

    fn ls(&mut self, a: LsArgs) -> Result<()> {
        let targets = self.targets(a.all_inboxes)?;
        let mut sections = Vec::new();
        for n in targets {
            let address = self.state.inboxes[n].address.clone();
            let created = self.state.inboxes[n].created_at;
            let result = self.with_inbox_at(n, |p, inbox| Self::sorted_list(p, inbox));
            match result {
                Ok(list) => sections.push((address, created, Ok(list))),
                // With -a, one dead inbox must not hide the others.
                Err(e) if a.all_inboxes => sections.push((address, created, Err(e))),
                Err(e) => return Err(e),
            }
        }

        if self.global.json {
            if !a.all_inboxes {
                let (address, created, list) = sections.remove(0);
                self.json_out(&json!({
                    "address": address,
                    "created_at": created,
                    "messages": list?,
                }));
            } else {
                let all: Vec<Value> = sections
                    .iter()
                    .map(|(address, created, r)| match r {
                        Ok(list) => json!({
                            "address": address,
                            "created_at": created,
                            "messages": list,
                        }),
                        Err(e) => json!({
                            "address": address,
                            "created_at": created,
                            "messages": [],
                            "error": { "kind": e.kind(), "message": e.to_string() },
                        }),
                    })
                    .collect();
                self.json_out(&json!({ "inboxes": all }));
            }
            return self.unless_all_failed(sections);
        }

        let total = sections.len();
        for (k, (address, created, result)) in sections.iter().enumerate() {
            let created = *created;
            let inbox = self.state.position(address).map(|n| self.inbox_option(n));
            if k > 0 {
                println!();
            }
            if !self.global.quiet || total > 1 {
                let since = format!(
                    "created {} ({})",
                    created
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M"),
                    relative(created),
                );
                println!(
                    "{}  {}",
                    self.bold(&format!("[ Inbox for {} ]", clean_line(address))),
                    self.dim(&since),
                );
            }
            match result {
                Ok(list) if list.is_empty() => {
                    if !self.global.quiet {
                        println!(
                            "No messages yet. `tmpmail {}wait` blocks until one arrives.",
                            inbox.unwrap_or_default()
                        );
                    }
                }
                Ok(list) => self.print_list(list),
                Err(e) => println!(
                    "{}",
                    self.dim(&format!("  unavailable: {}", clean_line(&e.to_string())))
                ),
            }
        }
        let any_read = sections.iter().any(|(_, _, r)| r.is_ok());
        if total > 1 && any_read && !self.global.quiet {
            self.note("\nread one with: tmpmail -i ADDRESS read N");
        }
        self.unless_all_failed(sections)
    }

    /// `ls -a` fails when no inbox could be read, with the first one's
    /// error for the exit code. Each error is in the listing already, so
    /// it isn't printed again.
    fn unless_all_failed(
        &mut self,
        sections: Vec<(String, DateTime<Utc>, Result<Vec<Summary>>)>,
    ) -> Result<()> {
        if sections.iter().any(|(_, _, r)| r.is_ok()) {
            return Ok(());
        }
        match sections.into_iter().find_map(|(_, _, r)| r.err()) {
            Some(e) => {
                self.error_shown = true;
                Err(e)
            }
            None => Ok(()),
        }
    }

    fn print_list(&self, list: &[Summary]) {
        let width = term_width();
        for (n, s) in list.iter().enumerate() {
            let att = if s.has_attachments { " [att]" } else { "" };
            let when = s.date.map(relative).unwrap_or_default();
            let from = truncate(&clean_line(&s.from), 28);
            let room = width.saturating_sub(4 + 10 + 30 + att.len()).max(20);
            let subject = truncate(&clean_line(&s.subject), room);
            // Unread mail stands out; read mail is plain.
            let (marker, subject) = if s.seen {
                (" ".to_string(), subject)
            } else {
                (self.cyan("*"), self.bold(&subject))
            };
            println!(
                "{:>3}{marker} {}  {from:<28}  {subject}{}",
                n + 1,
                self.dim(&format!("{when:>8}")),
                self.yellow(att),
            );
        }
    }

    fn read(&mut self, a: ReadArgs) -> Result<()> {
        let n = self.state.resolve(self.global.inbox.as_deref())?;
        if a.raw {
            let (raw, id) = self.with_inbox_at(n, |p, inbox| {
                let id = Self::resolve_id(p, inbox, a.msg.id.as_deref())?;
                Ok((p.raw(inbox, &id)?, id))
            })?;
            self.mark_read_noting(n, &id);
            let mut out = std::io::stdout();
            // Exact bytes when piped; a sender's escape sequences must not
            // reach a terminal, though.
            if out.is_terminal() {
                write!(out, "{}", clean(&String::from_utf8_lossy(&raw)))
            } else {
                out.write_all(&raw)
            }
            .map_err(anyhow::Error::from)?;
            return Ok(());
        }
        // Marked by its id as listed (a message's own may be spelled
        // differently).
        let (msg, id) = self.message_at(n, a.msg.id.as_deref())?;
        self.mark_read_noting(n, &id);
        if a.html {
            let inline = self.inline_images(n, &msg);
            let view = self.open_html(&msg, a.images, &inline)?;
            if let Some(note) = &view.note {
                self.warn(note);
            }
            self.note(&format!("opened {}", view.path.display()));
            return Ok(());
        }
        if self.global.json {
            self.json_out(&msg);
            return Ok(());
        }
        let text = self.render_message(&msg, n);
        let rows = system::term_size().map_or(0, |(_, rows)| rows);
        // Leave a row for the shell prompt.
        let fits = rows == 0 || text.lines().count() < rows;
        if a.no_pager || fits || !std::io::stdout().is_terminal() {
            print!("{text}");
        } else if let Err(e) = system::page(&text, self.config.pager.as_deref()) {
            self.warn(&format!("pager failed ({e:#}), printing instead"));
            print!("{text}");
        }
        self.note_html_version(&msg, n);
        Ok(())
    }

    /// Message `msg` of inbox `n`, as text.
    fn print_message(&self, msg: &Message, n: usize) {
        if self.global.json {
            self.json_out(msg);
            return;
        }
        print!("{}", self.render_message(msg, n));
        self.note_html_version(msg, n);
    }

    fn note_html_version(&self, msg: &Message, n: usize) {
        if msg.html.is_some() && !self.global.quiet {
            // The note goes to stderr; keep it after the body when piped.
            let _ = std::io::stdout().flush();
            self.note(&format!(
                "\n(HTML version: tmpmail {}read {} --html)",
                self.inbox_option(n),
                clean_line(&msg.id)
            ));
        }
    }

    /// `-i ADDRESS ` when inbox `n` isn't the one commands use by default
    /// (it came from `-i`, `TMPMAIL_INBOX` or `-a`), so a printed hint
    /// names it; else nothing.
    fn inbox_option(&self, n: usize) -> String {
        if self.state.resolve(None).is_ok_and(|d| d == n) {
            String::new()
        } else {
            let address = clean_line(&self.state.inboxes[n].address);
            let plain = |c: char| c.is_ascii_alphanumeric() || "@._+-".contains(c);
            if address.chars().all(plain) {
                format!("-i {address} ")
            } else {
                format!("-i '{}' ", address.replace('\'', r"'\''"))
            }
        }
    }

    /// Headers and text body of message `msg` of inbox `n`, ready to print
    /// or page.
    fn render_message(&self, msg: &Message, n: usize) -> String {
        let width = term_width().min(100);
        let mut out = String::new();
        let mut header = |k: &str, v: String| {
            out.push_str(&format!("{} {v}\n", self.bold(&format!("{k:<8}"))));
        };
        header("From:", clean_line(&msg.from));
        header("To:", clean_line(&msg.to.join(", ")));
        header("Subject:", self.bold(&clean_line(&msg.subject)));
        if let Some(d) = msg.date {
            let local = d.with_timezone(&chrono::Local);
            header(
                "Date:",
                format!("{} ({})", local.format("%Y-%m-%d %H:%M:%S"), relative(d)),
            );
        }
        if !msg.attachments.is_empty() {
            let names: Vec<String> = msg.attachments.iter().map(|a| a.filename.clone()).collect();
            header(
                "Attach:",
                format!(
                    "{} {}",
                    self.yellow(&clean_line(&names.join(", "))),
                    self.dim(&format!(
                        "(tmpmail {}save {})",
                        self.inbox_option(n),
                        clean_line(&msg.id)
                    ))
                ),
            );
        }
        out.push('\n');
        let body = body_text(msg.text.as_deref(), msg.html.as_deref(), width);
        out.push_str(&clean(body.trim_end()));
        out.push('\n');
        out
    }

    /// The inline images (`cid:`) of a message's HTML as `data:` URLs, by
    /// cid: the attachment the provider lists under that name, else the
    /// part with that `Content-ID` in the raw source (mail.tm lists inline
    /// images by file name only), read once for all of them. Raster images
    /// only, up to [`provider::MAX_INLINE_IMAGE`] each; one that can't be
    /// fetched is left out. A listed attachment is fetched once however
    /// many names point at it, at most [`provider::MAX_INLINE_CIDS`] names
    /// are looked up, and at most [`provider::MAX_INLINE_TOTAL`] bytes of
    /// images are put in the copy.
    fn inline_images(&mut self, n: usize, msg: &Message) -> HashMap<String, String> {
        let mut out = HashMap::new();
        let Some(html) = &msg.html else {
            return out;
        };
        let id = self.state.inboxes[n].provider.clone();
        let can_raw = self.provider(&id).is_ok_and(|p| p.caps().raw);
        let mut from_source: Option<HashMap<String, (String, Vec<u8>)>> = None;
        // By attachment: its `data:` URL and size, or None if it can't be
        // shown, so other names for it reuse that.
        let mut fetched: HashMap<usize, Option<(std::sync::Arc<String>, usize)>> = HashMap::new();
        let mut total = 0;
        let fits = |len: usize, total: usize| {
            len <= provider::MAX_INLINE_IMAGE && total + len <= provider::MAX_INLINE_TOTAL
        };
        let cids = crate::render::cid_images(html);
        for cid in cids.into_iter().take(provider::MAX_INLINE_CIDS) {
            let listed = provider::inline_attachment(msg, &cid).and_then(|(att, mime)| {
                let i = msg.attachments.iter().position(|a| std::ptr::eq(a, att))?;
                let listed_size = att.size.and_then(|s| usize::try_from(s).ok());
                fetched
                    .entry(i)
                    .or_insert_with(|| {
                        if listed_size.is_some_and(|len| !fits(len, total)) {
                            return None;
                        }
                        let bytes = self
                            .with_inbox_at(n, |p, inbox| p.attachment(inbox, msg, att))
                            .ok()
                            .filter(|b| b.len() <= provider::MAX_INLINE_IMAGE)?;
                        let url = format!("data:{mime};base64,{}", base64(&bytes));
                        Some((std::sync::Arc::new(url), bytes.len()))
                    })
                    .clone()
            });
            let image = match listed {
                Some(image) => Some(image),
                None if can_raw => from_source
                    .get_or_insert_with(|| {
                        self.with_inbox_at(n, |p, inbox| p.raw(inbox, &msg.id))
                            .map(|raw| provider::inline_images(&raw))
                            .unwrap_or_default()
                    })
                    .remove(&cid)
                    .filter(|(_, b)| b.len() <= provider::MAX_INLINE_IMAGE)
                    .map(|(mime, bytes)| {
                        let url = format!("data:{mime};base64,{}", base64(&bytes));
                        (std::sync::Arc::new(url), bytes.len())
                    }),
                None => None,
            };
            if let Some((url, len)) = image.filter(|(_, len)| fits(*len, total)) {
                total += len;
                out.insert(cid, url.as_str().to_string());
            }
        }
        out
    }

    /// Writes the message's sanitized HTML to a private file, opens it in
    /// the browser and returns the file. `inline` has the `data:`
    /// URLs of its `cid:` images ([`App::inline_images`]).
    pub(crate) fn open_html(
        &self,
        msg: &Message,
        images: bool,
        inline: &HashMap<String, String>,
    ) -> Result<system::View> {
        let html = match (&msg.html, &msg.text) {
            (Some(h), _) => h.clone(),
            (None, Some(t)) => format!("<pre>{}</pre>", crate::render::escape_html(t)),
            (None, None) => String::new(),
        };
        let doc = safe_html_document_inline(&msg.subject, &html, images, inline);
        let name = format!("{}.html", safe_filename(&msg.id, "message"));
        let browser = self.browser();
        let view = system::write_view(self.store.dir(), browser.as_deref(), &name, &doc)?;
        system::open(&view.path.to_string_lossy(), browser.as_deref())?;
        Ok(view)
    }

    // ----- wait / otp / links -----

    fn wait_cmd(&mut self, a: WaitArgs) -> Result<()> {
        let otp_re = a.otp_regex.as_deref().map(compile_plain).transpose()?;
        let (want_otp, want_link) = (a.otp, a.link);
        // `Some(found)` accepts the message; `found` is the code or link asked for.
        let (n, msg, text, found) = self.wait_for_where(&a.filter, a.delete, |m, body| {
            if want_otp {
                extract::otp(&m.subject, body, otp_re.as_ref()).map(Some)
            } else if want_link {
                message_links(m).into_iter().next().map(Some)
            } else {
                Some(None)
            }
        })?;

        let inbox = self.state.inboxes[n].address.clone();
        if let Some(code) = found.as_deref().filter(|_| want_otp) {
            self.print_code(code, &msg, &text, otp_re.is_some(), &inbox);
        } else if let Some(link) = found {
            if self.global.json {
                self.json_out(&json!({
                    "link": link,
                    "message_id": msg.id,
                    "inbox": inbox,
                }));
            } else {
                println!("{}", clean_line(&link));
            }
        } else if self.global.json {
            let mut v = serde_json::to_value(&msg).expect("serializable");
            v["inbox"] = json!(inbox);
            self.json_out(&v);
        } else {
            self.print_message(&msg, n);
        }
        if a.delete {
            let id = msg.id.clone();
            // The message is out, so the failure is told on stderr only:
            // `--json` keeps one document on stdout, and the exit code
            // still says the message is left in the inbox.
            if let Err(e) = self.with_inbox_at(n, |p, inbox| p.delete_message(inbox, &id)) {
                let _ = std::io::stdout().flush();
                self.warn(&format!("could not delete the message: {e}"));
                self.error_shown = true;
                return Err(e);
            }
        }
        Ok(())
    }

    /// `body` is the message text the code came from. Heuristic candidates
    /// are listed only when no custom regex picked the code.
    fn print_code(&self, code: &str, msg: &Message, body: &str, custom: bool, inbox: &str) {
        if self.global.json {
            let candidates = if custom {
                Vec::new()
            } else {
                extract::otp_candidates(&msg.subject, body)
            };
            self.json_out(&json!({
                "code": code,
                "candidates": candidates,
                "inbox": inbox,
                "message_id": msg.id,
                "from": msg.from,
                "subject": msg.subject,
            }));
        } else {
            println!("{}", clean_line(code));
        }
    }

    /// Polls the target inbox(es) until a message passes the filter and
    /// `accept` (given the message and its body text) returns `Some`, or
    /// times out. Returns the inbox index, the message, its body text and what
    /// `accept` found.
    pub(crate) fn wait_for<T>(
        &mut self,
        f: &WaitFilter,
        accept: impl Fn(&Message, &str) -> Option<T>,
    ) -> Result<(usize, Message, String, T)> {
        self.wait_for_where(f, false, accept)
    }

    /// [`App::wait_for`]; with `deleting` only on inboxes whose provider can
    /// delete messages (`wait --delete`), checked before waiting. With
    /// `--all-inboxes` the others are skipped with a note.
    fn wait_for_where<T>(
        &mut self,
        f: &WaitFilter,
        deleting: bool,
        accept: impl Fn(&Message, &str) -> Option<T>,
    ) -> Result<(usize, Message, String, T)> {
        let from = f.from.as_deref().map(compile).transpose()?;
        let subject = f.subject.as_deref().map(compile).transpose()?;
        let body = f.body.as_deref().map(compile).transpose()?;
        let timeout = f.timeout.unwrap_or_else(|| self.config.timeout());
        let interval = self.config.poll_interval(f.interval);
        let deadline = Instant::now() + Duration::from_secs(timeout);
        let all = f.all_inboxes;
        let mut targets = self.targets(all)?;
        if deleting {
            let mut refused = None;
            for n in targets.clone() {
                let inbox = &self.state.inboxes[n];
                let (id, address) = (inbox.provider.clone(), inbox.address.clone());
                let can = match self.provider(&id) {
                    Ok(p) => p.caps().delete_message,
                    Err(e) if !all => return Err(e),
                    Err(_) => false,
                };
                if !can {
                    let e = unsupported(&id, &format!("deleting messages (--delete, {address})"));
                    if all {
                        self.note(&format!("skipping {address}: {e}"));
                    }
                    targets.retain(|&t| t != n);
                    refused.get_or_insert(e);
                }
            }
            if let (true, Some(e)) = (targets.is_empty(), refused) {
                return Err(e);
            }
        }

        // Keyed by (inbox, message id): ids are only unique per inbox.
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        if !f.include_existing {
            for &n in &targets.clone() {
                match self.with_inbox_at(n, |p, inbox| p.list(inbox)) {
                    Ok(list) => seen.extend(list.into_iter().map(|s| (n, s.id))),
                    Err(e) if all => self.skip_or_warn(&mut targets, n, &e),
                    Err(e) => return Err(e),
                }
            }
        }
        if targets.is_empty() {
            return Err(Error::NotFound("no usable inbox to wait on".into()));
        }
        self.note(&format!(
            "waiting for mail to {} (timeout {timeout}s)...",
            self.targets_label(&targets)
        ));

        let mut failing: HashSet<usize> = HashSet::new();
        // Inboxes whose mail from before the wait hasn't all been looked at
        // yet (with `include_existing`): there the newest match wins, so a
        // code from an earlier try doesn't beat the one just sent.
        let mut existing: HashSet<usize> = if f.include_existing {
            targets.iter().copied().collect()
        } else {
            HashSet::new()
        };
        let mut poller = Poller::new(Duration::from_secs(interval), self.config.push());
        loop {
            self.check_cancel()?;
            for n in targets.clone() {
                if !poller.is_due(&self.state.inboxes[n].address) {
                    continue;
                }
                let Some(mut list) = self.poll(n, &mut targets, &mut failing, all, &mut poller)?
                else {
                    continue;
                };
                // New arrivals oldest first, so the first matching one wins.
                if !existing.contains(&n) {
                    list.reverse();
                }
                let mut looked_at_all = true;
                for s in &list {
                    let key = (n, s.id.clone());
                    if seen.contains(&key) {
                        continue;
                    }
                    if from.as_ref().is_some_and(|re| !re.is_match(&s.from))
                        || subject.as_ref().is_some_and(|re| !re.is_match(&s.subject))
                    {
                        seen.insert(key);
                        continue;
                    }
                    let msg = match self.with_inbox_at(n, |p, inbox| p.get(inbox, &key.1)) {
                        Ok(m) => m,
                        Err(e) if e.is_provider_failure() => {
                            looked_at_all = false;
                            break;
                        }
                        // Deleted or purged since it was listed.
                        Err(Error::NotFound(_)) => {
                            seen.insert(key);
                            continue;
                        }
                        Err(e) => return Err(e),
                    };
                    seen.insert(key);
                    let text = message_body(&msg);
                    if body.as_ref().is_some_and(|re| !re.is_match(&text)) {
                        continue;
                    }
                    if let Some(found) = accept(&msg, &text) {
                        if all {
                            self.note(&format!("received by {}", self.state.inboxes[n].address));
                        }
                        return Ok((n, msg, text, found));
                    }
                }
                if looked_at_all {
                    existing.remove(&n);
                }
            }
            if targets.is_empty() {
                return Err(Error::NotFound("all inboxes have expired".into()));
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(timeout));
            }
            let wake = match self.cancel {
                Some(_) => deadline.min(Instant::now() + CANCEL_CHECK),
                None => deadline,
            };
            poller.sleep(Some(wake));
        }
    }

    /// Lists inbox `n` for a polling loop and (re)starts its push stream. A
    /// temporary provider failure is warned about once and yields `None`; in
    /// multi-inbox mode other errors go through `skip_or_warn`.
    fn poll(
        &mut self,
        n: usize,
        targets: &mut Vec<usize>,
        failing: &mut HashSet<usize>,
        multi: bool,
        poller: &mut Poller,
    ) -> Result<Option<Vec<Summary>>> {
        let address = self.state.inboxes[n].address.clone();
        let result = self.with_inbox_at(n, |p, inbox| {
            let list = Self::sorted_list(p, inbox)?;
            poller.subscribe(p, inbox);
            Ok(list)
        });
        poller.polled(&address, result.is_ok());
        match result {
            Ok(list) => {
                if failing.remove(&n) {
                    self.note(&format!(
                        "{} reachable again",
                        self.state.inboxes[n].address
                    ));
                }
                Ok(Some(list))
            }
            Err(e) if e.is_provider_failure() => {
                if failing.insert(n) {
                    self.warn(&format!("{e}; will keep trying"));
                }
                Ok(None)
            }
            Err(e) if multi => {
                self.skip_or_warn(targets, n, &e);
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn targets_label(&self, targets: &[usize]) -> String {
        match targets {
            [n] => self.state.inboxes[*n].address.clone(),
            _ => format!("{} inboxes", targets.len()),
        }
    }

    /// In multi-inbox mode: stop polling an inbox that is gone for good,
    /// and only warn about temporary failures.
    fn skip_or_warn(&self, targets: &mut Vec<usize>, n: usize, e: &Error) {
        let address = &self.state.inboxes[n].address;
        if e.is_provider_failure() {
            self.warn(&format!("{e}; will retry {address}"));
        } else {
            self.note(&format!("skipping {address}: {e}"));
            targets.retain(|&t| t != n);
        }
    }

    fn otp(&mut self, a: OtpArgs) -> Result<()> {
        let re = a.regex.as_deref().map(compile_plain).transpose()?;
        let (inbox, msg, body, code) = if a.wait {
            let (n, msg, body, code) = self.wait_for(&a.filter, |m, body| {
                extract::otp(&m.subject, body, re.as_ref())
            })?;
            (self.state.inboxes[n].address.clone(), msg, body, Some(code))
        } else {
            let msg = self.fetch(&a.msg)?;
            let body = message_body(&msg);
            let code = extract::otp(&msg.subject, &body, re.as_ref());
            (self.current()?.address.clone(), msg, body, code)
        };
        match code {
            Some(code) => {
                self.print_code(&code, &msg, &body, re.is_some(), &inbox);
                Ok(())
            }
            None => Err(Error::NotFound(format!(
                "no verification code found in message {} ({:?}); try --regex",
                msg.id,
                clean_line(&msg.subject)
            ))),
        }
    }

    fn links(&mut self, a: LinksArgs) -> Result<()> {
        let filter = a.r#match.as_deref().map(compile).transpose()?;
        let msg = self.fetch(&a.msg)?;
        let mut links = message_links(&msg);
        if let Some(re) = &filter {
            links.retain(|l| re.is_match(l));
        }
        if links.is_empty() {
            return Err(Error::NotFound(format!(
                "no links found in message {}",
                msg.id
            )));
        }
        if a.first || a.open {
            links.truncate(1);
        }
        if a.open {
            system::open_link(&links[0], self.browser().as_deref())?;
        }
        if self.global.json {
            self.json_out(&json!({ "links": links, "message_id": msg.id }));
        } else {
            for l in &links {
                println!("{}", clean_line(l));
            }
        }
        Ok(())
    }

    fn watch(&mut self, a: WatchArgs) -> Result<()> {
        let interval = self.config.poll_interval(a.interval);
        let mut targets = self.targets(a.all_inboxes)?;
        let multi = a.all_inboxes;
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        let mut first = true;
        let mut failing: HashSet<usize> = HashSet::new();
        let mut poller = Poller::new(Duration::from_secs(interval), self.config.push());
        let mut rotate_at = a.rotate.map(|every| Instant::now() + every);
        self.note(&format!(
            "watching {}, Ctrl+C to stop",
            self.targets_label(&targets)
        ));
        loop {
            if let (Some(at), Some(every)) = (rotate_at, a.rotate) {
                if Instant::now() >= at {
                    rotate_at = Some(match self.watch_rotate(&a, &mut poller) {
                        Ok(n) => {
                            targets = vec![n];
                            // Indexes moved; the new inbox has nothing seen.
                            seen.clear();
                            failing.clear();
                            Instant::now() + every
                        }
                        Err(e) => {
                            self.warn(&format!("could not rotate ({e}); retrying in a minute"));
                            Instant::now() + Duration::from_secs(60)
                        }
                    });
                }
            }
            for n in targets.clone() {
                if !poller.is_due(&self.state.inboxes[n].address) {
                    continue;
                }
                let Some(list) = self.poll(n, &mut targets, &mut failing, multi, &mut poller)?
                else {
                    continue;
                };
                let address = clean_line(&self.state.inboxes[n].address);
                for s in list.iter().rev() {
                    if !seen.insert((n, s.id.clone())) {
                        continue;
                    }
                    let line = if multi {
                        format!("{address}  {}", summary_line(s))
                    } else {
                        summary_line(s)
                    };
                    if first && !self.global.json {
                        // Show what's already there, without notifying.
                        println!("{}", self.dim(&line));
                        continue;
                    }
                    if self.global.json {
                        let mut v = serde_json::to_value(s).expect("serializable");
                        v["inbox"] = json!(address);
                        println!("{}", crate::render::escape_json_invisible(&v.to_string()));
                    } else {
                        println!("{line}");
                    }
                    if a.notify && !first {
                        let title = if multi {
                            format!("{} → {address}", clean_line(&s.from))
                        } else {
                            clean_line(&s.from)
                        };
                        system::notify(&title, &clean_line(&s.subject));
                    }
                }
            }
            let _ = std::io::stdout().flush();
            first = false;
            if targets.is_empty() {
                return Err(Error::NotFound("all inboxes have expired".into()));
            }
            poller.sleep(rotate_at);
        }
    }

    /// `watch --rotate`: replaces the watched inbox with a fresh one, makes
    /// it active and announces it. Returns the new inbox's index.
    fn watch_rotate(&mut self, a: &WatchArgs, poller: &mut Poller) -> Result<usize> {
        let (inbox, old_inbox) = self.replace_current()?;
        let old = old_inbox.address.clone();
        let added = inbox.clone();
        self.state = self.store.update(|s| s.add(added))?.0;
        // `-i` named the old inbox; from now on follow the active one.
        self.global.inbox = None;
        poller.forget(&old);
        let address = clean_line(&inbox.address);
        if self.global.json {
            println!(
                "{}",
                json!({ "rotated": { "from": old, "to": inbox.address } })
            );
        } else {
            println!("{} {}", self.green("new address:"), self.bold(&address));
        }
        if a.copy {
            self.copy_new_address(&inbox.address);
        }
        if a.notify {
            system::notify("tmpmail: new address", &address);
        }
        self.retire(&old_inbox, &inbox, a.keep);
        self.state
            .position(&inbox.address)
            .ok_or_else(|| Error::NotFound(format!("{address} vanished from the state file")))
    }

    // ----- attachments -----

    fn attachments(&mut self, a: MessageRef) -> Result<()> {
        let msg = self.fetch(&a)?;
        if self.global.json {
            self.json_out(&json!({ "message_id": msg.id, "attachments": msg.attachments }));
            return Ok(());
        }
        if msg.attachments.is_empty() {
            println!("No attachments.");
        }
        let rows: Vec<[String; 4]> = msg
            .attachments
            .iter()
            .map(|att| {
                [
                    clean_line(&att.id),
                    clean_line(&att.filename),
                    clean_line(&att.content_type),
                    att.size.map(human_size).unwrap_or_default(),
                ]
            })
            .collect();
        let [iw, fw, tw, sw] = column_widths(&rows);
        for [id, file, kind, size] in &rows {
            println!(
                "{}  {}  {}  {size:>sw$}",
                self.dim(&pad(id, iw)),
                pad(file, fw),
                self.dim(&pad(kind, tw)),
            );
        }
        Ok(())
    }

    fn save(&mut self, a: SaveArgs) -> Result<()> {
        let (wanted, out_dir) = (&a.attachments, &a.output);
        std::fs::create_dir_all(out_dir).map_err(anyhow::Error::from)?;
        let (saved, failed) = self.with_message(Some(&a.id), |p, inbox, id| {
            let msg = p.get(inbox, id)?;
            let selected: Vec<_> = msg
                .attachments
                .iter()
                .filter(|att| {
                    wanted.is_empty() || wanted.iter().any(|w| *w == att.id || *w == att.filename)
                })
                .collect();
            if selected.is_empty() {
                return Err(Error::NotFound(format!(
                    "no matching attachments in message {id}"
                )));
            }
            // One the provider can't find (mail.tm refuses names with a
            // `/`) doesn't stop the others; a provider or disk failure does.
            let (mut saved, mut failed) = (Vec::new(), Vec::new());
            for (n, att) in selected.iter().enumerate() {
                let name = safe_filename(&att.filename, &format!("attachment-{}", n + 1));
                let result = p.attachment(inbox, &msg, att).and_then(|data| {
                    let (path, mut file) = create_unique(&out_dir.join(&name)).map_err(|e| {
                        anyhow::anyhow!("cannot create {}: {e}", out_dir.join(&name).display())
                    })?;
                    if let Err(e) = file.write_all(&data) {
                        drop(file);
                        let _ = std::fs::remove_file(&path);
                        return Err(anyhow::anyhow!("cannot write {}: {e}", path.display()).into());
                    }
                    Ok(path)
                });
                match result {
                    Ok(path) => saved.push(path),
                    Err(e @ Error::NotFound(_)) => failed.push((att.id.clone(), name, e)),
                    // Nothing done yet: the usual error.
                    Err(e) if saved.is_empty() && failed.is_empty() => return Err(e),
                    Err(e) => {
                        failed.push((att.id.clone(), name, e));
                        break;
                    }
                }
            }
            Ok((saved, failed))
        })?;
        if self.global.json {
            let failures: Vec<Value> = failed
                .iter()
                .map(|(id, name, e)| {
                    json!({
                        "id": id,
                        "filename": name,
                        "error": { "kind": e.kind(), "message": format!("{e:#}") },
                    })
                })
                .collect();
            let mut v = json!({ "saved": saved });
            if !failures.is_empty() {
                v["failed"] = json!(failures);
            }
            self.json_out(&v);
        } else {
            for p in &saved {
                println!("{}", p.display());
            }
            for (_, name, e) in &failed {
                self.warn(&format!("could not save {name}: {e:#}"));
            }
        }
        match failed.into_iter().next() {
            Some((_, _, e)) => {
                // Shown above with what was saved; the exit code still tells.
                self.error_shown = true;
                Err(e)
            }
            None => Ok(()),
        }
    }

    fn delete(&mut self, a: DeleteArgs) -> Result<()> {
        let deleted = if a.all {
            self.with_inbox(|p, inbox| {
                let list = p.list(inbox)?;
                for s in &list {
                    p.delete_message(inbox, &s.id)?;
                }
                Ok(list.len())
            })?
        } else {
            self.with_message(a.id.as_deref(), |p, inbox, id| {
                p.delete_message(inbox, id).map(|()| 1)
            })?
        };
        if self.global.json {
            self.json_out(&json!({ "deleted": deleted }));
        } else {
            self.note(&format!("deleted {deleted} message(s)"));
        }
        Ok(())
    }

    // ----- inbox management -----

    fn whoami(&mut self) -> Result<()> {
        let inbox = self.current()?;
        if self.global.json {
            self.json_out(&self.view(inbox));
        } else {
            println!("{}", clean_line(&inbox.address));
            if inbox.is_expired() {
                self.warn("this inbox has expired; create a new one with `tmpmail new`");
            }
        }
        Ok(())
    }

    fn copy(&mut self) -> Result<()> {
        let address = self.current()?.address.clone();
        system::copy(&address, self.clipboard_cmd().as_deref())?;
        if self.global.json {
            self.json_out(&json!({ "copied": address }));
        } else {
            self.note(&format!("copied {address}"));
        }
        Ok(())
    }

    fn qr(&mut self) -> Result<()> {
        let address = self.current()?.address.clone();
        if self.global.json {
            self.json_out(&json!({ "address": address }));
            return Ok(());
        }
        let code = qrcode::QrCode::new(address.as_bytes()).map_err(|e| anyhow::anyhow!("{e}"))?;
        let art = code
            .render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build();
        println!("{art}\n{}", clean_line(&address));
        Ok(())
    }

    fn domains(&mut self, a: ProviderArg) -> Result<()> {
        let id = a
            .provider
            .clone()
            .unwrap_or_else(|| self.config.provider_order()[0].clone());
        let domains = provider::get(&id)?.domains()?;
        if self.global.json {
            self.json_out(&json!({ "provider": id, "domains": domains }));
        } else {
            for d in domains {
                println!("{}", clean_line(&d));
            }
        }
        Ok(())
    }

    fn inboxes(&mut self, a: InboxesArgs) -> Result<()> {
        if a.prune {
            let (state, pruned) = self.store.update(State::prune_expired)?;
            self.state = state;
            self.note(&format!("forgot {pruned} expired inbox(es)"));
        }
        if self.global.json {
            let views: Vec<_> = self.state.inboxes.iter().map(|i| self.view(i)).collect();
            self.json_out(&views);
            return Ok(());
        }
        if self.state.inboxes.is_empty() {
            println!("No inboxes. Create one with `tmpmail new`.");
        }
        let rows: Vec<[String; 3]> = self
            .state
            .inboxes
            .iter()
            .map(|i| {
                [
                    clean_line(&i.address),
                    i.label
                        .as_deref()
                        .map(|l| format!("[{}]", clean_line(l)))
                        .unwrap_or_default(),
                    i.provider.clone(),
                ]
            })
            .collect();
        let [aw, lw, pw] = column_widths(&rows);
        for (i, [address, label, provider]) in self.state.inboxes.iter().zip(&rows) {
            let v = self.view(i);
            let mark = if v.active {
                self.green("*")
            } else {
                " ".into()
            };
            let status = match i.expires_at {
                _ if v.expired => self.red("expired"),
                Some(t) if t - Utc::now() < chrono::Duration::minutes(10) => {
                    self.yellow(&format!("expires {}", relative(t)))
                }
                Some(t) => self.dim(&format!("expires {}", relative(t))),
                None => String::new(),
            };
            let line = format!(
                "{mark} {}  {}  {}  {status}",
                self.bold(&pad(address, aw)),
                self.cyan(&pad(label, lw)),
                self.dim(&pad(provider, pw)),
            );
            println!("{}", line.trim_end());
        }
        Ok(())
    }

    fn use_inbox(&mut self, address: &str) -> Result<()> {
        let n = self.state.resolve(Some(address))?;
        let address = self.state.inboxes[n].address.clone();
        let active = address.clone();
        self.state = self.store.update(|s| s.active = Some(active))?.0;
        if self.global.json {
            if let Some(n) = self.state.position(&address) {
                self.json_out(&self.view(&self.state.inboxes[n]));
            }
        } else {
            println!("{}", clean_line(&address));
        }
        Ok(())
    }

    fn login(&mut self, a: LoginArgs) -> Result<()> {
        let id = a.provider.clone().unwrap_or_else(|| {
            provider::all()
                .find(|id| provider::get(id).is_ok_and(|p| p.owns_address(&a.address)))
                .unwrap_or("mailtm")
                .to_string()
        });
        if let Some(label) = &a.label {
            self.state.check_label(label, Some(&a.address))?;
        }
        if let Some((user, _)) = a.address.split_once('@') {
            self.state.check_user(user, Some(&a.address))?;
        }
        let p = provider::get(&id)?;
        let password = match (&a.password, p.caps().private) {
            (Some(pw), _) => pw.clone(),
            (None, false) => String::new(),
            (None, true) => {
                return Err(Error::Usage(
                    "--password (or TMPMAIL_PASSWORD) is required".into(),
                ));
            }
        };
        let mut inbox = checked_address(p.as_ref(), p.login(&a.address, &password)?)?;
        // Logging in again to a stored address keeps its label.
        inbox.label = a.label.or_else(|| {
            let n = self.state.position(&inbox.address)?;
            self.state.inboxes[n].label.clone()
        });
        self.activate(&inbox, false)?;
        if !self.global.json {
            self.warn_if_public(p.as_ref());
        }
        Ok(())
    }

    fn drop_inbox(&mut self, a: DropArgs) -> Result<()> {
        let n = self
            .state
            .resolve(a.address.as_deref().or(self.global.inbox.as_deref()))?;
        let mut inbox = self.state.inboxes[n].clone();
        if !a.local {
            let p = provider::get(&inbox.provider)?;
            if let Err(e) = p.delete_inbox(&mut inbox) {
                self.warn(&format!(
                    "could not delete it at the provider ({e}); forgetting it locally"
                ));
            }
        }
        self.forget(&inbox.address)?;
        if self.global.json {
            self.json_out(&json!({ "dropped": inbox.address, "active": self.state.active }));
        } else {
            self.note(&format!("dropped {}", inbox.address));
        }
        Ok(())
    }

    fn rotate(&mut self, a: RotateArgs) -> Result<()> {
        let (inbox, old) = self.replace_current()?;
        self.activate(&inbox, a.copy)?;
        self.retire(&old, &inbox, a.keep);
        Ok(())
    }

    fn providers(&mut self, a: ProvidersArgs) -> Result<()> {
        let order = self.config.provider_order();
        // Health checks are independent network calls: run them side by side.
        let mut health: Vec<Option<Health>> = if a.check {
            std::thread::scope(|s| {
                let checks: Vec<_> = provider::all()
                    .map(|id| s.spawn(move || check_health(id)))
                    .collect();
                checks
                    .into_iter()
                    .map(|c| Some(c.join().expect("health check panicked")))
                    .collect()
            })
        } else {
            provider::all().map(|_| None).collect()
        };
        let mut rows = Vec::new();
        for (id, status) in provider::all().zip(health.drain(..)) {
            let p = provider::get(id)?;
            rows.push(ProviderRow {
                id,
                name: p.name(),
                website: p.website(),
                in_fallback_order: order.iter().position(|o| o == id).map(|n| n + 1),
                capabilities: p.caps(),
                status,
            });
        }
        if self.global.json {
            self.json_out(&rows);
            return Ok(());
        }
        for r in &rows {
            let caps = &r.capabilities;
            let order = match r.in_fallback_order {
                Some(n) => format!("#{n}"),
                None => "--".into(),
            };
            let flags: Vec<&str> = [
                (!caps.private, "PUBLIC"),
                (caps.custom_user, "custom-user"),
                (caps.choose_domain, "domain"),
                (caps.attachments, "attachments"),
                (caps.raw, "raw"),
                (caps.delete_message, "delete"),
                (caps.login, "login"),
            ]
            .into_iter()
            .filter_map(|(on, flag)| on.then_some(flag))
            .collect();
            let status = match &r.status {
                None => String::new(),
                Some(Health { ms: Some(ms), .. }) => {
                    format!("  {}", self.green(&format!("ok {ms}ms")))
                }
                Some(h) => format!(
                    "  {} {}",
                    self.red("DOWN:"),
                    clean_line(h.error.as_deref().unwrap_or_default())
                ),
            };
            let about = if flags.is_empty() {
                caps.lifetime.to_string()
            } else {
                format!("{} | {}", flags.join(","), caps.lifetime)
            };
            println!(
                "{order:>3} {:<13} {:<15} {}{}",
                r.id,
                r.name,
                self.dim(&about),
                status
            );
        }
        Ok(())
    }
}

fn check_health(id: &str) -> Health {
    let start = Instant::now();
    match provider::get(id).and_then(|p| p.health()) {
        Ok(()) => Health {
            ok: true,
            ms: Some(start.elapsed().as_millis() as u64),
            error: None,
        },
        Err(e) => Health {
            ok: false,
            ms: None,
            error: Some(e.to_string()),
        },
    }
}

fn summary_line(s: &Summary) -> String {
    let when = s.date.map(relative).unwrap_or_default();
    format!(
        "{when:>9}  {}  {}",
        clean_line(&s.from),
        clean_line(&s.subject)
    )
}

fn term_width() -> usize {
    system::term_size().map_or(100, |(cols, _)| cols)
}

/// Widest cell of each column, in characters.
fn column_widths<const N: usize>(rows: &[[String; N]]) -> [usize; N] {
    let mut widths = [0; N];
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    widths
}

fn pad(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

fn human_size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// Creates `path` as a new file, or `name (1).ext`, `name (2).ext`, ... if
/// that name is taken. Each is created with `O_EXCL` (and `O_NOFOLLOW` on
/// Unix: `O_EXCL` alone already refuses a link, this says it twice), so
/// nothing that is already there is written through or over, a link put in
/// the folder included, and no check can be raced. A name taken by
/// something that can't be opened (a folder, on Windows) is passed over
/// too.
fn create_unique(path: &std::path::Path) -> std::io::Result<(std::path::PathBuf, std::fs::File)> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 0..10_000 {
        let candidate = match (n, ext) {
            (0, _) => path.to_path_buf(),
            (_, Some(e)) => path.with_file_name(format!("{stem} ({n}).{e}")),
            (_, None) => path.with_file_name(format!("{stem} ({n})")),
        };
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW);
        }
        match opts.open(&candidate) {
            Ok(f) => return Ok((candidate, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) if std::fs::symlink_metadata(&candidate).is_ok() => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "every name up to (9999) is taken",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Attachment;
    use chrono::TimeZone;

    /// A provider whose inbox holds `mail` (id, subject, minute it came in).
    struct Fixed {
        mail: Vec<(&'static str, &'static str, u32)>,
        /// Claims to delete messages, though deleting one always fails.
        deletes: bool,
    }

    impl Fixed {
        fn message(&self, id: &str) -> Option<Message> {
            let (id, subject, minute) = self.mail.iter().find(|m| m.0 == id)?;
            Some(Message {
                id: id.to_string(),
                from: "Demo <hello@example.org>".into(),
                subject: subject.to_string(),
                date: Some(Utc.with_ymd_and_hms(2026, 10, 31, 12, *minute, 0).unwrap()),
                text: Some(subject.to_string()),
                ..Default::default()
            })
        }
    }

    impl Provider for Fixed {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn website(&self) -> &'static str {
            ""
        }
        fn caps(&self) -> Capabilities {
            Capabilities {
                private: true,
                custom_user: false,
                choose_domain: false,
                attachments: false,
                raw: false,
                delete_message: self.deletes,
                delete_inbox: false,
                login: false,
                lifetime: "",
            }
        }
        fn domains(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn create(&self, _: &CreateOpts) -> Result<Inbox> {
            Ok(Inbox::new("fake", "n@x.com".into(), Value::Null))
        }
        fn list(&self, _: &mut Inbox) -> Result<Vec<Summary>> {
            Ok(self
                .mail
                .iter()
                .filter_map(|m| self.message(m.0))
                .map(|m| m.summary())
                .collect())
        }
        fn get(&self, _: &mut Inbox, id: &str) -> Result<Message> {
            self.message(id).ok_or_else(|| Error::NotFound(id.into()))
        }
    }

    fn app_with(dir: &std::path::Path, p: Fixed) -> App {
        let mut app = App::for_test(dir.to_path_buf());
        app.set_provider(Box::new(p));
        let inbox = Inbox::new("fake", "a@x.com".into(), Value::Null);
        app.state = app.store.update(|s| s.add(inbox)).unwrap().0;
        app
    }

    /// mail.tm as round 2 met it: the inline image is listed by its file
    /// name, its `Content-ID` is only in the raw source.
    struct MailTmLike;

    const CID: &str = "179350583260.525126.3557191885519229186@tmpmail.test";

    impl Provider for MailTmLike {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn website(&self) -> &'static str {
            ""
        }
        fn caps(&self) -> Capabilities {
            Capabilities {
                private: true,
                custom_user: false,
                choose_domain: false,
                attachments: true,
                raw: true,
                delete_message: false,
                delete_inbox: false,
                login: false,
                lifetime: "",
            }
        }
        fn domains(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn create(&self, _: &CreateOpts) -> Result<Inbox> {
            unimplemented!()
        }
        fn list(&self, _: &mut Inbox) -> Result<Vec<Summary>> {
            Ok(Vec::new())
        }
        fn get(&self, _: &mut Inbox, _: &str) -> Result<Message> {
            unimplemented!()
        }
        fn raw(&self, _: &mut Inbox, _: &str) -> Result<Vec<u8>> {
            Ok(format!(
                "Content-Type: multipart/related; boundary=b\r\n\r\n--b\r\n\
                 Content-Type: text/html\r\n\r\n<img src=\"cid:{CID}\">\r\n--b\r\n\
                 Content-Type: image/png\r\nContent-ID: <{CID}>\r\n\
                 Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--b--\r\n"
            )
            .into_bytes())
        }
        fn attachment(&self, _: &mut Inbox, _: &Message, a: &Attachment) -> Result<Vec<u8>> {
            if a.filename == "big.png" {
                BIG_FETCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(vec![0; 9 << 20]);
            }
            Ok(format!("bytes of {}", a.filename).into_bytes())
        }
    }

    static BIG_FETCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    #[test]
    fn inline_images_are_fetched_once_and_capped() {
        // Security review C1: one 9 MiB attachment under a thousand `cid:`
        // names is fetched once, and the copy holds at most 25 MiB of it.
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        app.set_provider(Box::new(MailTmLike));
        let inbox = Inbox::new("fake", "a@x.com".into(), Value::Null);
        app.state = app.store.update(|s| s.add(inbox)).unwrap().0;
        let html: String = (0..1000)
            .map(|i| format!("<img src=\"cid:big.png@{i}\">"))
            .collect::<String>()
            + &"<img src=\"cid:big.png@0\">".repeat(100);
        let msg = Message {
            id: "m1".into(),
            html: Some(html.clone()),
            attachments: vec![Attachment {
                id: "A1".into(),
                filename: "big.png".into(),
                content_type: "image/png".into(),
                size: None,
                url: None,
            }],
            ..Default::default()
        };
        let inline = app.inline_images(0, &msg);
        assert_eq!(BIG_FETCHES.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(inline.len(), provider::MAX_INLINE_TOTAL / (9 << 20));
        // The same name a hundred times is put in the copy within the
        // limit too.
        let doc = safe_html_document_inline("s", &html, false, &inline);
        let shown = doc.matches("data:image/png;base64,").count();
        assert_eq!(shown, 2, "{shown}");
        assert!(doc.len() < provider::MAX_INLINE_TOTAL * 2);
    }

    #[test]
    fn read_html_embeds_inline_images() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        app.set_provider(Box::new(MailTmLike));
        let inbox = Inbox::new("fake", "a@x.com".into(), Value::Null);
        app.state = app.store.update(|s| s.add(inbox)).unwrap().0;
        let att = |name: &str| Attachment {
            id: format!("ATTACH-{name}"),
            filename: name.into(),
            content_type: "image/png".into(),
            size: None,
            url: None,
        };
        let msg = Message {
            id: "m1".into(),
            html: Some(format!(
                "<img src=\"cid:{}\"><img src='cid:logo.png'><img src=cid:gone@x>",
                CID.replace('@', "%40")
            )),
            attachments: vec![att("inline-box.png"), att("logo.png")],
            ..Default::default()
        };
        let inline = app.inline_images(0, &msg);
        // From the source, by Content-ID; and by file name, from the listing.
        assert_eq!(inline[CID], "data:image/png;base64,iVBORw0KGgo=");
        assert_eq!(
            inline["logo.png"],
            format!("data:image/png;base64,{}", base64(b"bytes of logo.png"))
        );
        assert_eq!(inline.len(), 2);
        // The copy for the browser shows them (its policy allows `data:`);
        // a cid without an image stays.
        let doc = safe_html_document_inline("s", msg.html.as_deref().unwrap(), false, &inline);
        assert!(
            doc.contains(r#"src="data:image/png;base64,iVBORw0KGgo=""#),
            "{doc}"
        );
        assert_eq!(doc.matches("data:image/png;base64,").count(), 2, "{doc}");
        assert!(doc.contains(r#"src="cid:gone@x""#), "{doc}");
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"\xff\xfe\xfd\x00"), "//79AA==");
    }

    #[test]
    fn the_newest_existing_match_wins() {
        let dir = tempfile::tempdir().unwrap();
        // Listed in the provider's order, which isn't by date.
        let mut app = app_with(
            dir.path(),
            Fixed {
                mail: vec![
                    ("old", "Your code is 111222", 10),
                    ("other", "Newsletter", 30),
                    ("new", "Your code is 333444", 20),
                ],
                deletes: false,
            },
        );
        let filter = WaitFilter {
            include_existing: true,
            timeout: Some(5),
            ..Default::default()
        };
        let (_, msg, _, code) = app
            .wait_for(&filter, |m, body| extract::otp(&m.subject, body, None))
            .unwrap();
        assert_eq!((msg.id.as_str(), code.as_str()), ("new", "333444"));
    }

    #[test]
    fn new_and_login_refuse_a_taken_label_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with(
            dir.path(),
            Fixed {
                mail: Vec::new(),
                deletes: false,
            },
        );
        app.state = app
            .store
            .update(|s| s.inboxes[0].label = Some("mt1".into()))
            .unwrap()
            .0;
        // Guerrilla is never contacted: the label is checked first.
        let new = NewArgs {
            user: None,
            domain: None,
            provider: Some("guerrilla".into()),
            label: Some("mt1".into()),
            password: None,
            copy: false,
            address: None,
        };
        let err = app.run(Command::New(new)).unwrap_err();
        assert!(matches!(err, Error::Usage(_)), "{err}");
        assert!(err.to_string().contains("a@x.com"), "{err}");
        let login = LoginArgs {
            address: "b@maildrop.cc".into(),
            password: None,
            provider: None,
            label: Some("A".into()),
        };
        let err = app.run(Command::Login(login)).unwrap_err();
        assert!(
            err.to_string().contains("would also match a@x.com"),
            "{err}"
        );
    }

    #[test]
    fn a_failed_copy_keeps_the_new_inbox_and_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        app.global.clipboard_cmd = Some("false".into());
        let inbox = Inbox::new("guerrilla", "n@x.com".into(), Value::Null);
        assert!(!app.activate(&inbox, true).unwrap(), "not copied");
        let stored = app.store.load().unwrap();
        assert_eq!(stored.active.as_deref(), Some("n@x.com"));
        #[cfg(unix)]
        {
            app.global.clipboard_cmd = Some("cat".into());
            assert!(app.activate(&inbox, true).unwrap());
        }
    }

    #[test]
    fn rotate_stores_the_new_inbox_before_dropping_the_old() {
        for keep in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = app_with(
                dir.path(),
                Fixed {
                    mail: Vec::new(),
                    deletes: false,
                },
            );
            app.state = app
                .store
                .update(|s| s.inboxes[0].label = Some("work".into()))
                .unwrap()
                .0;
            app.global.clipboard_cmd = Some("false".into());
            app.run(Command::Rotate(RotateArgs { keep, copy: true }))
                .unwrap();
            let s = app.store.load().unwrap();
            assert_eq!(s.active.as_deref(), Some("n@x.com"));
            assert_eq!(
                s.resolve(Some("work")).unwrap(),
                s.position("n@x.com").unwrap()
            );
            assert_eq!(s.position("a@x.com").is_some(), keep);
        }
    }

    #[test]
    fn wait_delete_is_refused_before_waiting_where_deleting_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing there matches: the wait itself would time out.
        let mut app = app_with(
            dir.path(),
            Fixed {
                mail: vec![("1", "Newsletter", 1)],
                deletes: false,
            },
        );
        let wait = WaitArgs {
            filter: WaitFilter {
                include_existing: true,
                timeout: Some(3),
                ..Default::default()
            },
            otp: true,
            link: false,
            otp_regex: None,
            delete: true,
        };
        let start = Instant::now();
        let err = app.run(Command::Wait(wait)).unwrap_err();
        assert!(matches!(err, Error::Unsupported { .. }), "{err}");
        assert!(err.to_string().contains("a@x.com"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_failed_delete_after_wait_prints_one_document_and_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with(
            dir.path(),
            Fixed {
                mail: vec![("1", "Your code is 111222", 1)],
                deletes: true,
            },
        );
        app.global.json = true;
        let wait = WaitArgs {
            filter: WaitFilter {
                include_existing: true,
                timeout: Some(3),
                ..Default::default()
            },
            otp: true,
            link: false,
            otp_regex: None,
            delete: true,
        };
        assert!(app.run(Command::Wait(wait)).is_err());
        // main prints no `{"error": ...}` after the code's document.
        assert!(app.error_shown());
    }

    #[test]
    fn hints_name_an_inbox_that_isnt_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with(
            dir.path(),
            Fixed {
                mail: Vec::new(),
                deletes: false,
            },
        );
        let other = Inbox::new("fake", "b@x.com".into(), Value::Null);
        app.state = app.store.update(|s| s.add(other)).unwrap().0;
        // b@x.com is active now; a@x.com came from -i.
        assert_eq!(app.inbox_option(1), "");
        assert_eq!(app.inbox_option(0), "-i a@x.com ");
        app.state.inboxes[0].address = "o'k$@x.com".into();
        assert_eq!(app.inbox_option(0), r"-i 'o'\''k$@x.com' ");
        app.state.inboxes[0].address = "a@x.com".into();
        let msg = Message {
            id: "m1".into(),
            attachments: vec![crate::provider::Attachment {
                filename: "f.txt".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            app.render_message(&msg, 0)
                .contains("(tmpmail -i a@x.com save m1)")
        );
        assert!(app.render_message(&msg, 1).contains("(tmpmail save m1)"));
    }

    #[test]
    fn a_maildrop_login_warns_that_the_inbox_is_public() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        app.log_to(tx);
        let login = LoginArgs {
            address: "c1md18730@maildrop.cc".into(),
            password: None,
            provider: None,
            label: None,
        };
        app.run(Command::Login(login)).unwrap();
        let said: Vec<String> = rx.try_iter().collect();
        assert!(said.iter().any(|w| w.contains("public")), "{said:?}");
    }

    /// One message with two attachments, of which only `note.txt` can be
    /// fetched (mail.tm refuses names with a `/`); listing fails when `down`.
    struct Files {
        down: bool,
    }

    impl Provider for Files {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn website(&self) -> &'static str {
            ""
        }
        fn caps(&self) -> Capabilities {
            Fixed {
                mail: Vec::new(),
                deletes: false,
            }
            .caps()
        }
        fn domains(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn create(&self, _: &CreateOpts) -> Result<Inbox> {
            unimplemented!()
        }
        fn list(&self, _: &mut Inbox) -> Result<Vec<Summary>> {
            if self.down {
                return Err(Error::Unreachable {
                    provider: "fake".into(),
                    detail: "down".into(),
                });
            }
            Ok(vec![
                self.get(&mut Inbox::new("fake", "a@x.com".into(), Value::Null), "m")?
                    .summary(),
            ])
        }
        fn get(&self, _: &mut Inbox, id: &str) -> Result<Message> {
            let att = |id: &str, name: &str| crate::provider::Attachment {
                id: id.into(),
                filename: name.into(),
                ..Default::default()
            };
            Ok(Message {
                id: id.into(),
                attachments: vec![att("1", "../../etc/evil name.txt"), att("2", "note.txt")],
                ..Default::default()
            })
        }
        fn attachment(
            &self,
            _: &mut Inbox,
            _: &Message,
            att: &crate::provider::Attachment,
        ) -> Result<Vec<u8>> {
            match att.id.as_str() {
                "2" => Ok(b"hello".to_vec()),
                _ => Err(Error::NotFound("fake: not found (HTTP 404)".into())),
            }
        }
    }

    #[test]
    fn save_keeps_going_past_a_failed_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("sv");
        let mut app = App::for_test(dir.path().join("state"));
        app.set_provider(Box::new(Files { down: false }));
        let inbox = Inbox::new("fake", "a@x.com".into(), Value::Null);
        app.state = app.store.update(|s| s.add(inbox)).unwrap().0;
        app.global.json = true;
        let save = SaveArgs {
            id: "1".into(),
            attachments: Vec::new(),
            output: out.clone(),
        };
        let err = app.run(Command::Save(save)).unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err}");
        // The JSON document (saved + failed) was printed instead.
        assert!(app.error_shown());
        assert_eq!(std::fs::read(out.join("note.txt")).unwrap(), b"hello");
        let files: Vec<_> = std::fs::read_dir(&out).unwrap().collect();
        assert_eq!(files.len(), 1, "only note.txt");
    }

    #[test]
    fn ls_all_fails_when_no_inbox_can_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        app.set_provider(Box::new(Files { down: true }));
        let inbox = Inbox::new("fake", "a@x.com".into(), Value::Null);
        app.state = app.store.update(|s| s.add(inbox)).unwrap().0;
        app.global.json = true;
        let err = app
            .run(Command::Ls(LsArgs { all_inboxes: true }))
            .unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(app.error_shown());

        let mut app = App::for_test(dir.path().to_path_buf());
        app.set_provider(Box::new(Files { down: false }));
        app.state = app.store.load().unwrap();
        assert!(app.run(Command::Ls(LsArgs { all_inboxes: true })).is_ok());
    }

    #[test]
    fn a_name_that_is_another_inboxs_label_is_refused_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with(
            dir.path(),
            Fixed {
                mail: Vec::new(),
                deletes: false,
            },
        );
        app.state = app
            .store
            .update(|s| s.inboxes[0].label = Some("gm1".into()))
            .unwrap()
            .0;
        // Guerrilla is never contacted: the name is checked first.
        for (user, address) in [(Some("GM1"), None), (None, Some("gm1@sharklasers.com"))] {
            let new = NewArgs {
                user: user.map(Into::into),
                domain: None,
                provider: Some("guerrilla".into()),
                label: Some("gmx".into()),
                password: None,
                copy: false,
                address: address.map(Into::into),
            };
            let err = app.run(Command::New(new)).unwrap_err();
            assert!(matches!(err, Error::Usage(_)), "{err}");
            assert!(
                err.to_string().contains("is already the label of a@x.com"),
                "{err}"
            );
        }
        let login = LoginArgs {
            address: "gm1@maildrop.cc".into(),
            password: None,
            provider: None,
            label: None,
        };
        let err = app.run(Command::Login(login)).unwrap_err();
        assert!(matches!(err, Error::Usage(_)), "{err}");
        assert!(err.to_string().contains("a@x.com"), "{err}");
        // Nothing was stored.
        assert_eq!(app.store.load().unwrap().inboxes.len(), 1);
    }

    #[test]
    fn unique_paths_do_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        assert_eq!(create_unique(&p).unwrap().0, p);
        assert_eq!(create_unique(&p).unwrap().0, dir.path().join("a (1).txt"));
        assert_eq!(create_unique(&p).unwrap().0, dir.path().join("a (2).txt"));
    }

    #[test]
    #[cfg(unix)]
    fn saving_never_follows_a_planted_link() {
        // Security review B2: a dangling link named like the attachment
        // (another user's, in a shared folder) isn't written through.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("victim-rc");
        let planted = dir.path().join("invoice.pdf");
        std::os::unix::fs::symlink(&target, &planted).unwrap();
        let (path, mut f) = create_unique(&planted).unwrap();
        f.write_all(b"attacker bytes").unwrap();
        assert_eq!(path, dir.path().join("invoice (1).pdf"));
        assert!(!target.exists());
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(2048), "2.0 KB");
    }

    #[test]
    fn columns_fit_the_widest_cell() {
        let rows = [
            ["a@x.com".to_string(), String::new()],
            ["long@example.org".to_string(), "[github]".to_string()],
        ];
        assert_eq!(column_widths(&rows), [16, 8]);
        assert_eq!(pad("ab", 4), "ab  ");
    }
}
