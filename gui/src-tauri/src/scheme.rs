//! The `tmpmail-mail` URI scheme: serves opened messages, sanitized by
//! ammonia, to the preview's sandboxed iframe. Each response carries its own
//! Content-Security-Policy, so the mail frame's rules don't depend on the
//! app's, and "load images" is a different response, not a looser shell.
//!
//! Paths: `/mail/<key>` is a message body, `/mail/<key>/source` its source
//! as escaped text, `/mail/<key>/cid/<id>` one of its inline images. The
//! query carries the view: `theme=dark|light`, `images=1` (remote https
//! images), `original=1` (the sender's colors in dark mode). The theme
//! reaches the frame only this way, never through a script. Nothing here
//! does network work: inline images and sources that aren't cached are
//! fetched by the worker.

use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, UriSchemeResponder};
use tmpmail::provider::Message;
use tmpmail::render::{Frame, escape_html, has_own_colors, sanitize_mail_html};

use crate::Shared;
use crate::worker::Req;

pub const SCHEME: &str = "tmpmail-mail";

/// The scheme's origin as the webview sees it: Windows (WebView2) maps
/// custom schemes to `http://<scheme>.localhost`.
pub fn origin() -> &'static str {
    if cfg!(windows) {
        "http://tmpmail-mail.localhost"
    } else {
        "tmpmail-mail://localhost"
    }
}

/// The policy of a mail document: no scripts, fonts, frames, forms or
/// connections; images only inline, from this scheme, and (once the user
/// asks) from https.
pub fn mail_csp(images: bool) -> String {
    let own = if cfg!(windows) {
        "http://tmpmail-mail.localhost"
    } else {
        "tmpmail-mail:"
    };
    let remote = if images { " https:" } else { "" };
    format!(
        "default-src 'none'; img-src data: {own}{remote}; style-src 'unsafe-inline'; \
         base-uri 'none'; form-action 'none'"
    )
}

/// The app theme a frame is drawn for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Theme {
    Light,
    Dark,
}

/// How a mail document is colored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// The sender's colors on a white page: light mode, and "Original
    /// colors" in dark mode.
    Light,
    /// The dark theme's colors, for plain text and HTML without colors.
    Dark,
    /// HTML with its own colors in dark mode: the document is inverted and
    /// its pictures inverted back, as mail clients do.
    Inverted,
}

/// The view a request asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub theme: Theme,
    pub images: bool,
    pub original: bool,
}

impl View {
    pub fn parse(query: &str) -> View {
        let has = |kv: &str| query.split('&').any(|q| q == kv);
        View {
            theme: if has("theme=dark") {
                Theme::Dark
            } else {
                Theme::Light
            },
            images: has("images=1"),
            original: has("original=1"),
        }
    }
}

const BASE_CSS: &str = "body{margin:0;padding:20px 24px;\
font:14px/1.6 system-ui,-apple-system,'Segoe UI',sans-serif;overflow-wrap:anywhere}\
img{max-width:100%;height:auto}\
.plain{white-space:pre-wrap}pre{white-space:pre-wrap;margin:0;font:13px/1.55 ui-monospace,'Cascadia Mono',Menlo,monospace}";

/// Light: white paper. Dark: the app's dark surface (`--raised`) and text.
/// Inverted: `#dddddd` inverts to that same `#222222`, so the page edge
/// matches the app; pictures (and anything with a background image) are
/// inverted back so photos and logos look right.
fn look_css(look: Look) -> &'static str {
    match look {
        Look::Light => "html{background:#fff;color:#1a1a19;color-scheme:light}a{color:#1f4fd1}",
        Look::Dark => {
            "html{background:#222;color:#e9e8e3;color-scheme:dark}a{color:#ffd23f}\
             hr{border-color:#3a3a3a}"
        }
        Look::Inverted => {
            "html{background:#222;color-scheme:light}\
             body{background:#ddd;color:#1a1a19;min-height:100vh;box-sizing:border-box;\
             filter:invert(1) hue-rotate(180deg)}a{color:#1f4fd1}\
             img,picture,video,svg,[style*=\"url(\"]{filter:invert(1) hue-rotate(180deg)}\
             [style*=\"url(\"] :is(img,picture,video,svg,[style*=\"url(\"]){filter:none}"
        }
    }
}

/// How a mail is colored in `view`. `html` is the sanitized body, or
/// `None` for a plain-text mail.
pub fn look(view: View, html: Option<&str>) -> Look {
    match (view.theme, view.original) {
        (Theme::Light, _) | (Theme::Dark, true) => Look::Light,
        (Theme::Dark, false) if html.is_some_and(has_own_colors) => Look::Inverted,
        (Theme::Dark, false) => Look::Dark,
    }
}

