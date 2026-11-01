//! `tmpmail tui`: inboxes on the left, their mail and a preview on the
//! right, updated live. Network calls run on a worker thread with its own
//! `App`, so the screen never waits on a provider; the worker lists every
//! inbox on the same schedule as `watch` (push where the provider has it).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::app::{App, message_body, message_links, relative};
use crate::cli::Global;
use crate::error::Result;
use crate::extract;
use crate::provider::{Inbox, Message, Summary};
use crate::render::{body_text, clean, clean_line};
use crate::system;
use crate::watcher::{self, Watcher};

/// How long a status message stays in the footer.
const STATUS_TTL: Duration = Duration::from_secs(6);

pub fn run(app: &mut App) -> Result<()> {
    let (req_tx, req_rx) = channel();
    let (ev_tx, ev_rx) = channel();
    let (log_tx, log_rx) = channel();
    app.log_to(log_tx.clone());
    let global = app.global.clone();
    // `-i` picks the inbox to start on; checked before taking the screen.
    let start = match &global.inbox {
        Some(i) => Some(
            app.state.inboxes[app.state.resolve(Some(i))?]
                .address
                .clone(),
        ),
        None => None,
    };
    // Detached: a provider call can block for many seconds, and quitting
    // must not wait for it. The process exits with the UI.
    std::thread::spawn(move || {
        if let Err(e) = worker(global, req_rx, ev_tx.clone(), log_tx) {
            let _ = ev_tx.send(Ev::Fatal(e.to_string()));
        }
    });

    let mut terminal = ratatui::try_init().map_err(anyhow::Error::from)?;
    let mut ui = Ui::new(req_tx);
    ui.pending_select = start;
    let result = ui.run(&mut terminal, app, &ev_rx, &log_rx);
    ratatui::restore();
    result
}

// ---------- worker ----------

enum Req {
    Refresh(String),
    Open {
        address: String,
        id: String,
    },
    /// The user viewed this message: saved as read for every front end.
    MarkRead {
        address: String,
        id: String,
    },
    /// A new inbox at this provider only, or with the fallback order.
    Create(Option<String>),
    Use(String),
    DeleteMessage {
        address: String,
        id: String,
    },
    Drop(String),
}

enum Ev {
    Inboxes {
        inboxes: Vec<Inbox>,
        active: Option<String>,
    },
    Created(String),
    Listed {
        address: String,
        result: std::result::Result<Vec<Summary>, String>,
        live: bool,
    },
    Opened {
        address: String,
        /// The id asked for (a message's own may be spelled differently).
        id: String,
        result: std::result::Result<Box<Message>, String>,
    },
    Deleted {
        address: String,
        id: String,
    },
    Fatal(String),
}

fn worker(global: Global, rx: Receiver<Req>, tx: Sender<Ev>, log: Sender<String>) -> Result<()> {
    let mut w = Watcher::new(global)?;
    // `run` already sent the config warnings to this log.
    w.app.log_to_quietly(log.clone());
    w.run(&rx, &mut Front { tx, log });
    Ok(())
}

/// The TUI's side of the shared worker loop.
struct Front {
    tx: Sender<Ev>,
    log: Sender<String>,
}

impl watcher::Front for Front {
    type Req = Req;

    fn is_open(req: &Req) -> bool {
        matches!(req, Req::Open { .. })
    }

    fn inboxes(&mut self, app: &App) -> bool {
        self.tx
            .send(Ev::Inboxes {
                inboxes: app.state.inboxes.clone(),
                active: app.state.active.clone(),
            })
            .is_ok()
    }

    fn listed(
        &mut self,
        address: String,
        result: std::result::Result<Vec<Summary>, String>,
        live: bool,
    ) -> bool {
        self.tx
            .send(Ev::Listed {
                address,
                result,
                live,
            })
            .is_ok()
    }

    /// Carries out one request. Returns whether the inbox list changed.
    fn handle(&mut self, w: &mut Watcher, req: Req) -> bool {
        let tx = &self.tx;
        let fail = |what: &str, e: crate::error::Error| {
            let _ = self.log.send(format!("{what}: {e}"));
        };
        match req {
            Req::Refresh(address) => {
                w.poller.forget(&address); // makes it due now; the stream restarts
                false
            }
            Req::Open { address, id } => {
                let Some(n) = w.app.state.position(&address) else {
                    return false;
                };
                // Shown, not yet read: the preview opens on its own when a
                // list loads. `MarkRead` follows once the user looks at it.
                let result = w
                    .app
                    .with_inbox_at(n, |p, inbox| p.get(inbox, &id))
                    .map(Box::new)
                    .map_err(|e| e.to_string());
                let _ = tx.send(Ev::Opened {
                    address,
                    id,
                    result,
                });
                false
            }
            Req::Create(provider) => {
                match w.create(provider.as_deref()) {
                    Ok(address) => {
                        let _ = tx.send(Ev::Created(address));
                    }
                    Err(e) => {
                        let _ = self.log.send(e);
                    }
                }
                true
            }
            Req::Use(address) => {
                if let Err(e) = w.make_active(&address) {
                    fail("could not switch inbox", e);
                }
                true
            }
            Req::MarkRead { address, id } => {
                if let Some(n) = w.app.state.position(&address)
                    && let Err(e) = w.app.mark_read(n, &id)
                {
                    fail("couldn't mark the message read", e);
                }
                false
            }
            Req::DeleteMessage { address, id } => {
                match w.delete_message(&address, &id) {
                    Some(Ok(())) => {
                        let _ = self.tx.send(Ev::Deleted { address, id });
                    }
                    Some(Err(e)) => fail("could not delete the message", e),
                    None => {}
                }
                false
            }
            Req::Drop(address) => {
                w.drop_inbox(&address, fail);
                true
            }
        }
    }
}

