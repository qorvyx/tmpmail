//! Debug builds only: a security probe for the mail preview. With
//! `TMPMAIL_GUI_PROBE=1` the window renders a hostile test mail through the
//! real preview path (and through control frames that each drop one layer),
//! every request to the mail scheme is recorded, and once the frames have
//! loaded the result is printed to stdout as JSON (or written to the file
//! `TMPMAIL_GUI_PROBE_REPORT` names) and the app exits: 0 when
//! every check held, 1 otherwise. `TMPMAIL_GUI_PROBE_WAIT=SECS` keeps the
//! window open that long first, so a person can click the test links.
//!
//! What the script in the mail would do if it ran is observable from Rust:
//! it requests `/probe/hit/<variant>-<signal>` URLs on the mail scheme.
//!
//! The script also attacks the caller check (`caller.rs`) the way a frame
//! that knows Tauri's invoke key could: through the bridge if it has one, a
//! direct `fetch` to the IPC protocol, and a forged `postMessage` call with
//! a fake `Origin` header. The probe hands the frames the key (debug builds
//! only), so this holds on engines that keep the bridge out of frames too.
//! Every call the invoke handler sees from a frame is recorded, and the
//! forged ones carry a `Tmpmail-Probe` header naming the frame.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tauri::http::Response;
use tauri::ipc::InvokeMessage;
use tauri::{AppHandle, Manager, Runtime, State, Url};
use tmpmail::provider::{Attachment, Message};
use tmpmail::render::{Frame, sanitize_mail_html};

use crate::scheme::{self, html_response};
use crate::{Mails, Shared, caller};

/// The answer of every image endpoint: a 1x1 PNG.
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0,
    0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 208, 143, 190, 12, 0, 2,
    26, 1, 94, 54, 195, 76, 179, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

const CID: &str = "probe-logo@tmpmail";

/// The test mail through the real preview: plain, with images loaded, and
/// in dark mode (where the test mail's own colors make it inverted).
const REAL: [&str; 3] = ["real", "real_images", "real_dark"];

/// Frames that serve the raw (unsanitized) mail, each without one of the
/// layers, as (name, CSP header, iframe sandbox attribute).
const CONTROLS: &[(&str, bool, Option<&str>)] = &[
    // The CSP header alone: no sandbox.
    ("raw_csp", true, None),
    // The sandbox alone: no CSP header.
    ("raw_sandbox", false, Some("allow-popups")),
    // Scripts allowed in an opaque origin: shows the method works, and
    // whether Tauri's IPC reaches such a frame.
    ("raw_scripts", false, Some("allow-scripts allow-popups")),
    // No layer at all (cross-origin frame on the mail origin, scripts on):
    // only the caller check stands between it and the commands.
    ("raw_open", false, None),
];

/// A command call the invoke handler saw that didn't come from the app's
/// page, or that carried the probe's forgery marker.
#[derive(Clone, Debug)]
struct Call {
    command: String,
    origin: Option<String>,
    top: Option<String>,
    /// The `Tmpmail-Probe` header: `<variant>-<route>` of a forged call.
    marker: Option<String>,
    accepted: bool,
}

pub struct Probe {
    requests: Mutex<Vec<String>>,
    popups: Mutex<Vec<String>>,
    remote: Mutex<Vec<String>>,
    remote_base: Mutex<String>,
    init_calls: AtomicUsize,
    keys: Mutex<Vec<(String, u64)>>,
    /// Tauri's invoke key, handed to the test mail (see the module docs).
    invoke_key: Mutex<String>,
    calls: Mutex<Vec<Call>>,
    /// Whether the report is out (or being written): the first of the
    /// window and the watchdog wins.
    reported: Mutex<bool>,
    wait: u64,
}

