//! Guerrilla Mail JSON API: https://www.guerrillamail.com/GuerrillaMailAPI.html
//! Session based (`sid_token`). Addresses keep mail for 60 minutes; any of the
//! Guerrilla domains delivers to the same mailbox.

use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{
    Attachment, Capabilities, CreateOpts, Inbox, Message, Provider, Summary, base_url,
    domain_unavailable, parse_date, str_field,
};
use crate::error::{Result, message_not_found};
use crate::http::Http;

const BASE: &str = "https://api.guerrillamail.com/ajax.php";
/// Attachments are served by the website, not the API; the API's `sid_token`
/// doubles as the site's session cookie.
const WEB: &str = "https://www.guerrillamail.com";

/// Guerrilla has no domain-list endpoint; these are the domains its web UI
/// offers, all of which deliver to the same mailbox.
const DOMAINS: &[&str] = &[
    "sharklasers.com",
    "guerrillamail.info",
    "grr.la",
    "guerrillamail.biz",
    "guerrillamail.com",
    "guerrillamail.de",
    "guerrillamail.net",
    "guerrillamail.org",
    "guerrillamailblock.com",
    "pokemail.net",
    "spam4.me",
];

pub struct Guerrilla {
    base: String,
    web: String,
    http: Http,
}

impl Guerrilla {
    pub fn new() -> Self {
        Guerrilla {
            base: base_url("guerrilla", BASE),
            web: base_url("guerrilla_web", WEB),
            http: Http::new("guerrilla"),
        }
    }

    fn call(&self, f: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut query: Vec<(&str, &str)> =
            vec![("f", f), ("ip", "127.0.0.1"), ("agent", "tmpmail")];
        query.extend_from_slice(params);
        let v: Value = self
            .http
            .json(|| self.http.client().get(&self.base).query(&query))?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            return Err(self.http.bad(err.to_string()));
        }
        Ok(v)
    }

    /// Calls with the inbox's session and re-binds the address if the API
    /// silently started a new session (it does after ~18 minutes idle).
    fn session_call(&self, inbox: &mut Inbox, f: &str, params: &[(&str, &str)]) -> Result<Value> {
        let sid = inbox.cred_str("sid_token").unwrap_or_default().to_string();
        let mut all = vec![("sid_token", sid.as_str())];
        all.extend_from_slice(params);
        let v = self.call(f, &all)?;
        if let Some(new_sid) = v.get("sid_token").and_then(Value::as_str) {
            if new_sid != sid {
                inbox.set_cred("sid_token", new_sid.to_string());
                let user = inbox.local_part().to_string();
                self.call(
                    "set_email_user",
                    &[
                        ("sid_token", new_sid),
                        ("email_user", &user),
                        ("lang", "en"),
                    ],
                )?;
                // The first call ran against an empty session; repeat it.
                let mut again = vec![("sid_token", new_sid)];
                again.extend_from_slice(params);
                return self.call(f, &again);
            }
        }
        Ok(v)
    }

    fn summary(v: &Value) -> Summary {
        Summary {
            id: id_of(&v["mail_id"]),
            from: str_field(v, "mail_from").to_string(),
            subject: decode_entities(str_field(v, "mail_subject")),
            intro: decode_entities(str_field(v, "mail_excerpt")),
            date: parse_date(&v["mail_timestamp"]),
            seen: truthy(&v["mail_read"]),
            has_attachments: truthy(&v["att"]),
        }
    }
}

fn id_of(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Flags arrive as numbers or numeric strings.
fn truthy(v: &Value) -> bool {
    v.as_i64().unwrap_or(0) != 0 || v.as_str().is_some_and(|s| s != "0")
}

/// `att_info` lists each file as `{"f": name, "t": type, "p": part id}`, the
/// shape Guerrilla's own web client reads.
fn attachments(v: &Value) -> Vec<Attachment> {
    v["att_info"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| Attachment {
            id: id_of(&a["p"]),
            filename: str_field(a, "f").to_string(),
            content_type: str_field(a, "t").to_string(),
            size: a["s"].as_u64(),
            url: None,
        })
        .filter(|a| !a.id.is_empty())
        .collect()
}

/// Guerrilla points images (`src`, `background`) at its own proxy, a path
/// on its website (`/res.php?...&q=URL`), which means nothing in a page
/// opened from a file, and the proxy only serves a placeholder outside a
/// Guerrilla session. Each goes back to the image's own URL (`q`), or to
/// the proxy's full URL if there is none. They still load only with
/// `--images` (or the app's setting), like any provider's remote images.
fn unproxy_images(html: &str, web: &str) -> String {
    static PROXIED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?i)(\s(?:src|background)\s*=\s*)(?:"(/res\.php\?[^"]*)"|'(/res\.php\?[^']*)')"#,
        )
        .unwrap()
    });
    PROXIED
        .replace_all(html, |c: &regex::Captures| {
            let path = c
                .get(2)
                .or(c.get(3))
                .map_or("", |m| m.as_str())
                .replace("&amp;", "&")
                .replace("&#38;", "&")
                .replace("&#x26;", "&");
            let original = path
                .split_once('?')
                .map_or("", |(_, q)| q)
                .split('&')
                .find_map(|kv| kv.strip_prefix("q="))
                .map(percent_decode)
                .filter(|u| {
                    let lower = u.to_ascii_lowercase();
                    lower.starts_with("https://") || lower.starts_with("http://")
                });
            let url = original.unwrap_or_else(|| format!("{web}{path}"));
            let quoted = url.replace('&', "&amp;").replace('"', "&quot;");
            format!("{}\"{quoted}\"", &c[1])
        })
        .into_owned()
}

