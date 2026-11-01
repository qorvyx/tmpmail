//! mail.tm (and mail.gw, which runs the same software). Documented REST API:
//! https://docs.mail.tm — private inboxes behind address+password and a JWT.
//! Its terms ask clients to credit mail.tm, which the README and `--help` do.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{Value, json};

use super::{
    Attachment, Capabilities, CreateOpts, Inbox, Message, Provider, PushEvent, PushStream, Summary,
    base_url, domain_unavailable, opt_str, parse_date, random_password, random_user, source,
    str_field, with_token,
};
use crate::error::{Error, Result};
use crate::http::Http;

const PAGE_SIZE: usize = 30;
/// Where mail.tm cuts a message's text and HTML (102 400 bytes).
const BODY_CAP: usize = 100 * 1024;
const MAX_PAGES: usize = 5;
/// The Mercure hub sends a comment line every 30 s; this much silence means
/// the connection is dead.
const MERCURE_IDLE: Duration = Duration::from_secs(75);
/// A stream ends itself after this long, so the poller reconnects it with
/// the inbox's current token (the hub checks it only when a stream starts).
const MERCURE_MAX_AGE: Duration = Duration::from_secs(30 * 60);

pub struct MailTm {
    id: &'static str,
    name: &'static str,
    website: &'static str,
    base: String,
    /// Mercure hub that pushes account events (SSE); mail.gw has none.
    mercure: Option<String>,
    http: Http,
}

impl MailTm {
    pub fn tm() -> Self {
        let mut tm = Self::with_base(
            "mailtm",
            "mail.tm",
            "https://mail.tm",
            "https://api.mail.tm",
        );
        tm.mercure = Some(base_url("mailtm_mercure", "https://mercure.mail.tm"));
        tm
    }

    pub fn gw() -> Self {
        Self::with_base(
            "mailgw",
            "mail.gw",
            "https://mail.gw",
            "https://api.mail.gw",
        )
    }

    fn with_base(id: &'static str, name: &'static str, website: &'static str, base: &str) -> Self {
        MailTm {
            id,
            name,
            website,
            base: base_url(id, base),
            mercure: None,
            http: Http::new(id),
        }
    }

