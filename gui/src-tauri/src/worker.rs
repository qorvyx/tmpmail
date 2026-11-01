//! The background thread that does all network work: the core's shared
//! worker loop (`tmpmail::watcher`, also behind the TUI) with the window's
//! requests and events. It owns its own `App`, polls every stored inbox
//! (with push where the provider has it), and re-reads the state file every
//! few seconds so changes made by the CLI show up. Results go to the window
//! as events.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, UriSchemeResponder};
use tmpmail::app::{App, Global};
use tmpmail::config::Config;
use tmpmail::provider::{self, CreateOpts, Message, Summary};
use tmpmail::render::clean_line;
use tmpmail::watcher::{self, CreateField, Watcher};

use crate::notify::{self, Arrivals};
use crate::{Mails, Shared};
use crate::{commands, scheme, tray};

pub enum Req {
    /// The window just loaded: send the inbox list and every inbox's last
    /// listing again, since events sent before it listened are lost.
    Refresh,
    Create {
        provider: Option<String>,
        opts: CreateOpts,
        label: Option<String>,
        /// From the tray menu: copy the address and say so in a notification.
        from_tray: bool,
    },
    /// The settings were saved: read the config again.
    Reload,
    /// List this inbox now instead of on its next tick.
    Select {
        address: String,
    },
    Use {
        address: String,
    },
    DeleteInbox {
        address: String,
    },
    Open {
        address: String,
        id: String,
    },
    DeleteMessage {
        address: String,
        id: String,
    },
    /// An inline image of an opened message, for the mail scheme.
    Inline {
        key: u64,
        cid: String,
        responder: UriSchemeResponder,
    },
    /// An opened message's source, for the mail scheme.
    Source {
        key: u64,
        theme: scheme::Theme,
        responder: UriSchemeResponder,
    },
    /// An opened message's attachment, for `save_attachment`.
    Attachment {
        key: u64,
        index: usize,
        reply: Sender<Result<Vec<u8>, String>>,
    },
}