/// A query value as PHP's `urlencode` writes it: `%XX` bytes, `+` a space
/// (an encoded plus is `%2B`, so it survives the swap).
fn percent_decode(s: &str) -> String {
    String::from_utf8_lossy(&crate::render::percent_decode(&s.replace('+', " "))).into_owned()
}

/// Guerrilla HTML-escapes subjects and excerpts.
fn decode_entities(s: &str) -> String {
    s.replace("&#039;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

impl Provider for Guerrilla {
    fn id(&self) -> &'static str {
        "guerrilla"
    }
    fn name(&self) -> &'static str {
        "Guerrilla Mail"
    }
    fn website(&self) -> &'static str {
        "https://www.guerrillamail.com"
    }

    fn caps(&self) -> Capabilities {
        Capabilities {
            private: true,
            custom_user: true,
            choose_domain: true,
            attachments: true,
            raw: false,
            delete_message: true,
            delete_inbox: true,
            login: false,
            lifetime: "mail kept 60 minutes",
        }
    }

    fn domains(&self) -> Result<Vec<String>> {
        Ok(DOMAINS.iter().map(|d| d.to_string()).collect())
    }

    fn health(&self) -> Result<()> {
        // Starts an anonymous session but no inbox is kept.
        self.call("get_email_address", &[("lang", "en")])
            .map(|_| ())
    }

    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        if let Some(d) = &opts.domain {
            if !DOMAINS.contains(&d.as_str()) {
                return Err(domain_unavailable(self.id(), d, &self.domains()?));
            }
        }
        let mut v = self.call("get_email_address", &[("lang", "en")])?;
        let sid = str_field(&v, "sid_token").to_string();
        if let Some(user) = &opts.user {
            v = self.call(
                "set_email_user",
                &[("sid_token", &sid), ("email_user", user), ("lang", "en")],
            )?;
        }
        let address = str_field(&v, "email_addr");
        let Some((local, default_domain)) = address.split_once('@') else {
            return Err(self.http.bad("no email_addr in response"));
        };
        let domain = opts.domain.as_deref().unwrap_or(default_domain);
        let sid = match str_field(&v, "sid_token") {
            "" => sid,
            newer => newer.to_string(),
        };
        let mut inbox = Inbox::new(
            self.id(),
            format!("{local}@{domain}"),
            json!({ "sid_token": sid }),
        );
        inbox.expires_at = Some(Utc::now() + Duration::minutes(60));
        Ok(inbox)
    }

    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        let v = self.session_call(inbox, "get_email_list", &[("offset", "0")])?;
        Ok(v["list"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Self::summary)
            .collect())
    }

    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        let v = self.session_call(inbox, "fetch_email", &[("email_id", id)])?;
        if v.is_null() || v == json!(false) || v.get("mail_id").is_none() {
            return Err(message_not_found(id));
        }
        let s = Self::summary(&v);
        let body = str_field(&v, "mail_body").to_string();
        Ok(Message {
            id: s.id,
            from: s.from,
            to: vec![inbox.address.clone()],
            subject: s.subject,
            date: s.date,
            // Even plain-text mail comes back HTML-escaped inside <pre>.
            text: None,
            html: Some(unproxy_images(&body, &self.web)),
            attachments: attachments(&v),
        })
    }

    fn attachment(&self, inbox: &mut Inbox, msg: &Message, att: &Attachment) -> Result<Vec<u8>> {
        let sid = inbox.cred_str("sid_token").unwrap_or_default().to_string();
        let query = [
            ("get_att", ""),
            ("email_id", msg.id.as_str()),
            ("part_id", att.id.as_str()),
            ("sid_token", sid.as_str()),
        ];
        let url = format!("{}/", self.web);
        self.http
            .bytes(|| self.http.client().get(&url).query(&query))
    }

    fn delete_message(&self, inbox: &mut Inbox, id: &str) -> Result<()> {
        self.session_call(inbox, "del_email", &[("email_ids[]", id)])
            .map(|_| ())
    }

    fn delete_inbox(&self, inbox: &mut Inbox) -> Result<()> {
        let addr = inbox.address.clone();
        self.session_call(inbox, "forget_me", &[("email_addr", &addr)])
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn proxied_images_point_at_their_own_url() {
        let html = r#"<p><img src="/res.php?r=1&amp;n=img&amp;q=https%3A%2F%2Fexample.com%2Fpixel.gif%3Fa%3D1%26b%3D2" alt="pixel.gif">
            <IMG SRC='/res.php?r=1&amp;n=img'> <img src="https://cdn.example/x.png">
            <img src="/res.php?q=javascript%3Aalert(1)">
            <td background="/res.php?r=1&#38;q=https%3A%2F%2Fx.com%2Fo%27brien+bg.png">
            <img src='/res.php?q=https%3A%2F%2Fx.com%2Fa%22b.png'></p>"#;
        let out = unproxy_images(html, "https://www.guerrillamail.com");
        assert!(
            out.contains(
                r#"<img src="https://example.com/pixel.gif?a=1&amp;b=2" alt="pixel.gif">"#
            ),
            "{out}"
        );
        assert!(
            out.contains(r#"SRC="https://www.guerrillamail.com/res.php?r=1&amp;n=img""#),
            "{out}"
        );
        assert!(
            out.contains(r#"<img src="https://cdn.example/x.png">"#),
            "{out}"
        );
        assert!(!out.contains(r#"src="javascript"#), "{out}");
        assert!(
            out.contains(r#"background="https://x.com/o'brien bg.png""#),
            "{out}"
        );
        assert!(out.contains(r#"src="https://x.com/a&quot;b.png""#), "{out}");
    }

    #[test]
    fn create_custom_user_and_read() {
        let mut s = Server::new();
        s.mock("GET", "/")
            .match_query(Matcher::UrlEncoded("f".into(), "get_email_address".into()))
            .with_body(r#"{"email_addr":"rnd@guerrillamailblock.com","sid_token":"sid1"}"#)
            .create();
        s.mock("GET", "/")
            .match_query(Matcher::UrlEncoded("f".into(), "set_email_user".into()))
            .with_body(r#"{"email_addr":"otter@guerrillamailblock.com","sid_token":"sid1"}"#)
            .create();
        s.mock("GET", "/")
            .match_query(Matcher::UrlEncoded("f".into(), "get_email_list".into()))
            .with_body(
                r#"{"list":[{"mail_id":"7","mail_from":"x@y.z","mail_subject":"It&#039;s here",
                "mail_excerpt":"code 4242","mail_timestamp":"1791194910","mail_read":0,"att":"0"}],
                "sid_token":"sid1"}"#,
            )
            .create();
        s.mock("GET", "/")
            .match_query(Matcher::UrlEncoded("f".into(), "fetch_email".into()))
            .with_body(
                r#"{"mail_id":7,"mail_from":"x@y.z","mail_subject":"It&#039;s here",
                "mail_body":"<b>code 4242</b>","content_type":"text/html","mail_timestamp":1791194910,
                "att":1,"att_info":[{"f":"report.pdf","t":"application/pdf","p":"2"}],
                "sid_token":"sid1"}"#,
            )
            .create();

        s.mock("GET", "/")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("email_id".into(), "7".into()),
                Matcher::UrlEncoded("part_id".into(), "2".into()),
                Matcher::UrlEncoded("sid_token".into(), "sid1".into()),
            ]))
            .with_body("%PDF-1.4")
            .create();

        let p = Guerrilla {
            base: s.url() + "/",
            web: s.url(),
            http: Http::new("guerrilla"),
        };
        let mut inbox = p
            .create(&CreateOpts {
                user: Some("otter".into()),
                domain: Some("sharklasers.com".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(inbox.address, "otter@sharklasers.com");

        let list = p.list(&mut inbox).unwrap();
        assert_eq!(list[0].id, "7");
        assert_eq!(list[0].subject, "It's here");
        assert!(list[0].date.is_some());

        let msg = p.get(&mut inbox, "7").unwrap();
        assert_eq!(msg.html.as_deref(), Some("<b>code 4242</b>"));
        assert!(msg.text.is_none());
        assert_eq!(msg.attachments.len(), 1);
        assert_eq!(msg.attachments[0].filename, "report.pdf");
        assert_eq!(msg.attachments[0].content_type, "application/pdf");

        let data = p.attachment(&mut inbox, &msg, &msg.attachments[0]).unwrap();
        assert_eq!(data, b"%PDF-1.4");
    }
}
