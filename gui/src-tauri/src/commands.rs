//! The commands the window may call, and only these (`capabilities/main.json`
//! and `build.rs` list the same set). Each one checks what it is given: an
//! address must be a stored inbox, a message key one the app opened, an
//! index one of that message's links or attachments. What the page gets back
//! never includes credentials.

use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::channel;

use serde_json::{Value, json};
use tauri::{AppHandle, State, Url, WebviewWindow};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tmpmail::provider::{self, CreateOpts};
use tmpmail::render::{clean_line, safe_filename};

use crate::worker::Req;
use crate::{Shared, drag, scheme, settings, tray};

pub fn send(shared: &Shared, req: Req) -> Result<(), String> {
    if let Some(e) = shared.start_error.get() {
        return Err(e.clone());
    }
    shared
        .worker
        .get()
        .ok_or("the worker is not running")?
        .send(req)
        .map_err(|_| "the worker stopped".to_string())
}

/// The provider id of a stored inbox, or an error for any other address.
fn stored(shared: &Shared, address: &str) -> Result<String, String> {
    shared
        .inboxes
        .lock()
        .expect("inboxes lock")
        .get(address)
        .cloned()
        .ok_or_else(|| format!("{} is not a stored inbox", clean_line(address)))
}

fn provider_of(id: &str) -> Result<Box<dyn provider::Provider>, String> {
    if !provider::all().any(|p| p == id) {
        return Err(format!("unknown provider {:?}", clean_line(id)));
    }
    provider::get(id).map_err(|e| e.to_string())
}

/// Message ids come from the providers: any printable text, but not huge.
fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
        return Err("invalid message id".into());
    }
    Ok(())
}

/// An http or https link, the only kind the app opens: the core's rule
/// ([`tmpmail::extract::is_web_link`]: absolute, with a host, nothing a
/// system opener could take for a file path).
pub fn web_url(s: &str) -> Option<Url> {
    tmpmail::extract::is_web_link(s)
        .then(|| Url::parse(s).ok())
        .flatten()
}

fn optional(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The new inbox's options, checked: a username of letters, digits, `.`,
/// `_` and `-`; a domain name; a one-line label.
fn create_opts(
    user: Option<String>,
    domain: Option<String>,
    label: Option<String>,
) -> Result<(CreateOpts, Option<String>), String> {
    let user = optional(user).map(|u| u.to_ascii_lowercase());
    if let Some(u) = &user
        && (u.len() > 64
            || !u
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Err(
            "a username can only have letters, digits, dots, dashes and underscores".into(),
        );
    }
    let domain = optional(domain).map(|d| d.to_ascii_lowercase());
    if let Some(d) = &domain
        && !is_domain(d)
    {
        return Err(format!("{:?} is not a domain name", clean_line(d)));
    }
    let label = optional(label).map(|l| clean_line(&l));
    if label.as_ref().is_some_and(|l| l.chars().count() > 64) {
        return Err("a label can be at most 64 characters".into());
    }
    let opts = CreateOpts {
        user,
        domain,
        password: None,
    };
    Ok((opts, label))
}

/// What the window needs to start: the mail scheme's origin and the
/// providers with what each can do. Also asks the worker for the inbox
/// list and every inbox's mail, since the window listens for events only
/// once it has loaded. Fails with the reason if the config or state
/// couldn't be loaded.
#[tauri::command]
pub fn init(shared: State<'_, Arc<Shared>>) -> Result<Value, String> {
    #[cfg(debug_assertions)]
    let probe = shared.probe.as_ref().map(|p| p.on_init(&shared));
    #[cfg(not(debug_assertions))]
    let probe: Option<Value> = None;

    let providers: Vec<Value> = provider::all()
        .filter_map(|id| provider::get(id).ok())
        .map(|p| json!({ "id": p.id(), "name": p.name(), "caps": p.caps() }))
        .collect();
    send(&shared, Req::Refresh)?;
    let reveal = {
        let mut pending = shared.pending.lock().expect("pending lock");
        pending.page_ready = true;
        pending.target.take()
    }
    .map(|t| json!({ "address": t.address, "id": t.id }));
    Ok(json!({
        "origin": scheme::origin(),
        "providers": providers,
        "probe": probe,
        "drag": drag::SUPPORTED,
        "reveal": reveal,
    }))
}

/// The domains a provider offers, for the new inbox dialog.
#[tauri::command]
pub async fn domains(provider: String) -> Result<Vec<String>, String> {
    let p = provider_of(&provider)?;
    let name = p.name();
    drop(p);
    let list = tauri::async_runtime::spawn_blocking(move || provider::get(&provider)?.domains())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            clean_line(&format!(
                "Couldn't load {name}'s domains: {e}. Try again, or leave the domain on automatic."
            ))
        })?;
    Ok(shown_domains(list))
}