/// Starts the worker once its `App` has loaded the config and state, or
/// returns why they couldn't be loaded.
pub fn spawn(handle: AppHandle, shared: Arc<Shared>) -> Result<Sender<Req>, String> {
    let (tx, rx) = channel();
    let (ready_tx, ready_rx) = channel();
    std::thread::Builder::new()
        .name("tmpmail-worker".into())
        .spawn(move || {
            // Built here: an `App` can't move between threads.
            let mut w = match Watcher::new(Global::default()) {
                Ok(w) => w,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            let (log_tx, log) = channel();
            w.app.log_to(log_tx);
            w.run(
                &rx,
                &mut Front {
                    handle,
                    shared,
                    log,
                    last: HashMap::new(),
                    arrivals: Arrivals::default(),
                },
            );
        })
        .map_err(|e| format!("could not start the worker: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(tx),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the worker stopped".into()),
    }
}

/// The window's side of the shared loop.
struct Front {
    handle: AppHandle,
    shared: Arc<Shared>,
    log: Receiver<String>,
    /// The last `messages` event of each inbox, for `Req::Refresh`.
    last: HashMap<String, Value>,
    /// New mail waiting for its notification.
    arrivals: Arrivals,
}

impl Front {
    fn messages(&mut self, address: String, payload: Value) -> bool {
        let ok = self.handle.emit("messages", &payload).is_ok();
        self.last.insert(address, payload);
        ok
    }

    /// A toast in the window. Its text often quotes a provider's error, so
    /// it is cleaned here, once for every notice.
    fn notice(&self, kind: &str, text: &str) {
        let _ = self
            .handle
            .emit("notice", json!({ "kind": kind, "text": clean_line(text) }));
    }
}

impl watcher::Front for Front {
    type Req = Req;

    fn is_open(req: &Req) -> bool {
        matches!(req, Req::Open { .. })
    }

    fn inboxes(&mut self, app: &App) -> bool {
        *self.shared.inboxes.lock().expect("inboxes lock") = app
            .state
            .inboxes
            .iter()
            .map(|i| (i.address.clone(), i.provider.clone()))
            .collect();
        self.last
            .retain(|address, _| app.state.position(address).is_some());
        self.arrivals
            .retain(|address| app.state.position(address).is_some());
        let inboxes: Vec<_> = app.state.inboxes.iter().map(|i| app.view(i)).collect();
        self.handle
            .emit("inboxes", json!({ "inboxes": inboxes }))
            .is_ok()
    }

    fn listed(
        &mut self,
        address: String,
        result: Result<Vec<Summary>, String>,
        live: bool,
    ) -> bool {
        let payload = match result {
            Ok(list) => {
                // Codes are looked for in the sender's own words, as when
                // the message is opened; only what the page shows is cleaned.
                let arrived = self.arrivals.listed(&address, &list);
                for a in &arrived {
                    if let Some(code) = &a.code {
                        tray::saw_code(&self.shared, code, a.date);
                    }
                }
                self.arrivals.queue(arrived);
                let list: Vec<Summary> = list.into_iter().map(shown).collect();
                json!({ "address": address, "live": live, "messages": list })
            }
            Err(e) => json!({ "address": address, "live": live, "error": clean_line(&e) }),
        };
        self.messages(address, payload)
    }

    fn expired(&mut self, address: &str) -> bool {
        let payload = json!({ "address": address, "expired": true });
        self.messages(address.to_string(), payload)
    }

    fn turn(&mut self) {
        let notes: Vec<String> = self.log.try_iter().collect();
        for note in notes {
            self.notice("info", &note);
        }
        if let Some(batch) = self.arrivals.due()
            && !batch.is_empty()
            && notify::wanted(&self.shared)
        {
            let (title, body) = notify::text(&batch);
            notify::show(
                &self.handle,
                &self.shared,
                &title,
                &body,
                notify::target(&batch),
            );
        }
        tray::refresh(&self.shared);
    }

    fn handle(&mut self, w: &mut Watcher, req: Req) -> bool {
        match req {
            Req::Refresh => {
                // Inboxes not listed yet are listed on their own soon.
                for inbox in &w.app.state.inboxes {
                    if let Some(payload) = self.last.get(&inbox.address) {
                        let _ = self.handle.emit("messages", payload);
                    }
                }
                true
            }
            Req::Select { address } => {
                w.poll_now(&address);
                false
            }
            Req::Create {
                provider,
                opts,
                label,
                from_tray,
            } => {
                let asked = label.clone();
                match w.create_with(provider.as_deref(), &opts, label) {
                    Ok(address) => {
                        // The list first: the window selects the new inbox
                        // at once, and commands must already know it.
                        watcher::Front::inboxes(self, &w.app);
                        let _ = self.handle.emit(
                            "created",
                            json!({ "address": address, "from_tray": from_tray }),
                        );
                        if from_tray {
                            self.announce(&address);
                        }
                    }
                    Err(e) if from_tray => {
                        let text =
                            format!("{}. Try again, or pick a provider in the app.", capital(&e));
                        notify::show(
                            &self.handle,
                            &self.shared,
                            "Couldn't create an address",
                            &text,
                            None,
                        );
                    }
                    Err(e) => {
                        // A taken label or username, refused before any
                        // request: the dialog shows it by that field.
                        let payload = match w.refused(&opts, asked.as_deref()) {
                            Some((field, why)) => json!({
                                "text": clean_line(&format!("{}.", capital(&why))),
                                "field": match field {
                                    CreateField::User => "user",
                                    _ => "label",
                                },
                            }),
                            None => json!({
                                "text": clean_line(&format!(
                                    "{}. Try another provider, or leave the options empty.",
                                    capital(&e)
                                )),
                            }),
                        };
                        let _ = self.handle.emit("create_failed", payload);
                    }
                }
                false
            }
            Req::Reload => {
                match Config::load() {
                    Ok(config) => w.set_config(config),
                    Err(e) => self.notice(
                        "error",
                        &format!(
                            "The settings were saved, but tmpmail couldn't read them back: {e}"
                        ),
                    ),
                }
                false
            }
            Req::Use { address } => {
                match w.make_active(&address) {
                    Ok(()) => {
                        self.notice("success", &format!("{} is now the active inbox", address))
                    }
                    Err(e) => {
                        self.notice("error", &format!("Couldn't make it the active inbox: {e}"))
                    }
                }
                true
            }
            Req::DeleteInbox { address } => {
                let name = provider_name(&w.app, &address);
                let mut problems = Vec::new();
                let known = w.drop_inbox(&address, |what, e| problems.push(format!("{what}: {e}")));
                if known && problems.is_empty() {
                    self.notice("success", &format!("Deleted {address}"));
                } else if known {
                    let text = format!("{name}: {}", problems.join("; "));
                    self.notice("error", &text);
                }
                true
            }
            Req::Open { address, id } => {
                self.open(&mut w.app, address, id);
                false
            }
            Req::DeleteMessage { address, id } => {
                let name = provider_name(&w.app, &address);
                match w.delete_message(&address, &id) {
                    Some(Ok(())) => {
                        let _ = self
                            .handle
                            .emit("deleted", json!({ "address": address, "id": id }));
                        self.notice("success", "Message deleted");
                    }
                    Some(Err(e)) => self.notice(
                        "error",
                        &format!("Couldn't delete the message at {name}: {e}. Refresh the inbox and try again."),
                    ),
                    None => {}
                }
                false
            }
            Req::Source {
                key,
                theme,
                responder,
            } => {
                self.source(&mut w.app, key, theme, responder);
                false
            }
            Req::Attachment { key, index, reply } => {
                let _ = reply.send(self.attachment(&mut w.app, key, index));
                false
            }
            Req::Inline {
                key,
                cid,
                responder,
            } => {
                self.inline(&mut w.app, key, cid, responder);
                false
            }
        }
    }
}

impl Front {
    /// A new address from the tray: copied, and said so where the user
    /// looks (the window may be hidden).
    fn announce(&self, address: &str) {
        let copied = tauri_plugin_clipboard_manager::ClipboardExt::clipboard(&self.handle)
            .write_text(address)
            .is_ok();
        let title = if copied {
            "New address copied"
        } else {
            "New address (couldn't copy it)"
        };
        let target = notify::Target {
            address: address.to_string(),
            id: None,
        };
        notify::show(
            &self.handle,
            &self.shared,
            title,
            &clean_line(address),
            Some(target),
        );
    }

    fn open(&self, app: &mut App, address: String, id: String) {
        let Some(n) = app.state.position(&address) else {
            return;
        };
        let (payload, opened) = match app.with_inbox_at(n, |p, inbox| p.get(inbox, &id)) {
            Ok(msg) => {
                let mail = Mails::entry(&address, msg);
                let msg = &mail.msg;
                if let Some(code) = &mail.code {
                    tray::saw_code(&self.shared, code, msg.date);
                }
                let attachments: Vec<_> = msg
                    .attachments
                    .iter()
                    .map(|a| {
                        json!({
                            "filename": clean_line(&a.filename),
                            "content_type": clean_line(&a.content_type),
                            "size": a.size,
                        })
                    })
                    .collect();
                let mut event = json!({
                    "address": address,
                    "id": id,
                    "from": clean_line(&msg.from),
                    "to": msg.to.iter().map(|t| clean_line(t)).collect::<Vec<_>>(),
                    "subject": clean_line(&msg.subject),
                    "date": msg.date,
                    "attachments": attachments,
                    "code": mail.code,
                    "sender": commands::sender_address(&msg.from),
                    "links": mail.links,
                    "html": msg.html.is_some(),
                });
                let key = self.shared.mails.lock().expect("mails lock").insert(mail);
                event["key"] = json!(key);
                (event, true)
            }
            Err(e) => (
                json!({ "address": address, "id": id, "error": clean_line(&e.to_string()) }),
                false,
            ),
        };
        let _ = self.handle.emit("opened", payload);
        // Read now, in the state file the CLI and TUI read too; after the
        // page has the message, so the file write doesn't delay it.
        if opened {
            app.mark_read_noting(n, &id);
        }
    }

    /// Serves an inline image, fetching it from the provider the first
    /// time and keeping it with the message after that. The image is the
    /// attachment the provider lists under the cid's name; else (mail.tm
    /// lists it by file name only, or the download failed) the part with
    /// that `Content-ID` in the raw source, where the provider has one. The
    /// source is read once for all the message's images.
    fn inline(&self, app: &mut App, key: u64, cid: String, responder: UriSchemeResponder) {
        let found = {
            let mails = self.shared.mails.lock().expect("mails lock");
            mails.get(key).map(|m| {
                (
                    m.inline.get(&cid).cloned(),
                    m.address.clone(),
                    m.msg.clone(),
                )
            })
        };
        let (address, msg) = match found {
            None => return responder.respond(scheme::not_found()),
            // Fetched while this request was queued.
            Some((Some((mime, bytes)), ..)) => {
                return responder.respond(scheme::image(&mime, bytes));
            }
            Some((None, address, msg)) => (address, msg),
        };
        let listed = provider::inline_attachment(&msg, &cid).and_then(|(att, mime)| {
            let n = app.state.position(&address)?;
            app.with_inbox_at(n, |p, inbox| p.attachment(inbox, &msg, att))
                .ok()
                .filter(|bytes| bytes.len() <= provider::MAX_INLINE_IMAGE)
                .map(|bytes| (mime, bytes))
        });
        let image = match listed {
            Some(image) => {
                if let Some(m) = self.shared.mails.lock().expect("mails lock").get_mut(key) {
                    m.inline.insert(cid, image.clone());
                }
                Some(image)
            }
            None => {
                self.source_images(app, key, &address, &msg);
                let mails = self.shared.mails.lock().expect("mails lock");
                mails.get(key).and_then(|m| m.inline.get(&cid).cloned())
            }
        };
        match image {
            Some((mime, bytes)) => responder.respond(scheme::image(&mime, bytes)),
            None => responder.respond(scheme::not_found()),
        }
    }

    /// Puts the inline images of an opened message's raw source (where its
    /// provider has one) with the message, once: the source is fetched,
    /// read for all its images, and kept for the Source view too.
    fn source_images(&self, app: &mut App, key: u64, address: &str, msg: &Message) {
        {
            let mut mails = self.shared.mails.lock().expect("mails lock");
            match mails.get_mut(key) {
                Some(m) if !m.source_images => m.source_images = true,
                _ => return,
            }
        }
        let Some(n) = app.state.position(address) else {
            return;
        };
        let id = app.state.inboxes[n].provider.clone();
        if !provider::get(&id).is_ok_and(|p| p.caps().raw) {
            return;
        }
        let Ok(raw) = app.with_inbox_at(n, |p, inbox| p.raw(inbox, &msg.id)) else {
            return;
        };
        let images = provider::inline_images(&raw);
        let mut mails = self.shared.mails.lock().expect("mails lock");
        if let Some(m) = mails.get_mut(key) {
            for (cid, image) in images {
                m.inline.entry(cid).or_insert(image);
            }
            if m.source.is_none() {
                m.source = Some(Arc::new(String::from_utf8_lossy(&raw).into_owned()));
            }
        }
    }

    /// The opened message's source: raw MIME where the provider has it,
    /// else its HTML body. Fetched once, then kept with the message.
    fn source(&self, app: &mut App, key: u64, theme: scheme::Theme, responder: UriSchemeResponder) {
        let found = {
            let mails = self.shared.mails.lock().expect("mails lock");
            mails
                .get(key)
                .map(|m| (m.source.clone(), m.address.clone(), m.msg.clone()))
        };
        let (address, msg) = match found {
            None => return responder.respond(scheme::not_found()),
            Some((Some(text), ..)) => {
                return responder.respond(scheme::source_response(&text, theme));
            }
            Some((None, address, msg)) => (address, msg),
        };
        let body = || {
            msg.html
                .clone()
                .or_else(|| msg.text.clone())
                .unwrap_or_default()
        };
        // Only a source that was really read is kept: errors are retried
        // the next time.
        let (text, keep) = match app.state.position(&address) {
            None => (
                "This message's inbox was deleted, so its source can't be fetched.".to_string(),
                false,
            ),
            Some(n) => {
                let id = app.state.inboxes[n].provider.clone();
                if provider::get(&id).is_ok_and(|p| p.caps().raw) {
                    match app.with_inbox_at(n, |p, inbox| p.raw(inbox, &msg.id)) {
                        Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), true),
                        Err(e) => (
                            format!(
                                "Couldn't fetch the source: {e}. Turn Source off and on to try again."
                            ),
                            false,
                        ),
                    }
                } else {
                    (body(), true)
                }
            }
        };
        let text = Arc::new(text);
        if keep && let Some(m) = self.shared.mails.lock().expect("mails lock").get_mut(key) {
            m.source = Some(text.clone());
        }
        // Escaping a big source takes a while: off this thread too.
        tauri::async_runtime::spawn_blocking(move || {
            responder.respond(scheme::source_response(&text, theme));
        });
    }

    fn attachment(&self, app: &mut App, key: u64, index: usize) -> Result<Vec<u8>, String> {
        let (address, msg) = {
            let mails = self.shared.mails.lock().expect("mails lock");
            let m = mails.get(key).ok_or("that message is no longer open")?;
            (m.address.clone(), m.msg.clone())
        };
        let att = msg.attachments.get(index).ok_or("no such attachment")?;
        let n = app
            .state
            .position(&address)
            .ok_or("that inbox was deleted")?;
        let name = provider_name(app, &address);
        app.with_inbox_at(n, |p, inbox| p.attachment(inbox, &msg, att))
            .map_err(|e| {
                clean_line(&format!(
                    "Couldn't download it from {name}: {e}. Try again in a moment."
                ))
            })
    }
}