// ---------- screen ----------

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Inboxes,
    Messages,
    Preview,
}

/// What we know about one inbox's mail.
#[derive(Default)]
struct Mail {
    messages: Vec<Summary>,
    loaded: bool,
    error: Option<String>,
    live: bool,
    checked: Option<Instant>,
}

enum Confirm {
    DeleteMessage { address: String, id: String },
    Drop(String),
}

enum Preview {
    None,
    Loading,
    Failed(String),
    Ready(Box<Message>),
}

struct Ui {
    req: Sender<Req>,
    inboxes: Vec<Inbox>,
    active: Option<String>,
    inbox_state: ListState,
    msg_state: ListState,
    mail: HashMap<String, Mail>,
    /// Messages that arrived while the TUI was open, shown as new.
    fresh: HashSet<(String, String)>,
    /// Fetched messages, by (address, id).
    cache: HashMap<(String, String), Box<Message>>,
    /// The message the preview shows, and its state.
    shown: Option<(String, String)>,
    preview: Preview,
    /// The user went to the shown message before it loaded: mark it read
    /// once it has.
    read_on_load: bool,
    /// The preview's text for (message id, width), since converting HTML
    /// on every frame would be wasteful.
    rendered: Option<(String, u16, Text<'static>)>,
    scroll: u16,
    focus: Focus,
    status: Option<(String, Instant)>,
    confirm: Option<Confirm>,
    help: bool,
    /// The provider picker (`N`), its cursor on a row of `provider::all()`.
    picker: Option<ListState>,
    /// Select this inbox once the worker reports it (after `n`).
    pending_select: Option<String>,
    quit: bool,
}

impl Ui {
    fn new(req: Sender<Req>) -> Self {
        Ui {
            req,
            inboxes: Vec::new(),
            active: None,
            inbox_state: ListState::default(),
            msg_state: ListState::default(),
            mail: HashMap::new(),
            fresh: HashSet::new(),
            cache: HashMap::new(),
            shown: None,
            read_on_load: false,
            preview: Preview::None,
            rendered: None,
            scroll: 0,
            focus: Focus::Inboxes,
            status: None,
            confirm: None,
            help: false,
            picker: None,
            pending_select: None,
            quit: false,
        }
    }

    fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        app: &mut App,
        events: &Receiver<Ev>,
        logs: &Receiver<String>,
    ) -> Result<()> {
        while !self.quit {
            terminal
                .draw(|f| self.draw(f))
                .map_err(anyhow::Error::from)?;
            if event::poll(Duration::from_millis(150)).map_err(anyhow::Error::from)? {
                if let Event::Key(key) = event::read().map_err(anyhow::Error::from)? {
                    if key.kind == KeyEventKind::Press {
                        self.key(key, app);
                        if self.focus == Focus::Preview {
                            self.mark_shown_read();
                        }
                    }
                }
            }
            for ev in events.try_iter() {
                if let Ev::Fatal(e) = ev {
                    return Err(anyhow::anyhow!(e).into());
                }
                self.event(ev);
            }
            for msg in logs.try_iter() {
                self.say(msg);
            }
        }
        Ok(())
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn send(&self, req: Req) {
        let _ = self.req.send(req);
    }

    fn inbox(&self) -> Option<&Inbox> {
        self.inbox_state
            .selected()
            .and_then(|n| self.inboxes.get(n))
    }

    fn address(&self) -> Option<String> {
        self.inbox().map(|i| i.address.clone())
    }

    fn messages(&self) -> &[Summary] {
        self.inbox()
            .and_then(|i| self.mail.get(&i.address))
            .map_or(&[], |m| m.messages.as_slice())
    }

    fn summary(&self) -> Option<&Summary> {
        self.msg_state
            .selected()
            .and_then(|n| self.messages().get(n))
    }

    fn message(&self) -> Option<&Message> {
        match &self.preview {
            Preview::Ready(m) => Some(m),
            _ => None,
        }
    }

    // ----- worker events -----

    fn event(&mut self, ev: Ev) {
        match ev {
            Ev::Inboxes { inboxes, active } => {
                let selected = self.address();
                self.inboxes = inboxes;
                self.active = active;
                let want = self
                    .pending_select
                    .clone()
                    .or(selected)
                    .or_else(|| self.active.clone());
                let n = want
                    .and_then(|a| self.inboxes.iter().position(|i| i.address == a))
                    .or((!self.inboxes.is_empty()).then_some(0));
                if n != self.inbox_state.selected() || self.inbox().is_none() {
                    self.select_inbox(n);
                }
                if let Some(a) = &self.pending_select {
                    if self.inboxes.iter().any(|i| &i.address == a) {
                        self.pending_select = None;
                    }
                }
            }
            Ev::Created(address) => {
                self.say(format!("created {address}, c copies it"));
                self.pending_select = Some(address);
            }
            Ev::Listed {
                address,
                result,
                live,
            } => {
                let current = self.address().as_deref() == Some(address.as_str());
                let keep_id = current
                    .then(|| self.summary().map(|s| s.id.clone()))
                    .flatten();
                let mail = self.mail.entry(address.clone()).or_default();
                mail.live = live;
                mail.checked = Some(Instant::now());
                match result {
                    Ok(list) => {
                        let new: Vec<&Summary> = if mail.loaded {
                            let known: HashSet<&str> =
                                mail.messages.iter().map(|s| s.id.as_str()).collect();
                            list.iter()
                                .filter(|s| !known.contains(s.id.as_str()))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let announce = new.first().map(|s| {
                            format!(
                                "new mail for {}: {}",
                                crate::provider::local_part(&address),
                                clean_line(&s.subject)
                            )
                        });
                        for s in &new {
                            self.fresh.insert((address.clone(), s.id.clone()));
                        }
                        mail.messages = list;
                        mail.loaded = true;
                        mail.error = None;
                        if let Some(text) = announce {
                            self.say(text);
                        }
                    }
                    Err(e) => {
                        mail.loaded = true;
                        mail.error = Some(e);
                    }
                }
                if current {
                    // Keep the cursor on the same message as the list shifts.
                    let msgs = self.messages();
                    let n = keep_id
                        .and_then(|id| msgs.iter().position(|s| s.id == id))
                        .or((!msgs.is_empty()).then_some(0));
                    self.msg_state.select(n);
                    self.load_preview();
                }
            }
            Ev::Opened {
                address,
                id,
                result,
            } => match result {
                Ok(msg) => {
                    let key = (address, id);
                    let shown = self.shown.as_ref() == Some(&key);
                    if shown {
                        self.preview = Preview::Ready(msg.clone());
                    }
                    self.cache.insert(key, msg);
                    if shown && self.read_on_load {
                        self.mark_shown_read();
                    }
                }
                Err(e) => {
                    if self.shown.as_ref() == Some(&(address, id)) {
                        self.preview = Preview::Failed(e);
                    }
                }
            },
            Ev::Deleted { address, id } => {
                if let Some(m) = self.mail.get_mut(&address) {
                    m.messages.retain(|s| s.id != id);
                }
                self.cache.remove(&(address, id));
                let len = self.messages().len();
                if self.msg_state.selected().is_some_and(|n| n >= len) {
                    self.msg_state.select(len.checked_sub(1));
                }
                self.load_preview();
                self.say("message deleted");
            }
            Ev::Fatal(_) => unreachable!("handled in run"),
        }
    }

    fn select_inbox(&mut self, n: Option<usize>) {
        self.inbox_state.select(n);
        let has = !self.messages().is_empty();
        self.msg_state.select(has.then_some(0));
        self.load_preview();
    }

    /// Shows the selected message, fetching it unless it is cached.
    /// Marks the message in the preview read, here and (through the
    /// worker) in the state file. Called when the user moves to a message
    /// or into the preview; a preview that loads on its own isn't "read".
    fn mark_shown_read(&mut self) {
        let Some((address, id)) = self.shown.clone() else {
            return;
        };
        // Only a message the user could see: when it has loaded.
        self.read_on_load = !matches!(self.preview, Preview::Ready(_));
        if self.read_on_load {
            return;
        }
        let Some(s) = self
            .mail
            .get_mut(&address)
            .and_then(|m| m.messages.iter_mut().find(|s| s.id == id))
            .filter(|s| !s.seen)
        else {
            return;
        };
        s.seen = true;
        self.send(Req::MarkRead { address, id });
    }

    fn load_preview(&mut self) {
        let key = self.address().zip(self.summary().map(|s| s.id.clone()));
        if key == self.shown && !matches!(self.preview, Preview::Failed(_)) {
            return;
        }
        self.scroll = 0;
        // A retry of the same message (it failed) keeps the wish to read it.
        if key != self.shown {
            self.read_on_load = false;
        }
        self.shown = key.clone();
        self.preview = match key {
            None => Preview::None,
            Some(key) => match self.cache.get(&key) {
                Some(m) => Preview::Ready(m.clone()),
                None => {
                    self.send(Req::Open {
                        address: key.0,
                        id: key.1,
                    });
                    Preview::Loading
                }
            },
        };
    }

    // ----- keys -----

    fn key(&mut self, key: KeyEvent, app: &mut App) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // No Alt bindings; a lone Esc followed by a key can arrive as one.
        if key.modifiers.contains(KeyModifiers::ALT) {
            return;
        }
        if let Some(confirm) = self.confirm.take() {
            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                match confirm {
                    Confirm::DeleteMessage { address, id } => {
                        self.send(Req::DeleteMessage { address, id })
                    }
                    Confirm::Drop(address) => {
                        self.say(format!("dropping {address}"));
                        self.send(Req::Drop(address));
                    }
                }
            }
            return;
        }
        if self.picker.is_some() {
            self.picker_key(key.code);
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => match self.focus {
                Focus::Inboxes => self.quit = true,
                Focus::Messages => self.focus = Focus::Inboxes,
                Focus::Preview => self.focus = Focus::Messages,
            },
            KeyCode::Char('?') => self.help = true,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Inboxes => Focus::Messages,
                    Focus::Messages => Focus::Preview,
                    Focus::Preview => Focus::Inboxes,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Inboxes => Focus::Preview,
                    Focus::Messages => Focus::Inboxes,
                    Focus::Preview => Focus::Messages,
                }
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                self.focus = match self.focus {
                    Focus::Inboxes => Focus::Messages,
                    _ => Focus::Preview,
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.focus = match self.focus {
                    Focus::Preview => Focus::Messages,
                    _ => Focus::Inboxes,
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => self.jump(false),
            KeyCode::End | KeyCode::Char('G') => self.jump(true),
            KeyCode::Char('n') => {
                self.say("creating an inbox...");
                self.send(Req::Create(None));
            }
            KeyCode::Char('N') => {
                // Start on the provider `n` would try first.
                let first = app.config.provider_order().into_iter().next();
                let n = first.and_then(|id| crate::provider::all().position(|p| p == id));
                self.open_picker(n.unwrap_or(0));
            }
            KeyCode::Char('a') => {
                if let Some(a) = self.address() {
                    self.say(format!("{a} is now the active inbox"));
                    self.send(Req::Use(a));
                }
            }
            KeyCode::Char('r') => {
                if let Some(a) = self.address() {
                    self.say("checking...");
                    self.send(Req::Refresh(a));
                }
            }
            KeyCode::Char('c') => {
                if let Some(a) = self.address() {
                    self.copy(app, &a, &a);
                }
            }
            KeyCode::Char('y') => match self.message().map(code_of) {
                Some(Some(code)) => self.copy(app, &code, &format!("code {code}")),
                Some(None) => self.say("no code found in this message"),
                None => {}
            },
            KeyCode::Char('L') => match self.message().map(message_links) {
                Some(links) if !links.is_empty() => self.copy(app, &links[0], "the first link"),
                Some(_) => self.say("no links in this message"),
                None => {}
            },
            KeyCode::Char('O') => match self.message().map(message_links) {
                Some(links) if !links.is_empty() => {
                    match system::open(&links[0], app.browser().as_deref()) {
                        Ok(()) => self.say(format!("opened {}", clean_line(&links[0]))),
                        Err(e) => self.say(format!("could not open the link: {e:#}")),
                    }
                }
                Some(_) => self.say("no links in this message"),
                None => {}
            },
            KeyCode::Char('o') => {
                if let Some(msg) = self.message().cloned() {
                    if let Err(e) = app.open_html(&msg, false) {
                        self.say(format!("could not open the message: {e}"));
                    }
                }
            }
            KeyCode::Char('d') => {
                if let (Some(address), Some(s)) = (self.address(), self.summary()) {
                    self.confirm = Some(Confirm::DeleteMessage {
                        address,
                        id: s.id.clone(),
                    });
                }
            }
            KeyCode::Char('D') => {
                if let Some(a) = self.address() {
                    self.confirm = Some(Confirm::Drop(a));
                }
            }
            _ => {}
        }
    }

    fn open_picker(&mut self, row: usize) {
        self.picker = Some(ListState::default().with_selected(Some(row)));
    }

    /// Keys while the provider picker is open; it takes them all.
    fn picker_key(&mut self, code: KeyCode) {
        let Some(state) = self.picker.as_mut() else {
            return;
        };
        let last = crate::provider::all().count() - 1;
        let n = state.selected().unwrap_or(0);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.picker = None,
            KeyCode::Down | KeyCode::Char('j') => state.select(Some((n + 1).min(last))),
            KeyCode::Up | KeyCode::Char('k') => state.select(Some(n.saturating_sub(1))),
            KeyCode::Home | KeyCode::Char('g') => state.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => state.select(Some(last)),
            KeyCode::Enter => {
                self.picker = None;
                let Some(id) = crate::provider::all().nth(n) else {
                    return;
                };
                let name = crate::provider::get(id).map_or(id, |p| p.name());
                self.say(format!("creating an inbox at {name}..."));
                self.send(Req::Create(Some(id.to_string())));
            }
            _ => {}
        }
    }