    /// mail.tm (and mail.gw) cut a message's text, and its HTML, at
    /// [`BODY_CAP`] bytes, mid-line and without a word. A body that long
    /// (or a few bytes short, cut at a character's edge) is checked against
    /// the raw source, which is whole: when the source's body is longer, it
    /// was cut, and the source's is used. When the source can't be read or
    /// has no longer body to give, the body says it may be cut and where
    /// the rest is.
    fn whole_bodies(&self, inbox: &mut Inbox, msg: &mut Message) {
        let long = |b: &Option<String>| b.as_ref().is_some_and(|b| b.len() + 4 > BODY_CAP);
        if !long(&msg.text) && !long(&msg.html) {
            return;
        }
        let (text, html) = match self.raw(inbox, &msg.id) {
            Ok(raw) => source::bodies(&raw),
            Err(_) => (None, None),
        };
        let note = format!(
            "{} may have cut this message at 100 KiB. The whole message is in its source: \
             `tmpmail read --raw`, or Source in the desktop app.",
            self.name
        );
        for (body, whole, html) in [(&mut msg.text, text, false), (&mut msg.html, html, true)] {
            let Some(cut) = body.as_mut().filter(|b| b.len() + 4 > BODY_CAP) else {
                continue;
            };
            match whole.filter(|w| w.len() > cut.len()) {
                Some(whole) => *cut = whole,
                None if html => cut.push_str(&format!("\n<p><b>[{note}]</b></p>")),
                None => cut.push_str(&format!("\n\n[{note}]")),
            }
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn token(&self, address: &str, password: &str) -> Result<(String, String)> {
        let v: Value = self.http.json(|| {
            self.http
                .client()
                .post(self.url("/token"))
                .json(&json!({ "address": address, "password": password }))
        })?;
        let token = str_field(&v, "token");
        if token.is_empty() {
            return Err(self.http.bad("token missing from /token response"));
        }
        Ok((token.to_string(), str_field(&v, "id").to_string()))
    }

    /// Runs an authenticated request, refreshing the JWT once if it expired.
    fn authed<T>(&self, inbox: &mut Inbox, call: impl Fn(&str) -> Result<T>) -> Result<T> {
        let refresh = |inbox: &mut Inbox| {
            let password = inbox
                .cred_str("password")
                .ok_or_else(|| {
                    self.http
                        .bad("stored inbox has no password, cannot refresh token")
                })?
                .to_string();
            let (token, account_id) = self.token(&inbox.address, &password)?;
            inbox.set_cred("token", token.clone());
            if !account_id.is_empty() {
                inbox.set_cred("account_id", account_id);
            }
            Ok(token)
        };
        with_token(inbox, refresh, call)
    }

    fn get_json(&self, inbox: &mut Inbox, path: &str, query: &[(&str, usize)]) -> Result<Value> {
        self.authed(inbox, |token| {
            self.http.json(|| {
                self.http
                    .client()
                    .get(self.url(path))
                    .query(query)
                    .header("Accept", "application/json")
                    .bearer_auth(token)
            })
        })
    }

    fn delete(&self, inbox: &mut Inbox, path: &str) -> Result<()> {
        self.authed(inbox, |token| {
            self.http
                .send(|| self.http.client().delete(self.url(path)).bearer_auth(token))
                .map(|_| ())
        })
    }

    fn summary(v: &Value) -> Summary {
        Summary {
            id: str_field(v, "id").to_string(),
            from: format_addr(&v["from"]),
            subject: str_field(v, "subject").to_string(),
            intro: str_field(v, "intro").to_string(),
            date: parse_date(&v["createdAt"]),
            seen: v["seen"].as_bool().unwrap_or(false),
            has_attachments: v["hasAttachments"].as_bool().unwrap_or(false),
        }
    }
}

fn format_addr(v: &Value) -> String {
    let addr = str_field(v, "address");
    match str_field(v, "name") {
        "" => addr.to_string(),
        name => format!("{name} <{addr}>"),
    }
}

/// Hydra endpoints return a bare array with `Accept: application/json`, but
/// accept the JSON-LD envelope too in case that changes.
fn members(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        mut other => match other.get_mut("hydra:member").map(Value::take) {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        },
    }
}

impl Provider for MailTm {
    fn id(&self) -> &'static str {
        self.id
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn website(&self) -> &'static str {
        self.website
    }

    fn caps(&self) -> Capabilities {
        Capabilities {
            private: true,
            custom_user: true,
            choose_domain: true,
            attachments: true,
            raw: true,
            delete_message: true,
            delete_inbox: true,
            login: true,
            lifetime: "until deleted; messages kept 7 days",
        }
    }

    fn domains(&self) -> Result<Vec<String>> {
        let v: Value = self.http.json(|| {
            self.http
                .client()
                .get(self.url("/domains"))
                .header("Accept", "application/json")
        })?;
        let domains: Vec<String> = members(v)
            .iter()
            .filter(|d| d["isActive"].as_bool().unwrap_or(true))
            .map(|d| str_field(d, "domain").to_string())
            .filter(|d| !d.is_empty())
            .collect();
        if domains.is_empty() {
            return Err(self.http.bad("no active domains"));
        }
        Ok(domains)
    }

    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        let domains = self.domains()?;
        let domain = match &opts.domain {
            Some(d) if domains.contains(d) => d.clone(),
            Some(d) => return Err(domain_unavailable(self.id, d, &domains)),
            None => domains[rand::random_range(0..domains.len())].clone(),
        };
        let password = opts.password.clone().unwrap_or_else(random_password);
        // mail.tm rejects some usernames (a word filter: "steadymoose" fails,
        // "quietotter" passes), so a random name gets a few fresh tries.
        let attempts = if opts.user.is_some() { 1 } else { 4 };
        let mut address = String::new();
        for attempt in 1..=attempts {
            let user = opts.user.clone().unwrap_or_else(random_user).to_lowercase();
            address = format!("{user}@{domain}");
            let created = self.http.json::<Value>(|| {
                self.http
                    .client()
                    .post(self.url("/accounts"))
                    .json(&json!({ "address": address, "password": password }))
            });
            match created {
                Ok(_) => break,
                // 422 = address already used or rejected.
                Err(Error::BadResponse {
                    detail,
                    status: Some(422),
                    ..
                }) => {
                    if attempt == attempts {
                        return Err(Error::AddressTaken(format!(
                            "{}: {address} is taken or invalid ({detail})",
                            self.id
                        )));
                    }
                }
                Err(e) => return Err(e),
            }
        }