/// The provider's domain list as the picker shows it: only names
/// `create_inbox` would take (so nothing invisible or reordering), lower
/// case, each once.
fn shown_domains(list: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for d in list {
        let d = d.trim().to_ascii_lowercase();
        if is_domain(&d) {
            tmpmail::extract::push_new(&mut out, d);
        }
    }
    out
}

fn is_domain(d: &str) -> bool {
    d.len() <= 253
        && d.contains('.')
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

#[tauri::command]
pub fn create_inbox(
    shared: State<'_, Arc<Shared>>,
    provider: Option<String>,
    user: Option<String>,
    domain: Option<String>,
    label: Option<String>,
) -> Result<(), String> {
    let (opts, label) = create_opts(user, domain, label)?;
    let provider = optional(provider);
    if let Some(id) = &provider {
        let p = provider_of(id)?;
        let caps = p.caps();
        if opts.user.is_some() && !caps.custom_user {
            return Err(format!("{} picks the username itself", p.name()));
        }
        if opts.domain.is_some() && !caps.choose_domain {
            return Err(format!("{} has only one domain", p.name()));
        }
    }
    send(
        &shared,
        Req::Create {
            provider,
            opts,
            label,
            from_tray: false,
        },
    )
}

/// Lists an inbox now instead of on its next tick (selecting it, or the
/// refresh button). Its push stream stays up.
#[tauri::command]
pub fn select_inbox(shared: State<'_, Arc<Shared>>, address: String) -> Result<(), String> {
    stored(&shared, &address)?;
    send(&shared, Req::Select { address })
}

/// Makes an inbox the active one: the CLI's default.
#[tauri::command]
pub fn use_inbox(shared: State<'_, Arc<Shared>>, address: String) -> Result<(), String> {
    stored(&shared, &address)?;
    send(&shared, Req::Use { address })
}

/// Deletes the inbox at its provider where that's possible, and forgets it
/// locally either way (the TUI's `D`, the CLI's `drop`).
#[tauri::command]
pub fn delete_inbox(shared: State<'_, Arc<Shared>>, address: String) -> Result<(), String> {
    stored(&shared, &address)?;
    send(&shared, Req::DeleteInbox { address })
}

#[tauri::command]
pub fn open_message(
    shared: State<'_, Arc<Shared>>,
    address: String,
    id: String,
) -> Result<(), String> {
    stored(&shared, &address)?;
    check_id(&id)?;
    send(&shared, Req::Open { address, id })
}

#[tauri::command]
pub fn delete_message(
    shared: State<'_, Arc<Shared>>,
    address: String,
    id: String,
) -> Result<(), String> {
    let provider = stored(&shared, &address)?;
    check_id(&id)?;
    let p = provider_of(&provider)?;
    if !p.caps().delete_message {
        return Err(format!("{} can't delete messages", p.name()));
    }
    send(&shared, Req::DeleteMessage { address, id })
}

fn copy(app: &AppHandle, text: &str) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| format!("Couldn't copy to the clipboard: {e}"))
}

#[tauri::command]
pub async fn copy_address(
    app: AppHandle,
    shared: State<'_, Arc<Shared>>,
    address: String,
) -> Result<(), String> {
    stored(&shared, &address)?;
    copy(&app, &address)
}

/// The bare address in `Name <addr>`, else the whole field.
pub fn sender_address(from: &str) -> String {
    let inner = from
        .rsplit_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map(|(addr, _)| addr.trim());
    clean_line(inner.filter(|a| a.contains('@')).unwrap_or(from))
}

/// Copies part of an opened message: its code, its sender's address, or
/// one of its links.
#[tauri::command]
pub async fn copy_from_message(
    app: AppHandle,
    shared: State<'_, Arc<Shared>>,
    key: u64,
    part: String,
    index: Option<usize>,
) -> Result<(), String> {
    let text = {
        let mails = shared.mails.lock().expect("mails lock");
        let mail = mails.get(key).ok_or("that message is no longer open")?;
        match (part.as_str(), index) {
            ("code", _) => mail.code.clone().ok_or("this message has no code")?,
            ("sender", _) => sender_address(&mail.msg.from),
            ("link", Some(i)) => mail.links.get(i).cloned().ok_or("no such link")?,
            _ => return Err("nothing to copy".into()),
        }
    };
    copy(&app, &text)
}