    fn copy(&mut self, app: &App, text: &str, what: &str) {
        match system::copy(text, app.clipboard_cmd().as_deref()) {
            Ok(()) => self.say(format!("copied {}", clean_line(what))),
            Err(e) => self.say(format!("could not copy: {e:#}")),
        }
    }

    fn step(&mut self, by: i32) {
        let move_in = |state: &mut ListState, len: usize| {
            if len == 0 {
                return;
            }
            let n = state.selected().unwrap_or(0) as i32 + by;
            state.select(Some(n.clamp(0, len as i32 - 1) as usize));
        };
        match self.focus {
            Focus::Inboxes => {
                let mut state = self.inbox_state.clone();
                move_in(&mut state, self.inboxes.len());
                if state.selected() != self.inbox_state.selected() {
                    self.select_inbox(state.selected());
                }
            }
            Focus::Messages => {
                let len = self.messages().len();
                move_in(&mut self.msg_state, len);
                self.load_preview();
                self.mark_shown_read();
            }
            Focus::Preview => {
                self.scroll = if by > 0 {
                    self.scroll.saturating_add(1)
                } else {
                    self.scroll.saturating_sub(1)
                };
            }
        }
    }

    fn jump(&mut self, end: bool) {
        match self.focus {
            Focus::Inboxes => {
                let n = if end {
                    self.inboxes.len().checked_sub(1)
                } else {
                    Some(0)
                };
                self.select_inbox(n.filter(|_| !self.inboxes.is_empty()));
            }
            Focus::Messages => {
                let len = self.messages().len();
                let n = if end {
                    len.checked_sub(1)
                } else {
                    (len > 0).then_some(0)
                };
                self.msg_state.select(n);
                self.load_preview();
                self.mark_shown_read();
            }
            Focus::Preview => self.scroll = if end { u16::MAX / 2 } else { 0 },
        }
    }

    // ----- drawing -----

    fn draw(&mut self, f: &mut Frame) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(f.area());
        let side = (body.width / 3).clamp(24, 36);
        let [left, right] =
            Layout::horizontal([Constraint::Length(side), Constraint::Min(20)]).areas(body);
        // The list's rows plus its borders, at least 4 and at most a third
        // of the pane plus the borders (that cap wins in a short terminal).
        let rows = u16::try_from(self.messages().len()).unwrap_or(u16::MAX);
        let list_height = rows.saturating_add(2).max(4).min(right.height / 3 + 2);
        let [top, bottom] =
            Layout::vertical([Constraint::Length(list_height), Constraint::Min(3)]).areas(right);