/// A listing entry as the page shows it: the sender's words cleaned
/// (`render::clean_line`: escapes, control, bidi and other invisible format
/// characters), so a list row or the "New mail" notice can't be turned
/// around or made to show another address. The id stays as the provider
/// gave it: commands need it back unchanged, and the page never shows it.
pub fn shown(mut s: Summary) -> Summary {
    s.from = clean_line(&s.from);
    s.subject = clean_line(&s.subject);
    s.intro = clean_line(&s.intro);
    s
}

/// The display name of an inbox's provider.
fn provider_name(app: &App, address: &str) -> &'static str {
    app.state
        .position(address)
        .and_then(|n| provider::get(&app.state.inboxes[n].provider).ok())
        .map_or("the provider", |p| p.name())
}

/// The core's messages start lower case; notices are sentences.
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_rows_get_the_senders_words_cleaned() {
        // The hostile mail of campaign round 2 (B1).
        let raw = Summary {
            id: "m\u{202e}1".into(),
            from: "\"<b>Evil</b> & Co \u{202e}boN\" <evil@fldubai.com>".into(),
            subject: "\u{1b}[31mRED\u{1b}[0m \u{202e}desrever\u{200b}".into(),
            intro: "line\none\u{2066}".into(),
            ..Summary::default()
        };
        let row = shown(raw.clone());
        assert_eq!(row.from, "\"<b>Evil</b> & Co boN\" <evil@fldubai.com>");
        assert_eq!(row.subject, "RED desrever");
        assert_eq!(row.intro, "line one");
        assert_eq!(row.id, raw.id, "ids go back to the provider unchanged");
    }
}