impl Probe {
    pub fn from_env() -> Option<Probe> {
        let on = std::env::var("TMPMAIL_GUI_PROBE").is_ok_and(|v| !v.is_empty() && v != "0");
        on.then(|| Probe {
            requests: Mutex::default(),
            popups: Mutex::default(),
            remote: Mutex::default(),
            remote_base: Mutex::default(),
            init_calls: AtomicUsize::new(0),
            keys: Mutex::default(),
            invoke_key: Mutex::default(),
            calls: Mutex::default(),
            reported: Mutex::new(false),
            wait: std::env::var("TMPMAIL_GUI_PROBE_WAIT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        })
    }

    /// Starts the stand-in "remote" image server, stores the test mails for
    /// the real preview path, and arms a watchdog.
    pub fn start(&self, app: AppHandle, shared: &Arc<Shared>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the probe's image server");
        let port = listener.local_addr().expect("local addr").port();
        *self.remote_base.lock().unwrap() = format!("http://127.0.0.1:{port}");
        *self.invoke_key.lock().unwrap() = app.invoke_key().to_string();
        let for_server = shared.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve_remote(&for_server, stream);
            }
        });

        for name in REAL {
            let mut mail = Mails::entry("probe@tmpmail.invalid", self.message(name));
            mail.inline
                .insert(CID.to_string(), ("image/png".into(), PNG.to_vec()));
            mail.cids.insert(CID.to_string(), Some(CID.to_string()));
            let key = shared.mails.lock().unwrap().insert(mail);
            self.keys.lock().unwrap().push((name.to_string(), key));
        }

        let shared = shared.clone();
        let limit = 60 + self.wait;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(limit));
            let probe = shared.probe.as_ref().expect("probe");
            let mut result = probe.result(&app);
            result["timeout"] = json!(true);
            result["pass"] = json!(false);
            if probe.report(&result) {
                std::process::exit(1);
            }
        });
    }

    fn message(&self, variant: &str) -> Message {
        Message {
            id: format!("probe-{variant}"),
            from: "Probe <probe@tmpmail.invalid>".into(),
            to: vec!["you@tmpmail.invalid".into()],
            subject: format!("Hostile test mail ({variant})"),
            date: None,
            text: None,
            html: Some(self.hostile_html(variant)),
            attachments: vec![Attachment {
                id: "probe-logo".into(),
                filename: "logo.png".into(),
                content_type: "image/png".into(),
                size: Some(PNG.len() as u64),
                url: None,
            }],
        }
    }

    /// The test mail. Every way it could run code or fetch something
    /// requests a URL we can see.
    fn hostile_html(&self, v: &str) -> String {
        let hit = format!("{}/probe/hit/{v}-", scheme::origin());
        let remote = format!("{}/{v}-remote.png", self.remote_base.lock().unwrap());
        // As JS string literals.
        let key = json!(*self.invoke_key.lock().unwrap()).to_string();
        let ipc = ipc_base();
        let refused = json!(caller::REFUSED).to_string();
        format!(
            r#"<style>.probe-block{{background-image:url({hit}style-block)}}</style>
<h1>tmpmail preview probe</h1>
<p style="color:#0a7a3a;background-image:url({hit}style-attr)">This line is green if style attributes apply.</p>
<p class="probe-block">This line requests a URL if the style block applies.</p>
<p>Inline image: <img src="cid:{CID}" alt="cid" width="16" height="16"></p>
<p>Remote image: <img src="{remote}" alt="remote" width="16" height="16"></p>
<p>Broken image with onerror: <img src="missing.png" onerror="new Image().src='{hit}onerror'"></p>
<p><a id="blank" href="https://example.com/?tmpmail-probe={v}-link" target="_blank">A target=_blank link</a>
 and <a href="javascript:new Image().src='{hit}javascript'">a javascript: link</a>.</p>
<script>
(function () {{
  function h(n) {{ new Image().src = '{hit}' + n; }}
  h('script-ran');
  var t;
  try {{ t = window.__TAURI_INTERNALS__; h(t ? 'internals-present' : 'internals-absent'); }} catch (e) {{ h('internals-threw'); }}
  try {{ var p = window.parent.__TAURI_INTERNALS__; h(p ? 'parent-internals-readable' : 'parent-internals-undefined'); t = t || p; }} catch (e) {{ h('parent-blocked'); }}
  function why(e) {{ return String(e).indexOf({refused}) >= 0 ? 'refused' : 'rejected'; }}
  try {{
    if (t && t.invoke) {{ t.invoke('init').then(function () {{ h('invoke-ok'); }}, function (e) {{ h('invoke-' + why(e)); }}); }}
    else {{ h('invoke-unavailable'); }}
  }} catch (e) {{ h('invoke-threw'); }}
  // A direct call over the IPC protocol with the invoke key. The Origin
  // set here is ignored by the engine (a forbidden header).
  try {{
    fetch('{ipc}init', {{ method: 'POST', body: '{{}}', headers: {{
      'Content-Type': 'application/json', 'Tauri-Callback': '1', 'Tauri-Error': '2',
      'Tauri-Invoke-Key': {key}, 'Origin': 'tauri://localhost', 'Tmpmail-Probe': '{v}-fetch' }} }})
      .then(function (r) {{
        if (r.headers.get('Tauri-Response') === 'ok') {{ h('fetch-ok'); return; }}
        return r.text().then(function (b) {{ h('fetch-' + why(b)); }});
      }}, function () {{ h('fetch-failed'); }});
  }} catch (e) {{ h('fetch-threw'); }}
  // The postMessage route, whose headers are the script's own: a forged
  // Origin. Rust sees whether it arrives (it carries Tmpmail-Probe).
  try {{
    var msg = JSON.stringify({{ cmd: 'init', callback: 1, error: 2, payload: {{}},
      options: {{ headers: {{ 'Origin': 'tauri://localhost', 'Tmpmail-Probe': '{v}-post' }} }},
      __TAURI_INVOKE_KEY__: {key} }});
    if (window.ipc && window.ipc.postMessage) {{ window.ipc.postMessage(msg); h('post-sent-ipc'); }}
    else if (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.ipc) {{ window.webkit.messageHandlers.ipc.postMessage(msg); h('post-sent-webkit'); }}
    else if (window.chrome && window.chrome.webview) {{ window.chrome.webview.postMessage(msg); h('post-sent-webview2'); }}
    else {{ h('post-unavailable'); }}
  }} catch (e) {{ h('post-threw'); }}
  // And through the bridge with fetch broken, which makes it fall back to
  // postMessage with the headers given here.
  try {{
    if (t && t.invoke) {{
      window.fetch = function () {{ return Promise.reject(new Error('blocked')); }};
      t.invoke('init', {{}}, {{ headers: {{ 'Origin': 'tauri://localhost', 'Tmpmail-Probe': '{v}-fallback' }} }})
        .then(function () {{ h('fallback-ok'); }}, function (e) {{ h('fallback-' + why(e)); }});
    }}
  }} catch (e) {{ h('fallback-threw'); }}
  try {{ var w = window.open('https://example.com/?tmpmail-probe={v}-window-open'); h(w ? 'window-open-returned' : 'window-open-null'); }} catch (e) {{ h('window-open-threw'); }}
  try {{ document.getElementById('blank').click(); h('link-click-done'); }} catch (e) {{ h('link-click-threw'); }}
}})();
</script>"#
        )
    }

    /// Records what the invoke handler decided about a call that didn't
    /// come from the app's page, or that the probe forged.
    pub fn observe<R: Runtime>(
        &self,
        message: &InvokeMessage<R>,
        caller: &caller::Caller,
        accepted: bool,
    ) {
        let marker = message
            .headers()
            .get("tmpmail-probe")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if accepted && marker.is_none() {
            return;
        }
        self.calls.lock().unwrap().push(Call {
            command: message.command().to_string(),
            origin: caller.origin.clone(),
            top: caller.top.as_ref().map(Url::to_string),
            marker,
            accepted,
        });
    }

    /// Writes the report, once: to `TMPMAIL_GUI_PROBE_REPORT` when that is
    /// set (GLib prints some of its messages to stdout, so CI reads the
    /// file; written to a temporary file and renamed), else to stdout.
    /// Returns false if it was already out. The lock is held until the file
    /// is in place, so the watchdog's exit can't cut a write short.
    fn report(&self, result: &Value) -> bool {
        let mut reported = self.reported.lock().unwrap();
        if *reported {
            return false;
        }
        *reported = true;
        let text = format!("{}\n", serde_json::to_string_pretty(result).unwrap());
        match std::env::var_os("TMPMAIL_GUI_PROBE_REPORT").filter(|p| !p.is_empty()) {
            Some(path) => {
                let path = std::path::PathBuf::from(path);
                let tmp = path.with_extension("json.tmp");
                if let Err(e) =
                    std::fs::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &path))
                {
                    eprintln!(
                        "tmpmail-gui: could not write the probe report to {}: {e}",
                        path.display()
                    );
                    print!("{text}");
                }
            }
            None => print!("{text}"),
        }
        true
    }

    /// What `init` returns in probe mode: the frames to show. Only calls
    /// the caller check let through get here, so this counts the app
    /// page's calls alone.
    pub fn on_init(&self, _shared: &Shared) -> Value {
        self.init_calls.fetch_add(1, Ordering::SeqCst);
        let keys = self.keys.lock().unwrap().clone();
        let real: Vec<Value> = keys
            .iter()
            .map(|(name, key)| {
                json!({
                    "name": name,
                    "key": key,
                    "images": name == "real_images",
                    "theme": if name == "real_dark" { "dark" } else { "light" },
                })
            })
            .collect();
        let controls: Vec<Value> = CONTROLS
            .iter()
            .map(|(name, _, sandbox)| {
                json!({
                    "name": name,
                    "url": format!("{}/probe/doc/{name}", scheme::origin()),
                    "sandbox": sandbox,
                })
            })
            .collect();
        json!({ "real": real, "controls": controls, "wait_secs": self.wait })
    }

    pub fn record_request(&self, path: &str, query: &str) {
        let entry = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query}")
        };
        self.requests.lock().unwrap().push(entry);
    }

    pub fn record_popup(&self, url: &Url) {
        self.popups.lock().unwrap().push(url.to_string());
    }

    /// `/probe/doc/<control>` and `/probe/hit/<signal>`.
    pub fn serve(&self, rest: &[&str]) -> Response<Vec<u8>> {
        match rest {
            ["doc", name] => match CONTROLS.iter().find(|(n, ..)| n == name) {
                Some((name, csp, _)) => {
                    // The CSP header's own rules, without the sandbox it now
                    // also carries (that is the sandbox control's layer).
                    let csp = csp.then(|| {
                        scheme::mail_csp(false)
                            .replace(&format!("; {}", scheme::DOCUMENT_GUARD), "")
                    });
                    html_response(
                        &scheme::wrap(&self.hostile_html(name), scheme::Look::Light),
                        csp.as_deref(),
                    )
                }
                None => scheme::not_found(),
            },
            ["hit", _] => scheme::image("image/png", PNG.to_vec()),
            _ => scheme::not_found(),
        }
    }

    fn result(&self, app: &AppHandle) -> Value {
        let requests = self.requests.lock().unwrap().clone();
        let remote = self.remote.lock().unwrap().clone();
        let popups = self.popups.lock().unwrap().clone();
        let hits: BTreeSet<String> = requests
            .iter()
            .filter_map(|r| r.strip_prefix("/probe/hit/"))
            .map(str::to_string)
            .collect();
        let has = |s: &str| hits.contains(s);
        let ran = |v: &str| {
            ["script-ran", "onerror", "javascript"]
                .iter()
                .any(|s| has(&format!("{v}-{s}")))
        };
        let signals = |v: &str| -> Vec<String> {
            hits.iter()
                .filter_map(|h| h.strip_prefix(&format!("{v}-")))
                .map(str::to_string)
                .collect()
        };
        let keys = self.keys.lock().unwrap().clone();
        let cid_served = keys.iter().all(|(_, key)| {
            requests
                .iter()
                .any(|r| r.starts_with(&format!("/mail/{key}/cid/")))
        });
        let remote_hit = |v: &str| remote.iter().any(|r| r.contains(&format!("{v}-remote")));
        let calls = self.calls.lock().unwrap().clone();
        let from = |v: &str| {
            let prefix = format!("{v}-");
            calls
                .iter()
                .filter(|c| c.marker.as_deref().is_some_and(|m| m.starts_with(&prefix)))
                .cloned()
                .collect::<Vec<_>>()
        };
        let accepted_signal = |v: &str| {
            ["invoke-ok", "fetch-ok", "fallback-ok"]
                .iter()
                .any(|s| has(&format!("{v}-{s}")))
        };
        // The mail origin, as the engine reports it in `Origin`.
        let mail_origin = scheme::origin();
        let open_calls = from("raw_open");
        // Only raw_open runs scripts on the mail origin. Its direct fetch
        // reaches the handler on every engine; on WebView2 its bridge call
        // does too.
        let ipc_rejected_from_mail_origin = !accepted_signal("raw_open")
            && open_calls.iter().all(|c| !c.accepted)
            && calls
                .iter()
                .any(|c| !c.accepted && c.origin.as_deref() == Some(mail_origin));
        let real_no_ipc = REAL.iter().all(|v| {
            from(v).is_empty()
                && !accepted_signal(v)
                && !has(&format!("{v}-internals-present"))
                && !has(&format!("{v}-parent-internals-readable"))
        });
        let control = |v: &str| {
            let calls = from(v);
            json!({
                "internals": if has(&format!("{v}-internals-present")) { "present" } else if has(&format!("{v}-internals-absent")) { "absent" } else { "unknown" },
                "signals": signals(v),
                "calls_reaching_handler": calls.iter().map(|c| json!({
                    "route": c.marker.as_deref().and_then(|m| m.strip_prefix(&format!("{v}-"))),
                    "command": c.command,
                    "origin": c.origin,
                    "accepted": c.accepted,
                })).collect::<Vec<_>>(),
            })
        };

        // The sanitizer on its own: the frames above can't tell it from the
        // CSP, which blocks the probe's http "remote" image either way.
        let frame = Frame {
            cid_base: "tmpmail-mail://localhost/mail/0/cid/",
        };
        let hostile = self.hostile_html("check");
        let sanitized = sanitize_mail_html(&hostile, false, Some(&frame));
        let sanitized_images = sanitize_mail_html(&hostile, true, Some(&frame));
        let remote_base = self.remote_base.lock().unwrap().clone();
        let https_img = r#"<img src="https://tmpmail.invalid/x.png">"#;
        let https_kept = sanitize_mail_html(https_img, true, Some(&frame))
            .contains("https://tmpmail.invalid/x.png");
        let https_dropped =
            !sanitize_mail_html(https_img, false, Some(&frame)).contains("tmpmail.invalid");
        let windows = app.webview_windows().len();
        // Dark mode adds a document path: the test mail sets its own colors,
        // so it must come out inverted (and still script-free, above).
        let dark =
            scheme::mail_document(&self.message("check"), 0, scheme::View::parse("theme=dark"));
        let dark_inverted = dark.contains("invert(1)") && !dark.contains("<script");
        // With images off, a style keeps its colors but loses its url(...).
        let style_kept = sanitized.contains("color:#0a7a3a") && !sanitized.contains("style-attr");

        let checks = json!({
            "real_frame_ran_no_code": !ran("real") && !ran("real_images"),
            "dark_frame_ran_no_code": !ran("real_dark") && requests.iter().any(|r| r.contains("theme=dark")),
            "dark_document_is_inverted": dark_inverted,
            "sanitizer_keeps_style_colors_without_urls": style_kept,
            "csp_header_alone_blocks_scripts": !ran("raw_csp"),
            "csp_header_alone_blocks_remote_image": !remote_hit("raw_csp"),
            "sandbox_alone_blocks_scripts": !ran("raw_sandbox"),
            "control_scripts_ran": has("raw_scripts-script-ran"),
            "real_frames_no_internals_no_ipc": real_no_ipc,
            "ipc_rejected_from_mail_origin": ipc_rejected_from_mail_origin,
            "forged_calls_refused": calls.iter().all(|c| !c.accepted),
            "init_called_once": self.init_calls.load(Ordering::SeqCst) == 1,
            "remote_image_not_loaded": REAL.iter().all(|v| !remote_hit(v)),
            "sanitizer_drops_remote_images": !sanitized.contains(&remote_base) && https_dropped,
            "sanitizer_drops_http_images_with_images_on": !sanitized_images.contains(&remote_base),
            "sanitizer_keeps_https_images_with_images_on": https_kept,
            "cid_image_served": cid_served,
            "sanitizer_sets_target_blank": sanitized.contains(r#"target="_blank""#),
            "sanitizer_drops_javascript_links": !sanitized.contains("href=\"javascript"),
            "sanitizer_drops_script_and_handlers": !sanitized.contains("<script") && !sanitized.contains("onerror="),
            "no_popup_window_created": windows == 1,
        });
        let pass = checks
            .as_object()
            .expect("object")
            .values()
            .all(|v| v.as_bool() == Some(true));

        let variants: serde_json::Map<String, Value> = REAL
            .iter()
            .copied()
            .chain(CONTROLS.iter().map(|(n, ..)| *n))
            .map(|v| (v.to_string(), json!(signals(v))))
            .collect();
        json!({
            "engine": engine(),
            "os": std::env::consts::OS,
            "pass": pass,
            "checks": checks,
            "observations": {
                "inline_style_block_applies_under_csp_header": has("raw_csp-style-block"),
                "inline_style_attr_applies_in_real_frame": has("real_images-style-attr"),
                "internals_in_unsandboxed_frame": if has("raw_open-internals-present") { "present" } else if has("raw_open-internals-absent") { "absent" } else { "unknown" },
                "controls": {
                    "raw_scripts": control("raw_scripts"),
                    "raw_open": control("raw_open"),
                },
                "refused_calls": calls.iter().filter(|c| !c.accepted).map(|c| json!({
                    "command": c.command,
                    "origin": c.origin,
                    "top": c.top,
                    "marker": c.marker,
                })).collect::<Vec<_>>(),
                "forged_calls_accepted": calls.iter().filter(|c| c.accepted).filter_map(|c| c.marker.clone()).collect::<Vec<_>>(),
                "popups_reaching_on_new_window": popups,
                "webview_windows": windows,
            },
            "signals": variants,
            "requests": requests,
            "remote_requests": remote,
        })
    }
}