        self.login(&address, &password)
    }

    fn login(&self, address: &str, password: &str) -> Result<Inbox> {
        let (token, account_id) = self.token(address, password)?;
        Ok(Inbox::new(
            self.id,
            address.to_string(),
            json!({ "password": password, "token": token, "account_id": account_id }),
        ))
    }

    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            let batch = members(self.get_json(inbox, "/messages", &[("page", page)])?);
            let n = batch.len();
            all.extend(batch.iter().map(Self::summary));
            if n < PAGE_SIZE {
                break;
            }
        }
        Ok(all)
    }

    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        let v = self.get_json(inbox, &format!("/messages/{id}"), &[])?;
        let html = match &v["html"] {
            Value::Array(parts) => {
                let joined: String = parts.iter().filter_map(Value::as_str).collect();
                (!joined.is_empty()).then_some(joined)
            }
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        let attachments = v["attachments"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|att| Attachment {
                        id: str_field(att, "id").to_string(),
                        filename: str_field(att, "filename").to_string(),
                        content_type: str_field(att, "contentType").to_string(),
                        // In kilobytes, rounded up (19 bytes comes back as 1).
                        size: att["size"].as_u64().map(|kb| kb * 1024),
                        url: match str_field(att, "downloadUrl") {
                            "" => None,
                            u if u.starts_with("http") => Some(u.to_string()),
                            u => Some(self.url(u)),
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut msg = Message {
            id: str_field(&v, "id").to_string(),
            from: format_addr(&v["from"]),
            to: v["to"]
                .as_array()
                .map(|a| a.iter().map(format_addr).collect())
                .unwrap_or_default(),
            subject: str_field(&v, "subject").to_string(),
            date: parse_date(&v["createdAt"]),
            text: opt_str(&v, "text"),
            html,
            attachments,
        };
        self.whole_bodies(inbox, &mut msg);
        Ok(msg)
    }

    fn raw(&self, inbox: &mut Inbox, id: &str) -> Result<Vec<u8>> {
        let v = self.get_json(inbox, &format!("/sources/{id}"), &[])?;
        match v["data"].as_str() {
            Some(data) => Ok(data.as_bytes().to_vec()),
            None => Err(self.http.bad("message source missing `data`")),
        }
    }

    fn attachment(&self, inbox: &mut Inbox, msg: &Message, att: &Attachment) -> Result<Vec<u8>> {
        let url = att
            .url
            .clone()
            .unwrap_or_else(|| self.url(&format!("/messages/{}/attachment/{}", msg.id, att.id)));
        // The account's token goes only to the provider's own site.
        if !super::on_provider_host(&url, &self.base) {
            return Err(self.http.bad(format!(
                "attachment download address isn't on {}: refused",
                self.base
            )));
        }
        self.authed(inbox, |token| {
            self.http
                .bytes(|| self.http.client().get(&url).bearer_auth(token))
        })
    }

    fn delete_message(&self, inbox: &mut Inbox, id: &str) -> Result<()> {
        self.delete(inbox, &format!("/messages/{id}"))
    }

    fn push(&self, inbox: &Inbox) -> Option<Box<dyn PushStream>> {
        let account = inbox.cred_str("account_id").filter(|a| !a.is_empty())?;
        Some(Box::new(Mercure {
            url: format!("{}/.well-known/mercure", self.mercure.as_ref()?),
            topic: format!("/accounts/{account}"),
            token: inbox.cred_str("token")?.to_string(),
            http: self.http.clone(),
        }))
    }

    fn delete_inbox(&self, inbox: &mut Inbox) -> Result<()> {
        if inbox.cred_str("account_id").is_none() {
            // Older state without an id: a token refresh returns it.
            self.authed(inbox, |_| Ok(()))?;
        }
        let account = inbox.cred_str("account_id").unwrap_or_default().to_string();
        if account.is_empty() {
            return Err(self.http.bad("account id unknown"));
        }
        self.delete(inbox, &format!("/accounts/{account}"))
    }
}

/// mail.tm's server-sent events: the hub publishes the account's changes
/// (new mail, quota) to `/accounts/{id}`, readable with the account's JWT.
struct Mercure {
    url: String,
    topic: String,
    token: String,
    http: Http,
}

/// The longest line of the push stream read: its events are a few hundred
/// bytes.
const MAX_SSE_LINE: usize = 1 << 20;

/// The next line of the push stream (without its end), `None` at its end,
/// or an error for a line past [`MAX_SSE_LINE`]: an endless line would
/// otherwise grow without bound while the idle limit never fires.
fn sse_line(reader: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    match crate::lines::read_line(reader, MAX_SSE_LINE)? {
        None => Ok(None),
        Some(crate::lines::Line::TooLong) => {
            Err(std::io::Error::other("a line of the stream is too long"))
        }
        Some(crate::lines::Line::Text(mut line)) => {
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            Ok(Some(line))
        }
    }
}

impl PushStream for Mercure {
    fn run(self: Box<Self>, event: &mut dyn FnMut(PushEvent) -> bool) -> Result<()> {
        // No `.timeout()` on the request: that would end the stream after
        // MERCURE_IDLE however busy it was. The client's bounds each read.
        let client = self.http.stream_client(MERCURE_IDLE);
        let resp = self.http.send(|| {
            client
                .get(&self.url)
                .query(&[("topic", &self.topic)])
                .header("Accept", "text/event-stream")
                .bearer_auth(&self.token)
        })?;
        if !event(PushEvent::Connected) {
            return Ok(());
        }
        let started = std::time::Instant::now();
        let mut reader = BufReader::new(resp);
        loop {
            let line = match sse_line(&mut reader) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(e) => {
                    return Err(Error::Unreachable {
                        provider: "mailtm".into(),
                        detail: format!("push stream: {e}"),
                    });
                }
            };
            let kind = if line.starts_with(b"data:") {
                PushEvent::Mail
            } else {
                PushEvent::Alive
            };
            if !event(kind) || started.elapsed() >= MERCURE_MAX_AGE {
                return Ok(());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    fn provider(server: &Server) -> MailTm {
        MailTm {
            id: "mailtm",
            name: "mail.tm",
            website: "https://mail.tm",
            base: server.url(),
            mercure: Some(server.url()),
            http: Http::new("mailtm"),
        }
    }

    #[test]
    fn create_list_and_read() {
        let mut s = Server::new();
        s.mock("GET", "/domains")
            .with_body(r#"[{"domain":"maxxspace.com","isActive":true}]"#)
            .create();
        s.mock("POST", "/accounts")
            .match_body(Matcher::PartialJson(
                json!({"address":"otter1@maxxspace.com"}),
            ))
            .with_status(201)
            .with_body(r#"{"id":"acc1","address":"otter1@maxxspace.com"}"#)
            .create();
        s.mock("POST", "/token")
            .with_body(r#"{"token":"jwt1","id":"acc1"}"#)
            .create();
        s.mock("GET", "/messages")
            .match_query(Matcher::UrlEncoded("page".into(), "1".into()))
            .match_header("authorization", "Bearer jwt1")
            .with_body(
                r#"[{"id":"m1","from":{"address":"a@b.c","name":"Ann"},"subject":"Hi",
                "intro":"hello","seen":false,"hasAttachments":true,
                "createdAt":"2026-10-05T10:00:00+00:00"}]"#,
            )
            .create();
        s.mock("GET", "/messages/m1")
            .with_body(
                r#"{"id":"m1","from":{"address":"a@b.c","name":""},"to":[{"address":"otter1@maxxspace.com","name":""}],
                "subject":"Hi","text":"Your code is 123456","html":["<p>Your code</p>"," is 123456"],
                "createdAt":"2026-10-05T10:00:00+00:00",
                "attachments":[{"id":"ATT1","filename":"a.txt","contentType":"text/plain","size":3,
                "downloadUrl":"/messages/m1/attachment/ATT1"}]}"#,
            )
            .create();

        let p = provider(&s);
        let mut inbox = p
            .create(&CreateOpts {
                user: Some("Otter1".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(inbox.address, "otter1@maxxspace.com");
        assert_eq!(inbox.cred_str("account_id"), Some("acc1"));

        let list = p.list(&mut inbox).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].from, "Ann <a@b.c>");
        assert!(list[0].has_attachments);

        let msg = p.get(&mut inbox, "m1").unwrap();
        assert_eq!(msg.html.as_deref(), Some("<p>Your code</p> is 123456"));
        assert_eq!(msg.to, vec!["otter1@maxxspace.com"]);
        assert_eq!(msg.attachments[0].size, Some(3 * 1024));
        assert_eq!(
            msg.attachments[0].url.as_deref(),
            Some(format!("{}/messages/m1/attachment/ATT1", s.url()).as_str())
        );
    }

    /// A long mail: 2 500 lines, its text cut at
    /// exactly 102 400 bytes, mid-line; the source has all of it.
    #[test]
    fn a_cut_text_comes_whole_from_the_source() {
        let whole: String = (1..=2500)
            .map(|n| format!("Line {n:04}: the quick brown fox jumps over the lazy dog\n"))
            .collect();
        let cut = &whole[..BODY_CAP];
        let message = |id: &str| {
            json!({"id": id, "from": {"address": "a@b.c", "name": ""}, "to": [], "subject": "Long",
                "text": cut, "html": [], "createdAt": "2026-11-01T04:00:00+00:00", "attachments": []})
            .to_string()
        };
        let mut s = Server::new();
        for id in ["m1", "m2", "m3"] {
            s.mock("GET", format!("/messages/{id}").as_str())
                .with_body(message(id))
                .create();
        }
        let source = format!(
            "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{whole}"
        );
        s.mock("GET", "/sources/m1")
            .with_body(json!({"id": "m1", "data": source}).to_string())
            .create();
        s.mock("GET", "/sources/m2").with_status(500).create();
        // A source in a charset tmpmail can't decode: the cut text stays,
        // with the note, rather than garbage.
        let source = format!("Content-Type: text/plain; charset=koi8-r\r\n\r\n{whole}");
        s.mock("GET", "/sources/m3")
            .with_body(json!({"id": "m3", "data": source}).to_string())
            .create();
        let p = provider(&s);
        let mut inbox = Inbox::new(
            "mailtm",
            "x@x.com".into(),
            json!({"password":"pw","token":"t"}),
        );

        let msg = p.get(&mut inbox, "m1").unwrap();
        assert_eq!(msg.text.as_deref(), Some(whole.as_str()));
        assert!(
            msg.text
                .unwrap()
                .ends_with("Line 2500: the quick brown fox jumps over the lazy dog\n")
        );
        // No source: the cut text says so.
        let msg = p.get(&mut inbox, "m2").unwrap();
        let text = msg.text.unwrap();
        assert!(text.starts_with(cut));
        assert!(
            text.ends_with("`tmpmail read --raw`, or Source in the desktop app.]"),
            "{}",
            &text[BODY_CAP..]
        );
        let text = p.get(&mut inbox, "m3").unwrap().text.unwrap();
        assert!(text.starts_with(cut) && text.ends_with("desktop app.]"));
    }

    #[test]
    fn refreshes_expired_token_once() {
        let mut s = Server::new();
        s.mock("GET", "/messages")
            .match_query(Matcher::Any)
            .match_header("authorization", "Bearer old")
            .with_status(401)
            .with_body(r#"{"message":"Expired JWT Token"}"#)
            .create();
        let fresh = s
            .mock("POST", "/token")
            .with_body(r#"{"token":"new","id":"acc9"}"#)
            .expect(1)
            .create();
        s.mock("GET", "/messages")
            .match_query(Matcher::Any)
            .match_header("authorization", "Bearer new")
            .with_body("[]")
            .create();

        let p = provider(&s);
        let mut inbox = Inbox::new(
            "mailtm",
            "x@maxxspace.com".into(),
            json!({"password":"pw","token":"old"}),
        );
        assert!(p.list(&mut inbox).unwrap().is_empty());
        assert_eq!(inbox.cred_str("token"), Some("new"));
        assert_eq!(inbox.cred_str("account_id"), Some("acc9"));
        fresh.assert();
    }

    #[test]
    fn mercure_stream_reports_new_mail() {
        let mut s = Server::new();
        s.mock("GET", "/.well-known/mercure")
            .match_query(Matcher::UrlEncoded("topic".into(), "/accounts/acc1".into()))
            .match_header("authorization", "Bearer jwt1")
            .with_header("content-type", "text/event-stream")
            .with_body(":\n\nid: 1\nevent: message\ndata: {\"@type\":\"Message\"}\n\n")
            .create();
        let inbox = Inbox::new(
            "mailtm",
            "x@maxxspace.com".into(),
            json!({"token":"jwt1","account_id":"acc1"}),
        );
        let stream = provider(&s).push(&inbox).expect("mail.tm has push");
        let mut events = Vec::new();
        stream
            .run(&mut |e| {
                events.push(e);
                true
            })
            .unwrap();
        assert_eq!(events[0], PushEvent::Connected);
        assert_eq!(events.iter().filter(|e| **e == PushEvent::Mail).count(), 1);

        // Without an account id there is no topic to follow.
        let bare = Inbox::new("mailtm", "x@maxxspace.com".into(), json!({"token":"jwt1"}));
        assert!(provider(&s).push(&bare).is_none());

        // mail.gw has no Mercure hub, so no push (the settings screen says so).
        assert!(MailTm::gw().push(&inbox).is_none());
    }

    #[test]
    fn taken_address_is_reported_clearly() {
        let mut s = Server::new();
        s.mock("GET", "/domains")
            .with_body(r#"[{"domain":"maxxspace.com","isActive":true}]"#)
            .create();
        s.mock("POST", "/accounts")
            .with_status(422)
            .with_body(r#"{"hydra:description":"address: This value is already used."}"#)
            .create();
        let err = provider(&s)
            .create(&CreateOpts {
                user: Some("taken".into()),
                ..Default::default()
            })
            .unwrap_err();
        assert!(matches!(err, Error::AddressTaken(_)), "{err:?}");
        assert!(err.to_string().contains("already used"));
    }

    #[test]
    fn html_error_page_is_not_reported_as_json_garbage() {
        let mut s = Server::new();
        s.mock("GET", "/domains")
            .with_body("<html>Service unavailable</html>")
            .create();
        let err = provider(&s).domains().unwrap_err();
        assert!(
            err.to_string().contains("HTML page instead of JSON"),
            "{err}"
        );
        assert!(err.is_provider_failure());
    }

    #[test]
    fn server_error_counts_as_unreachable() {
        let mut s = Server::new();
        s.mock("GET", "/domains").with_status(502).create();
        let err = provider(&s).domains().unwrap_err();
        assert!(matches!(err, Error::Unreachable { .. }), "{err:?}");
    }

    #[test]
    fn an_endless_push_line_is_refused() {
        // Security review A5: one endless line grew without bound.
        let mut ok = std::io::Cursor::new(b"data: {}\r\n:\n".to_vec());
        assert_eq!(sse_line(&mut ok).unwrap().unwrap(), b"data: {}");
        assert_eq!(sse_line(&mut ok).unwrap().unwrap(), b":");
        assert!(sse_line(&mut ok).unwrap().is_none());
        let endless = std::io::Read::chain(
            std::io::Cursor::new(b"data: ".to_vec()),
            std::io::repeat(b'x'),
        );
        let mut endless = std::io::BufReader::new(endless);
        assert!(sse_line(&mut endless).is_err());
    }
}