        self.draw_header(f, header);
        self.draw_inboxes(f, left);
        self.draw_messages(f, top);
        self.draw_preview(f, bottom);
        self.draw_footer(f, footer);
        if self.help {
            draw_help(f);
        }
        if let Some(state) = &mut self.picker {
            draw_picker(f, state);
        }
    }

    fn block(&self, title: String, pane: Focus) -> Block<'static> {
        let focused = self.focus == pane;
        Block::bordered()
            .title(Span::styled(
                format!(" {title} "),
                if focused {
                    Style::new().bold()
                } else {
                    Style::new().dim()
                },
            ))
            .border_style(if focused {
                Style::new().fg(Color::Cyan)
            } else {
                Style::new().dim()
            })
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let mut spans = vec![Span::styled(" tmpmail ", Style::new().bold().reversed())];
        if let Some(i) = self.inbox() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(clean_line(&i.address), Style::new().bold()));
            spans.push(Span::styled(
                format!("  {}", i.provider),
                Style::new().dim(),
            ));
        }
        let status = self.inbox().and_then(|i| self.mail.get(&i.address));
        let right = match status {
            _ if self.inbox().is_some_and(Inbox::is_expired) => {
                Span::styled("expired ", Style::new().fg(Color::Red))
            }
            Some(m) if m.error.is_some() => {
                Span::styled("unreachable ", Style::new().fg(Color::Red))
            }
            Some(m) if m.live => Span::styled("● live ", Style::new().fg(Color::Green)),
            Some(Mail {
                checked: Some(t), ..
            }) => Span::styled(
                format!("checked {}s ago ", t.elapsed().as_secs()),
                Style::new().dim(),
            ),
            _ => Span::styled("connecting ", Style::new().dim()),
        };
        f.render_widget(Line::from(spans), area);
        f.render_widget(Line::from(right).right_aligned(), area);
    }

    fn draw_inboxes(&mut self, f: &mut Frame, area: Rect) {
        let block = self.block(format!("Inboxes {}", self.inboxes.len()), Focus::Inboxes);
        if self.inboxes.is_empty() {
            let p = Paragraph::new("No inboxes yet.\nPress n to create one.")
                .wrap(Wrap { trim: false })
                .block(block);
            f.render_widget(p, area);
            return;
        }
        let items: Vec<ListItem> = self
            .inboxes
            .iter()
            .map(|i| {
                let active = self.active.as_deref() == Some(i.address.as_str());
                let unread = self
                    .mail
                    .get(&i.address)
                    .map_or(0, |m| m.messages.iter().filter(|s| !s.seen).count());
                let mut first = vec![
                    Span::styled(
                        if active { "* " } else { "  " },
                        Style::new().fg(Color::Green),
                    ),
                    Span::styled(clean_line(i.local_part()), Style::new().bold()),
                ];
                if unread > 0 {
                    first.push(Span::styled(
                        format!(" {unread}"),
                        Style::new().fg(Color::Cyan),
                    ));
                }
                let domain = i.address.split('@').nth(1).unwrap_or_default();
                let mut second = format!("  @{}", clean_line(domain));
                if let Some(l) = &i.label {
                    second = format!("  [{}] @{}", clean_line(l), clean_line(domain));
                }
                let when = match i.expires_at {
                    _ if i.is_expired() => Span::styled("  expired", Style::new().fg(Color::Red)),
                    Some(t) => {
                        Span::styled(format!("  expires {}", relative(t)), Style::new().dim())
                    }
                    None => Span::raw(""),
                };
                ListItem::new(vec![
                    Line::from(first),
                    Line::from(Span::styled(second, Style::new().dim())),
                    Line::from(when),
                ])
            })
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        f.render_stateful_widget(list, area, &mut self.inbox_state);
    }

    fn draw_messages(&mut self, f: &mut Frame, area: Rect) {
        let address = self.address().unwrap_or_default();
        let mail = self.mail.get(&address);
        let count = mail.map_or(0, |m| m.messages.len());
        let block = self.block(format!("Messages {count}"), Focus::Messages);
        let empty = match mail {
            _ if address.is_empty() => Some("".to_string()),
            None => Some("Loading...".to_string()),
            Some(m) if m.messages.is_empty() => Some(match &m.error {
                Some(e) => format!("Unavailable: {}", clean_line(e)),
                None => "No mail yet. New messages appear here as they arrive.".to_string(),
            }),
            _ => None,
        };
        if let Some(text) = empty {
            f.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .dim()
                    .block(block),
                area,
            );
            return;
        }
        let width = area.width.saturating_sub(2) as usize;
        let items: Vec<ListItem> = self
            .messages()
            .iter()
            .map(|s| {
                let fresh = self.fresh.contains(&(address.clone(), s.id.clone()));
                let marker = match () {
                    _ if fresh => Span::styled("● ", Style::new().fg(Color::Green)),
                    _ if !s.seen => Span::styled("● ", Style::new().fg(Color::Cyan)),
                    _ => Span::raw("  "),
                };
                let when = s.date.map(relative).unwrap_or_default();
                let from_w = (width / 4).clamp(10, 28);
                let from = crate::app::truncate(&clean_line(&sender_name(&s.from)), from_w);
                let subject_style = if s.seen {
                    Style::new()
                } else {
                    Style::new().bold()
                };
                let mut spans = vec![
                    marker,
                    Span::styled(format!("{when:>8}  "), Style::new().dim()),
                    Span::raw(format!("{from:<from_w$}  ")),
                    Span::styled(clean_line(&s.subject), subject_style),
                ];
                if s.has_attachments {
                    spans.push(Span::styled(" +att", Style::new().fg(Color::Yellow)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        f.render_stateful_widget(list, area, &mut self.msg_state);
    }

    fn draw_preview(&mut self, f: &mut Frame, area: Rect) {
        let block = self.block("Message".into(), Focus::Preview);
        let inner = block.inner(area);
        let text = match &self.preview {
            Preview::None => Text::from(""),
            Preview::Loading => Text::from("Loading...").dim(),
            Preview::Failed(e) => {
                Text::from(format!("Could not load this message: {}", clean_line(e))).fg(Color::Red)
            }
            Preview::Ready(msg) => {
                let fresh = self
                    .rendered
                    .as_ref()
                    .is_none_or(|(id, w, _)| *id != msg.id || *w != inner.width);
                if fresh {
                    self.rendered = Some((
                        msg.id.clone(),
                        inner.width,
                        render_message(msg, inner.width),
                    ));
                }
                self.rendered
                    .as_ref()
                    .map(|(_, _, t)| t.clone())
                    .unwrap_or_default()
            }
        };
        // Keep the scroll within the text (lines can wrap, so allow some slack).
        let max = (text.lines.len() as u16).saturating_sub(inner.height / 2);
        self.scroll = self.scroll.min(max);
        let p = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0))
            .block(block);
        f.render_widget(p, area);
    }

    fn draw_footer(&mut self, f: &mut Frame, area: Rect) {
        if let Some(confirm) = &self.confirm {
            let q = match confirm {
                Confirm::DeleteMessage { .. } => "Delete this message?".to_string(),
                Confirm::Drop(a) => format!("Delete inbox {} for good?", clean_line(a)),
            };
            let line = Line::from(vec![
                Span::styled(format!(" {q} "), Style::new().fg(Color::Yellow).bold()),
                Span::raw(" y to confirm, any other key to cancel"),
            ]);
            f.render_widget(line, area);
            return;
        }
        if self
            .status
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > STATUS_TTL)
        {
            self.status = None;
        }
        let keys: &[(&str, &str)] = match self.focus {
            Focus::Inboxes => &[
                ("n/N", "new"),
                ("c", "copy"),
                ("a", "make active"),
                ("D", "delete inbox"),
            ],
            Focus::Messages => &[
                ("y", "copy code"),
                ("o", "open"),
                ("d", "delete"),
                ("r", "refresh"),
            ],
            Focus::Preview => &[
                ("y", "copy code"),
                ("L", "copy link"),
                ("O", "open link"),
                ("o", "open"),
            ],
        };
        let mut spans = vec![Span::raw(" ")];
        for (k, what) in keys.iter().chain(&[("?", "help"), ("q", "quit")]) {
            spans.push(Span::styled(*k, Style::new().bold()));
            spans.push(Span::styled(format!(" {what}  "), Style::new().dim()));
        }
        f.render_widget(Line::from(spans), area);
        if let Some((msg, _)) = &self.status {
            f.render_widget(
                Line::from(Span::styled(
                    format!("{} ", clean_line(msg)),
                    Style::new().fg(Color::Cyan),
                ))
                .right_aligned(),
                area,
            );
        }
    }
}