/// Where Tauri's IPC protocol is served, as `convertFileSrc(cmd, 'ipc')`
/// builds it.
fn ipc_base() -> &'static str {
    if cfg!(windows) {
        "http://ipc.localhost/"
    } else {
        "ipc://localhost/"
    }
}

fn engine() -> String {
    let name = if cfg!(windows) {
        "WebView2"
    } else if cfg!(target_os = "macos") {
        "WKWebView"
    } else {
        "WebKitGTK"
    };
    match tauri::webview_version() {
        Ok(v) => format!("{name} {v}"),
        Err(_) => name.to_string(),
    }
}

/// The stand-in remote server: records the path and answers with a PNG.
fn serve_remote(shared: &Shared, mut stream: std::net::TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    let path = head.split_whitespace().nth(1).unwrap_or("?").to_string();
    if let Some(p) = &shared.probe {
        p.remote.lock().unwrap().push(path);
    }
    let mut resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        PNG.len()
    )
    .into_bytes();
    resp.extend_from_slice(PNG);
    let _ = stream.write_all(&resp);
}

/// Called by the window once every frame has loaded (and the wait is over):
/// prints the result and exits.
#[tauri::command]
pub fn probe_finish(app: AppHandle, shared: State<'_, Arc<Shared>>) {
    let Some(probe) = &shared.probe else {
        return;
    };
    let result = probe.result(&app);
    if !probe.report(&result) {
        return;
    }
    let code = if result["pass"] == json!(true) { 0 } else { 1 };
    app.exit(code);
}