pub fn handle(
    _app: &AppHandle,
    shared: &Shared,
    req: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let uri = req.uri();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or_default().to_string();
    #[cfg(debug_assertions)]
    if let Some(probe) = &shared.probe {
        probe.record_request(&path, &query);
    }
    if std::env::var_os("TMPMAIL_GUI_LOG").is_some() {
        eprintln!("{SCHEME}: {} {uri}", req.method());
    }
    if req.method() != tauri::http::Method::GET {
        return responder.respond(status(StatusCode::METHOD_NOT_ALLOWED));
    }
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segments.as_slice() {
        ["mail", key] => {
            let view = View::parse(&query);
            let found = key.parse().ok().and_then(|key| {
                let mails = shared.mails.lock().expect("mails lock");
                mails.get(key).map(|m| (key, m.msg.clone()))
            });
            let Some((key, msg)) = found else {
                return responder.respond(not_found());
            };
            // Sanitizing a big mail takes a while: not on the webview's
            // thread.
            tauri::async_runtime::spawn_blocking(move || {
                let doc = mail_document(&msg, key, view);
                responder.respond(document(&doc, &mail_csp(view.images)));
            });
        }
        ["mail", key, "source"] => {
            let theme = View::parse(&query).theme;
            let Ok(key) = key.parse::<u64>() else {
                return responder.respond(not_found());
            };
            let cached = {
                let mails = shared.mails.lock().expect("mails lock");
                match mails.get(key) {
                    None => return responder.respond(not_found()),
                    Some(m) => m.source.clone(),
                }
            };
            match (cached, shared.worker.get()) {
                (Some(text), _) => {
                    // A raw source can be megabytes: not on the webview's thread.
                    tauri::async_runtime::spawn_blocking(move || {
                        responder.respond(source_response(&text, theme));
                    });
                }
                (None, Some(tx)) => {
                    // The worker answers (raw sources come from the provider).
                    let _ = tx.send(Req::Source {
                        key,
                        theme,
                        responder,
                    });
                }
                (None, None) => responder.respond(not_found()),
            }
        }
        ["mail", key, "cid", cid] => {
            let (Ok(key), Some(cid)) = (key.parse::<u64>(), percent_decode(cid)) else {
                return responder.respond(not_found());
            };
            // The mail's own `cid:` URL is percent-encoded too (RFC 2392):
            // `a%40b` means the part `<a@b>`.
            let cid = tmpmail::provider::cid_of_url(&cid);
            let cached = {
                let mails = shared.mails.lock().expect("mails lock");
                match mails.get(key) {
                    None => return responder.respond(not_found()),
                    Some(m) => m.inline.get(&cid).cloned(),
                }
            };
            match (cached, shared.worker.get()) {
                (Some((mime, bytes)), _) => responder.respond(image(&mime, bytes)),
                (None, Some(tx)) => {
                    // The worker answers (it may need the provider).
                    let _ = tx.send(Req::Inline {
                        key,
                        cid,
                        responder,
                    });
                }
                (None, None) => responder.respond(not_found()),
            }
        }
        #[cfg(debug_assertions)]
        ["probe", rest @ ..] if shared.probe.is_some() => {
            let probe = shared.probe.as_ref().expect("checked");
            responder.respond(probe.serve(rest));
        }
        _ => responder.respond(not_found()),
    }
}

/// The sanitized document for `msg`, colored for `view`.
pub fn mail_document(msg: &Message, key: u64, view: View) -> String {
    let cid_base = format!("{}/mail/{key}/cid/", origin());
    let frame = Frame {
        cid_base: &cid_base,
    };
    match (&msg.html, &msg.text) {
        (Some(h), _) => {
            let body = sanitize_mail_html(h, view.images, Some(&frame));
            // Colors are judged with images allowed, so loading images
            // never switches a mail between the dark and inverted looks.
            let look = if view.images || view.theme == Theme::Light || view.original {
                look(view, Some(&body))
            } else {
                look(view, Some(&sanitize_mail_html(h, true, Some(&frame))))
            };
            wrap(&body, look)
        }
        (None, Some(t)) => {
            let body = format!("<div class=\"plain\">{}</div>", escape_html(t));
            wrap(&body, look(view, None))
        }
        (None, None) => wrap("", look(view, None)),
    }
}

/// A full document around a sanitized body.
pub fn wrap(body: &str, look: Look) -> String {
    let css = look_css(look);
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\
         <meta name=\"referrer\" content=\"no-referrer\"><style>{BASE_CSS}{css}</style>\
         </head><body>\n{body}\n</body></html>\n"
    )
}

/// A message source as escaped text, under the strictest policy: no
/// images at all.
pub fn source_response(text: &str, theme: Theme) -> Response<Vec<u8>> {
    let look = match theme {
        Theme::Light => Look::Light,
        Theme::Dark => Look::Dark,
    };
    let doc = wrap(&format!("<pre>{}</pre>", escape_html(text)), look);
    html_response(
        &doc,
        Some("default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"),
    )
}