/// The detected verification code, if the message has one.
fn code_of(msg: &Message) -> Option<String> {
    extract::otp(&msg.subject, &message_body(msg), None)
}

/// `Name <addr>` -> `Name`; a bare address stays as is.
fn sender_name(from: &str) -> String {
    match from.split_once('<') {
        Some((name, _)) if !name.trim().is_empty() => name.trim().trim_matches('"').to_string(),
        _ => from.trim_matches(|c| c == '<' || c == '>').to_string(),
    }
}

/// Headers, the code strip (when there is a code), attachments and body.
fn render_message(msg: &Message, width: u16) -> Text<'static> {
    let key = |k: &str| Span::styled(format!("{k:<9}"), Style::new().dim());
    let mut lines = vec![
        Line::from(vec![key("From"), Span::raw(clean_line(&msg.from))]),
        Line::from(vec![key("To"), Span::raw(clean_line(&msg.to.join(", ")))]),
        Line::from(vec![
            key("Subject"),
            Span::styled(clean_line(&msg.subject), Style::new().bold()),
        ]),
    ];
    if let Some(d) = msg.date {
        let local = d.with_timezone(&chrono::Local);
        lines.push(Line::from(vec![
            key("Date"),
            Span::raw(format!(
                "{} ({})",
                local.format("%Y-%m-%d %H:%M"),
                relative(d)
            )),
        ]));
    }
    if !msg.attachments.is_empty() {
        let names: Vec<String> = msg
            .attachments
            .iter()
            .map(|a| clean_line(&a.filename))
            .collect();
        lines.push(Line::from(vec![
            key("Files"),
            Span::styled(names.join(", "), Style::new().fg(Color::Yellow)),
        ]));
    }
    lines.push(Line::from(""));
    if let Some(code) = code_of(msg) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}  ", clean_line(&code)),
                Style::new().fg(Color::Black).bg(Color::Yellow).bold(),
            ),
            Span::styled("  y copies the code", Style::new().dim()),
        ]));
        lines.push(Line::from(""));
    }
    let body = body_text(
        msg.text.as_deref(),
        msg.html.as_deref(),
        (width as usize).clamp(20, 100),
    );
    for line in clean(body.trim_end()).lines() {
        lines.push(Line::from(line.to_string()));
    }
    Text::from(lines)
}