/// Opens one of an opened message's links in the system browser.
#[tauri::command]
pub async fn open_link(
    app: AppHandle,
    shared: State<'_, Arc<Shared>>,
    key: u64,
    index: usize,
) -> Result<(), String> {
    let link = {
        let mails = shared.mails.lock().expect("mails lock");
        let mail = mails.get(key).ok_or("that message is no longer open")?;
        mail.links.get(index).cloned().ok_or("no such link")?
    };
    let url = web_url(&link).ok_or("only http and https links open")?;
    crate::open_in_browser(&app, url.as_str())
        .map_err(|e| format!("Couldn't open the link: {e}. Copy it and paste it in a browser."))
}

/// Saves an attachment where the user picks, under a sanitized name, and
/// never over an existing file. Returns the saved file's name, or nothing
/// if the dialog was cancelled.
#[tauri::command]
pub async fn save_attachment(
    app: AppHandle,
    shared: State<'_, Arc<Shared>>,
    key: u64,
    index: usize,
) -> Result<Option<String>, String> {
    let filename = attachment_name(&shared, key, index)?;
    let worker = shared
        .worker
        .get()
        .cloned()
        .ok_or("the worker is not running")?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app
            .dialog()
            .file()
            .set_title("Save attachment")
            .set_file_name(&filename)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = picked.into_path().map_err(|e| e.to_string())?;
        let (reply, answer) = channel();
        worker
            .send(Req::Attachment { key, index, reply })
            .map_err(|_| "the worker stopped".to_string())?;
        let bytes = answer
            .recv()
            .map_err(|_| "the worker stopped".to_string())??;
        let shown = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(filename);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => format!(
                    "{shown} already exists. tmpmail never replaces files, even when the \
                     dialog asks: save under another name."
                ),
                _ => format!("Couldn't save {shown}: {e}"),
            })?;
        // Marked as from the internet, as a browser would (Windows, macOS).
        tmpmail::store::mark_from_internet(&file, &path);
        if let Err(e) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            // Not a half-written file left behind under the chosen name.
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(format!("Couldn't save {shown}: {e}"));
        }
        Ok(Some(shown))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The settings screen's fields, as they are in the files.
#[tauri::command]
pub async fn settings(shared: State<'_, Arc<Shared>>) -> Result<settings::View, String> {
    let host = tauri::async_runtime::spawn_blocking(tray::host_available)
        .await
        .unwrap_or(false);
    shared
        .tray_host
        .store(host, std::sync::atomic::Ordering::Relaxed);
    let gui = shared.gui.lock().expect("gui lock").clone();
    settings::view(gui, host)
}

/// Saves the settings (checked in `settings::Input::check`), then applies
/// them: the tray at once, the rest in the worker without a restart. Runs on
/// the main thread, where the tray is made.
#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    shared: State<'_, Arc<Shared>>,
    input: settings::Input,
) -> Result<(), String> {
    let gui = settings::save(&input)?;
    *shared.gui.lock().expect("gui lock") = gui.clone();
    // The worker reads the file again even when the form changed nothing in
    // it: it may have been edited by hand since (a new unknown key gets its
    // notice). Polling (and the push streams) restart only if the poll
    // interval, push or the timeout changed. Without a worker (it couldn't start) there
    // is nothing to reload.
    if shared.start_error.get().is_none() {
        send(&shared, Req::Reload)?;
    }
    tray::set(&app, &shared, gui.tray)
        .map_err(|e| format!("The settings were saved, but the tray icon failed: {e}"))
}

/// Shows the config file in the file manager.
#[tauri::command]
pub async fn reveal_config(app: AppHandle) -> Result<(), String> {
    let path =
        tmpmail::config::Config::path().ok_or("there is no config directory on this system")?;
    if !path.exists() {
        return Err(
            "There's no config file yet. tmpmail creates it when you save a provider, interval, \
             push or timeout setting."
                .into(),
        );
    }
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| format!("Couldn't show {}: {e}", path.display()))
}