/// An HTML response with the given policy (`None`: no CSP header at all,
/// only for the probe's controls).
pub fn html_response(doc: &str, csp: Option<&str>) -> Response<Vec<u8>> {
    let mut b = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(csp) = csp {
        b = b.header(header::CONTENT_SECURITY_POLICY, csp);
    }
    b.body(doc.as_bytes().to_vec()).expect("valid response")
}

fn document(doc: &str, csp: &str) -> Response<Vec<u8>> {
    html_response(doc, Some(csp))
}

/// Inline images are served only as images, never as something a frame
/// could run.
pub fn image(content_type: &str, bytes: Vec<u8>) -> Response<Vec<u8>> {
    let ct = if content_type.starts_with("image/") && !content_type.contains("svg") {
        content_type
    } else {
        "application/octet-stream"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CONTENT_SECURITY_POLICY, "default-src 'none'")
        .header(header::CACHE_CONTROL, "no-store")
        .body(bytes)
        .expect("valid response")
}

pub fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Vec::new())
        .expect("valid response")
}

pub fn not_found() -> Response<Vec<u8>> {
    status(StatusCode::NOT_FOUND)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmpmail::provider::Attachment;

    #[test]
    fn cid_ids_round_trip() {
        assert_eq!(percent_decode("logo%40x%20y").as_deref(), Some("logo@x y"));
        assert_eq!(percent_decode("bad%4"), None);
    }

    fn attachment(filename: &str, content_type: &str) -> Attachment {
        Attachment {
            id: format!("id-{filename}"),
            filename: filename.into(),
            content_type: content_type.into(),
            size: None,
            url: None,
        }
    }

    #[test]
    fn inline_images_are_raster_images_only() {
        let msg = Message {
            attachments: vec![
                attachment("logo.png", "image/png; name=logo.png"),
                attachment("photo.JPG", "application/octet-stream"),
                attachment("x.svg", "image/svg+xml"),
                attachment("y.svg", "application/octet-stream"),
                attachment("page.html", "text/html"),
            ],
            ..Default::default()
        };
        let find = |cid: &str| {
            tmpmail::provider::inline_attachment(&msg, cid).map(|(a, t)| (a.filename.clone(), t))
        };
        assert_eq!(
            find("logo.png@x"),
            Some(("logo.png".into(), "image/png".into()))
        );
        assert_eq!(
            find("id-photo.JPG"),
            Some(("photo.JPG".into(), "image/jpeg".into()))
        );
        assert_eq!(find("x.svg"), None);
        assert_eq!(find("y.svg"), None);
        assert_eq!(find("page.html"), None);
        assert_eq!(find("nothing"), None);
    }

    #[test]
    fn the_view_comes_from_the_query_only() {
        let v = View::parse("theme=dark&images=1");
        assert_eq!(v.theme, Theme::Dark);
        assert!(v.images && !v.original);
        assert_eq!(View::parse("").theme, Theme::Light);
        assert_eq!(View::parse("theme=darkish").theme, Theme::Light);
    }

    #[test]
    fn dark_mail_is_dark() {
        let colored = Message {
            html: Some(r##"<table bgcolor="#ffffff"><tr><td>Hi</td></tr></table>"##.into()),
            ..Default::default()
        };
        let bare = Message {
            html: Some("<p>Hi <b>there</b></p>".into()),
            ..Default::default()
        };
        let text = Message {
            text: Some("Code: 123456 <b>".into()),
            ..Default::default()
        };
        let dark = View::parse("theme=dark");
        let original = View::parse("theme=dark&original=1");
        let light = View::parse("theme=light");
        assert!(mail_document(&colored, 1, dark).contains("invert(1)"));
        assert!(!mail_document(&colored, 1, original).contains("invert(1)"));
        assert!(mail_document(&colored, 1, light).contains("background:#fff"));
        let doc = mail_document(&bare, 1, dark);
        assert!(doc.contains("background:#222") && !doc.contains("invert(1)"));
        let doc = mail_document(&text, 1, dark);
        assert!(doc.contains("color-scheme:dark"), "{doc}");
        assert!(doc.contains("&lt;b&gt;"), "{doc}");
    }

    #[test]
    fn loading_images_keeps_the_look() {
        let hero = Message {
            html: Some(r#"<div style="background:url(https://x.example/h.png)">Hi</div>"#.into()),
            ..Default::default()
        };
        let off = mail_document(&hero, 1, View::parse("theme=dark"));
        let on = mail_document(&hero, 1, View::parse("theme=dark&images=1"));
        assert!(off.contains("invert(1)") && on.contains("invert(1)"));
        assert!(!off.contains("x.example"));
    }

    #[test]
    fn images_only_widen_img_src() {
        let off = mail_csp(false);
        let on = mail_csp(true);
        assert!(off.starts_with("default-src 'none';"));
        assert!(!off.contains("https:"));
        assert_eq!(on.replace(" https:", ""), off);
    }
}