fn draw_help(f: &mut Frame) {
    const KEYS: &[(&str, &str)] = &[
        ("Tab / ← →", "move between panes"),
        ("↑ ↓ / j k", "move, or scroll the message"),
        ("PgUp PgDn", "scroll the message"),
        ("n", "create a new inbox"),
        ("N", "create an inbox at a chosen provider"),
        ("a", "make the selected inbox the active one"),
        ("c", "copy the inbox address"),
        ("D", "delete the inbox"),
        ("y", "copy the verification code"),
        ("L / O", "copy / open the first link"),
        ("o", "open the message in the browser"),
        ("d", "delete the message"),
        ("r", "check the inbox now"),
        ("q", "quit"),
    ];
    let area = f.area();
    let w = 54.min(area.width);
    let h = (KEYS.len() as u16 + 2).min(area.height);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(format!(" {k:<12}"), Style::new().bold()),
                Span::raw(*what),
            ])
        })
        .collect();
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Keys ")
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        rect,
    );
}

/// The provider picker: one row per provider, with its lifetime, and a
/// warning on those whose inboxes anyone can read.
fn draw_picker(f: &mut Frame, state: &mut ListState) {
    let rows: Vec<(&str, &str, bool)> = crate::provider::all()
        .filter_map(|id| crate::provider::get(id).ok())
        .map(|p| (p.name(), p.caps().lifetime, p.caps().private))
        .collect();
    let name_w = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let items: Vec<ListItem> = rows
        .iter()
        .map(|(name, lifetime, private)| {
            let mut spans = vec![
                Span::styled(format!(" {name:<name_w$}  "), Style::new().bold()),
                Span::styled(*lifetime, Style::new().dim()),
            ];
            if !private {
                spans.push(Span::styled(
                    "  public",
                    Style::new().fg(Color::Yellow).dim(),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let widest = rows
        .iter()
        .map(|(_, l, private)| name_w + 3 + l.chars().count() + if *private { 0 } else { 8 })
        .max()
        .unwrap_or(0);
    let area = f.area();
    let w = (widest as u16 + 3).min(area.width);
    let h = (rows.len() as u16 + 2).min(area.height);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, rect);
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Provider ")
                .border_style(Style::new().fg(Color::Cyan)),
        )
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, rect, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_names() {
        assert_eq!(sender_name("GitHub <noreply@github.com>"), "GitHub");
        assert_eq!(sender_name("\"Acme Inc\" <a@acme.io>"), "Acme Inc");
        assert_eq!(sender_name("<a@b.c>"), "a@b.c");
        assert_eq!(sender_name("a@b.c"), "a@b.c");
    }

    #[test]
    fn the_code_strip_shows_up_for_codes_only() {
        let msg = Message {
            id: "1".into(),
            subject: "Your verification code".into(),
            text: Some("Your code is 482913. It expires in 10 minutes.".into()),
            ..Default::default()
        };
        let text = render_message(&msg, 80);
        assert!(
            text.lines
                .iter()
                .any(|l| l.to_string().contains("482913  "))
        );
        let plain = Message {
            text: Some("Hello there".into()),
            ..msg
        };
        let text = render_message(&plain, 80);
        assert!(
            !text
                .lines
                .iter()
                .any(|l| l.to_string().contains("y copies"))
        );
    }

    fn summary(id: &str, subject: &str) -> Summary {
        Summary {
            id: id.into(),
            subject: subject.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_message_is_read_once_the_user_moves_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        let (tx, rx) = channel();
        let mut ui = Ui::new(tx);
        let inbox = Inbox::new("mailtm", "a@x.com".into(), serde_json::Value::Null);
        ui.event(Ev::Inboxes {
            inboxes: vec![inbox],
            active: None,
        });
        ui.focus = Focus::Messages;
        ui.event(Ev::Listed {
            address: "a@x.com".into(),
            result: Ok(vec![summary("2", "new"), summary("1", "old")]),
            live: false,
        });
        // The preview opened on its own: shown, not read.
        assert!(matches!(rx.try_recv(), Ok(Req::Open { id, .. }) if id == "2"));
        assert!(rx.try_recv().is_err());
        assert!(!ui.summary().unwrap().seen);

        press(&mut ui, &mut app, KeyCode::Char('j'));
        assert!(matches!(rx.try_recv(), Ok(Req::Open { id, .. }) if id == "1"));
        // Not before it has loaded (it may fail to).
        assert!(rx.try_recv().is_err());
        assert!(!ui.summary().unwrap().seen);
        let opened = |id: &str| Ev::Opened {
            address: "a@x.com".into(),
            id: id.into(),
            result: Ok(Box::new(Message {
                id: id.into(),
                ..Default::default()
            })),
        };
        // The automatic preview's answer, now off screen, marks nothing.
        ui.event(opened("2"));
        assert!(rx.try_recv().is_err());
        ui.event(opened("1"));
        assert!(matches!(rx.try_recv(), Ok(Req::MarkRead { id, .. }) if id == "1"));
        assert!(ui.summary().unwrap().seen);
        // Only once; back on the cached "2", it is read at once.
        press(&mut ui, &mut app, KeyCode::Char('j'));
        assert!(rx.try_recv().is_err());
        press(&mut ui, &mut app, KeyCode::Char('k'));
        assert!(matches!(rx.try_recv(), Ok(Req::MarkRead { id, .. }) if id == "2"));
    }

    #[test]
    fn mail_arriving_later_is_marked_new_and_announced() {
        let (tx, rx) = channel();
        let mut ui = Ui::new(tx);
        let inbox = Inbox::new("mailtm", "a@x.com".into(), serde_json::Value::Null);
        ui.event(Ev::Inboxes {
            inboxes: vec![inbox],
            active: None,
        });
        let listed = |list: Vec<Summary>| Ev::Listed {
            address: "a@x.com".into(),
            result: Ok(list),
            live: true,
        };
        ui.event(listed(vec![summary("1", "old")]));
        assert!(ui.fresh.is_empty(), "the first listing is not news");
        assert!(ui.status.is_none());
        assert_eq!(ui.msg_state.selected(), Some(0));
        // Selecting the message asked the worker for it.
        assert!(matches!(rx.try_recv(), Ok(Req::Open { id, .. }) if id == "1"));

        ui.event(listed(vec![summary("2", "Your code"), summary("1", "old")]));
        assert!(ui.fresh.contains(&("a@x.com".to_string(), "2".to_string())));
        assert!(ui.status.as_ref().unwrap().0.contains("Your code"));
        // The cursor stays on the message it was on.
        assert_eq!(ui.summary().unwrap().id, "1");
    }

    /// A screen with an inbox, mail, a loaded preview and a status line.
    fn busy_ui() -> Ui {
        let (tx, _rx) = channel();
        let mut ui = Ui::new(tx);
        let mut inbox = Inbox::new("mailtm", "a@x.com".into(), serde_json::Value::Null);
        inbox.label = Some("shop".into());
        ui.event(Ev::Inboxes {
            inboxes: vec![inbox],
            active: Some("a@x.com".into()),
        });
        let list: Vec<Summary> = (1..=12)
            .map(|n| summary(&n.to_string(), "Your verification code"))
            .collect();
        ui.event(Ev::Listed {
            address: "a@x.com".into(),
            result: Ok(list),
            live: true,
        });
        ui.event(Ev::Opened {
            address: "a@x.com".into(),
            id: "1".into(),
            result: Ok(Box::new(Message {
                id: "1".into(),
                from: "Demo <hello@example.org>".into(),
                subject: "Your verification code".into(),
                text: Some("Your code is 482913.\n".repeat(40)),
                ..Default::default()
            })),
        });
        ui.say("a status line that is longer than a small terminal is wide");
        ui
    }

    #[test]
    fn tiny_terminals_draw_without_panicking() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let sizes = (1..=40u16)
            .flat_map(|w| (1..=10u16).map(move |h| (w, h)))
            .chain([(120, 7), (10, 4), (30, 8), (80, 24), (200, 3)]);
        let (tx, _rx) = channel();
        let mut screens = vec![Ui::new(tx)]; // no inboxes yet
        for focus in [Focus::Inboxes, Focus::Messages, Focus::Preview] {
            let mut ui = busy_ui();
            ui.focus = focus;
            screens.push(ui);
        }
        let mut ui = busy_ui();
        ui.focus = Focus::Preview;
        ui.jump(true); // scrolled to the end
        screens.push(ui);
        let mut ui = busy_ui();
        ui.help = true;
        screens.push(ui);
        let mut ui = busy_ui();
        ui.open_picker(0);
        screens.push(ui);
        let mut ui = busy_ui();
        ui.confirm = Some(Confirm::Drop("a@x.com".into()));
        screens.push(ui);
        // Each screen is redrawn at every size, as a resize would.
        for (w, h) in sizes {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            for ui in &mut screens {
                terminal
                    .draw(|f| ui.draw(f))
                    .unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
            }
        }
    }

    fn press(ui: &mut Ui, app: &mut App, code: KeyCode) {
        ui.key(KeyEvent::new(code, KeyModifiers::NONE), app);
    }

    #[test]
    fn the_provider_picker_closes_without_quitting() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        let (tx, rx) = channel();
        let mut ui = Ui::new(tx);
        press(&mut ui, &mut app, KeyCode::Char('N'));
        // The default order starts with mailtm, the first row.
        assert_eq!(ui.picker.as_ref().unwrap().selected(), Some(0));
        press(&mut ui, &mut app, KeyCode::Char('x'));
        assert!(ui.picker.is_some(), "other keys are ignored");
        press(&mut ui, &mut app, KeyCode::Esc);
        assert!(ui.picker.is_none());
        assert!(!ui.quit);

        press(&mut ui, &mut app, KeyCode::Char('N'));
        press(&mut ui, &mut app, KeyCode::Char('q'));
        assert!(ui.picker.is_none());
        assert!(!ui.quit, "q closes the picker, not the app");
        assert!(rx.try_recv().is_err(), "nothing was requested");
    }

    #[test]
    fn the_provider_picker_creates_at_the_chosen_provider() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_test(dir.path().to_path_buf());
        let (tx, rx) = channel();
        let mut ui = Ui::new(tx);
        press(&mut ui, &mut app, KeyCode::Char('N'));
        press(&mut ui, &mut app, KeyCode::Down);
        press(&mut ui, &mut app, KeyCode::Enter);
        assert!(ui.picker.is_none());
        let second = crate::provider::all().nth(1).unwrap();
        assert!(matches!(rx.try_recv(), Ok(Req::Create(Some(id))) if id == second));
        assert!(
            ui.status
                .as_ref()
                .unwrap()
                .0
                .starts_with("creating an inbox at ")
        );

        // `G` goes to the last row and the cursor stops there.
        press(&mut ui, &mut app, KeyCode::Char('N'));
        press(&mut ui, &mut app, KeyCode::Char('G'));
        press(&mut ui, &mut app, KeyCode::Char('j'));
        press(&mut ui, &mut app, KeyCode::Enter);
        let last = crate::provider::all().last().unwrap();
        assert!(matches!(rx.try_recv(), Ok(Req::Create(Some(id))) if id == last));

        press(&mut ui, &mut app, KeyCode::Char('n'));
        assert!(matches!(rx.try_recv(), Ok(Req::Create(None))));
    }
}