/// The opened message's attachment `index`, by its sanitized name.
fn attachment_name(shared: &Shared, key: u64, index: usize) -> Result<String, String> {
    let mails = shared.mails.lock().expect("mails lock");
    let mail = mails.get(key).ok_or("that message is no longer open")?;
    let att = mail
        .msg
        .attachments
        .get(index)
        .ok_or("no such attachment")?;
    Ok(safe_filename(&att.filename, "attachment"))
}

/// Downloads an attachment into the private drag directory, so a drag can
/// start at once. Returns once it is ready.
#[tauri::command]
pub async fn prepare_attachment(
    shared: State<'_, Arc<Shared>>,
    key: u64,
    index: usize,
) -> Result<(), String> {
    if !drag::SUPPORTED {
        return Err("Dragging files out isn't available on this system. Use Save instead.".into());
    }
    let name = attachment_name(&shared, key, index)?;
    if shared.drag.path(key, index).is_some() {
        return Ok(());
    }
    let worker = shared
        .worker
        .get()
        .cloned()
        .ok_or("the worker is not running")?;
    let shared = Arc::clone(&shared);
    tauri::async_runtime::spawn_blocking(move || {
        let (reply, answer) = channel();
        worker
            .send(Req::Attachment { key, index, reply })
            .map_err(|_| "the worker stopped".to_string())?;
        let bytes = answer
            .recv()
            .map_err(|_| "the worker stopped".to_string())??;
        shared.drag.write(key, index, &name, &bytes).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Starts dragging a prepared attachment out of the window. Runs on the main
/// thread, as the drag must.
#[tauri::command]
pub fn drag_attachment(
    window: WebviewWindow,
    shared: State<'_, Arc<Shared>>,
    key: u64,
    index: usize,
) -> Result<(), String> {
    attachment_name(&shared, key, index)?;
    let path = shared
        .drag
        .path(key, index)
        .ok_or("Still downloading it. Try again in a moment.")?;
    drag::start(&window, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_options_are_checked() {
        let ok = |u: Option<&str>, d: Option<&str>, l: Option<&str>| {
            create_opts(u.map(Into::into), d.map(Into::into), l.map(Into::into))
        };
        let (opts, label) = ok(Some(" Otter.1 "), Some("X.com"), Some("  github ")).unwrap();
        assert_eq!(opts.user.as_deref(), Some("otter.1"));
        assert_eq!(opts.domain.as_deref(), Some("x.com"));
        assert_eq!(label.as_deref(), Some("github"));
        let (opts, label) = ok(Some(""), Some(" "), Some("")).unwrap();
        assert!(opts.user.is_none() && opts.domain.is_none() && label.is_none());
        assert!(ok(Some("a b"), None, None).is_err());
        assert!(ok(Some("a@b"), None, None).is_err());
        assert!(ok(None, Some("nodot"), None).is_err());
        assert!(ok(None, Some("x.com/evil"), None).is_err());
        assert!(ok(None, None, Some(&"x".repeat(65))).is_err());
        let (_, label) = ok(None, None, Some("a\u{1b}[2Jb\nc")).unwrap();
        assert_eq!(label.as_deref(), Some("ab c"));
    }

    #[test]
    fn provider_domains_are_shown_only_as_domain_names() {
        let list = vec![
            "fldubai.com".into(),
            " X.ORG ".into(),
            "x.org".into(),
            "evil\u{202e}moc.com".into(),
            "a\u{1b}[31m.com".into(),
            "nodot".into(),
        ];
        assert_eq!(shown_domains(list), ["fldubai.com", "x.org"]);
    }

    #[test]
    fn only_web_links_open() {
        assert!(web_url("https://example.com/verify").is_some());
        assert!(web_url("http://example.com/").is_some());
        assert!(web_url("javascript:alert(1)").is_none());
        assert!(web_url("file:///etc/passwd").is_none());
        assert!(web_url("mailto:a@b.c").is_none());
        assert!(web_url("http/../../etc/hostname").is_none());
        assert!(web_url("http\\..\\calc.exe").is_none());
        assert!(web_url("https://x.example/a b").is_none());
    }

    #[test]
    fn ids_and_senders() {
        assert!(check_id("6aa6c0f1e2").is_ok());
        assert!(check_id("").is_err());
        assert!(check_id("a\nb").is_err());
        assert!(check_id(&"x".repeat(513)).is_err());
        assert_eq!(
            sender_address("GitHub <noreply@github.com>"),
            "noreply@github.com"
        );
        assert_eq!(sender_address("noreply@github.com"), "noreply@github.com");
        assert_eq!(sender_address("Odd <name>"), "Odd <name>");
    }
}
