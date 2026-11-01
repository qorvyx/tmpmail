//! DropMail.me GraphQL API: https://dropmail.me/api/
//! Needs a free `af_` token (one POST, no signup). Sessions die after idle
//! time; the stored `restoreKey` brings the same address back in a new session.

use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{
    Attachment, Capabilities, CreateOpts, Inbox, Message, Provider, PushEvent, PushStream, Summary,
    base_url, domain_unavailable, opt_str, parse_date, str_field, with_token,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration as StdDuration, Instant};

use crate::error::{Error, Result, message_not_found, unsupported};
use crate::http::Http;
use crate::store::{Store, create_private_dir, write_private};

const BASE: &str = "https://dropmail.me";
/// Longer lifetimes cost "credits" and trigger a captcha; 1d is free.
const TOKEN_LIFETIME: &str = "1d";
const TOKEN_REUSE_HOURS: i64 = 23;

/// The server holds a subscription request open until mail arrives (about
/// three minutes at most), so allow a little more than that.
const LONG_POLL: StdDuration = StdDuration::from_secs(240);

const MAILS_QUERY: &str = "query($id: ID!) { session(id: $id) { expiresAt mails { \
    id fromAddr toAddr receivedAt headerFrom headerSubject text html downloadUrl \
    attachments { id name mime rawSize downloadUrl } } } }";

pub struct DropMail {
    base: String,
    http: Http,
    /// Where the shared API token is cached. Generating one per call gets the
    /// IP challenged with a captcha quickly.
    cache: Option<PathBuf>,
    /// The last mailbox fetched, by address. `list` already downloads whole
    /// messages, so `get` and `raw` can take them from here.
    last_mails: RefCell<Option<(String, Vec<Value>)>>,
}

impl DropMail {
    pub fn new() -> Self {
        DropMail {
            base: base_url("dropmail", BASE),
            http: Http::new("dropmail"),
            cache: Some(Store::default_dir().join("dropmail-token.json")),
            last_mails: RefCell::default(),
        }
    }

    /// `TMPMAIL_DROPMAIL_TOKEN`, else a cached token younger than a day,
    /// else a freshly generated one (cached for next time).
    fn token(&self, refresh: bool) -> Result<String> {
        if let Ok(t) = std::env::var("TMPMAIL_DROPMAIL_TOKEN") {
            return Ok(t);
        }
        if !refresh {
            if let Some(t) = self.cached_token() {
                return Ok(t);
            }
        }
        let token = self.new_token()?;
        if let Some(path) = &self.cache {
            let data = json!({ "token": token, "created_at": Utc::now() }).to_string();
            if let Some(dir) = path.parent() {
                let _ = create_private_dir(dir);
            }
            let _ = write_private(path, data.as_bytes());
        }
        Ok(token)
    }

    fn cached_token(&self) -> Option<String> {
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(self.cache.as_ref()?).ok()?).ok()?;
        let created = parse_date(&v["created_at"])?;
        (Utc::now() - created < Duration::hours(TOKEN_REUSE_HOURS))
            .then(|| str_field(&v, "token").to_string())
            .filter(|t| !t.is_empty())
    }

    fn new_token(&self) -> Result<String> {
        let v: Value = match self.http.json(|| {
            self.http
                .client()
                .post(format!("{}/api/token/generate", self.base))
                .json(&json!({ "type": "af", "lifetime": TOKEN_LIFETIME }))
        }) {
            Err(Error::BadResponse {
                status: Some(402), ..
            }) => {
                return Err(self.http.bad(
                    "DropMail wants a captcha before issuing more API tokens to this IP; \
                     try later or set TMPMAIL_DROPMAIL_TOKEN (see https://dropmail.me/api/)",
                ));
            }
            other => other?,
        };
        match str_field(&v, "token") {
            "" => Err(self.http.bad("token missing from /api/token/generate")),
            t => Ok(t.to_string()),
        }
    }

    fn gql(&self, token: &str, query: &str, variables: &Value) -> Result<Value> {
        let url = format!("{}/api/graphql/{token}", self.base);
        self.http.graphql(&url, query, variables)
    }

    /// For calls outside an inbox: the cached token, replaced once if the API
    /// rejects it. Returns the token used along with the data.
    fn gql_any(&self, query: &str, variables: &Value) -> Result<(String, Value)> {
        let token = self.token(false)?;
        match self.gql(&token, query, variables) {
            Err(Error::Auth { .. }) => {
                let token = self.token(true)?;
                let data = self.gql(&token, query, variables)?;
                Ok((token, data))
            }
            other => other.map(|data| (token, data)),
        }
    }

    /// Ensures a valid token, regenerating it if the API rejects the stored one.
    fn gql_inbox(&self, inbox: &mut Inbox, query: &str, variables: Value) -> Result<Value> {
        let refresh = |inbox: &mut Inbox| {
            let token = self.token(true)?;
            inbox.set_cred("token", token.clone());
            Ok(token)
        };
        with_token(inbox, refresh, |token| self.gql(token, query, &variables))
    }

    /// Returns the session's mails, restoring the address into a fresh session
    /// when the old one expired.
    fn mails(&self, inbox: &mut Inbox) -> Result<Vec<Value>> {
        let session = inbox.cred_str("session_id").unwrap_or_default().to_string();
        let mut data = match self.gql_inbox(inbox, MAILS_QUERY, json!({ "id": session })) {
            // The API reports a dead session as an error, not a null session.
            Err(Error::BadResponse { detail, .. }) if detail.contains("session_not_found") => {
                Value::Null
            }
            other => other?,
        };
        let session = &mut data["session"];
        if session.is_object() {
            if let Some(exp) = parse_date(&session["expiresAt"]) {
                // Each check extends the session; recording every extension
                // would rewrite the state file on every poll.
                if inbox
                    .expires_at
                    .is_none_or(|old| (exp - old).num_seconds().abs() >= 60)
                {
                    inbox.expires_at = Some(exp);
                }
            }
            let mails = match session["mails"].take() {
                Value::Array(mails) => mails,
                _ => Vec::new(),
            };
            *self.last_mails.borrow_mut() = Some((inbox.address.clone(), mails.clone()));
            return Ok(mails);
        }
        self.restore(inbox)?;
        Ok(Vec::new())
    }

    fn find_mail(&self, inbox: &mut Inbox, id: &str) -> Result<Value> {
        // Mails never change, so one seen in the last listing is current.
        if let Some((address, mails)) = &*self.last_mails.borrow() {
            if *address == inbox.address {
                if let Some(m) = mails.iter().find(|m| str_field(m, "id") == id) {
                    return Ok(m.clone());
                }
            }
        }
        self.mails(inbox)?
            .into_iter()
            .find(|m| str_field(m, "id") == id)
            .ok_or_else(|| message_not_found(id))
    }

    fn restore(&self, inbox: &mut Inbox) -> Result<()> {
        let key = inbox
            .cred_str("restore_key")
            .ok_or_else(|| Error::Expired(inbox.address.clone()))?
            .to_string();
        let data = self.gql_inbox(
            inbox,
            "mutation { introduceSession(input: {withAddress: false}) { id expiresAt } }",
            json!({}),
        )?;
        let session = str_field(&data["introduceSession"], "id").to_string();
        let exp = parse_date(&data["introduceSession"]["expiresAt"]);
        let restored = self.gql_inbox(
            inbox,
            "mutation($s: ID!, $a: String!, $k: String!) { restoreAddress(input: \
             {sessionId: $s, mailAddress: $a, restoreKey: $k}) { address } }",
            json!({ "s": session, "a": inbox.address, "k": key }),
        )?;
        if restored["restoreAddress"].is_null() {
            return Err(Error::Expired(inbox.address.clone()));
        }
        inbox.set_cred("session_id", session);
        inbox.expires_at = exp;
        Ok(())
    }

    fn message(v: &Value) -> Message {
        let from = match str_field(v, "headerFrom") {
            "" => str_field(v, "fromAddr"),
            h => h,
        };
        Message {
            id: str_field(v, "id").to_string(),
            from: from.to_string(),
            to: vec![str_field(v, "toAddr").to_string()],
            subject: str_field(v, "headerSubject").to_string(),
            date: parse_date(&v["receivedAt"]),
            text: opt_str(v, "text"),
            html: opt_str(v, "html"),
            attachments: v["attachments"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|att| Attachment {
                            id: str_field(att, "id").to_string(),
                            filename: str_field(att, "name").to_string(),
                            content_type: str_field(att, "mime").to_string(),
                            size: att["rawSize"].as_u64(),
                            url: opt_str(att, "downloadUrl"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

impl Provider for DropMail {
    fn id(&self) -> &'static str {
        "dropmail"
    }
    fn name(&self) -> &'static str {
        "DropMail.me"
    }
    fn website(&self) -> &'static str {
        "https://dropmail.me"
    }

    fn caps(&self) -> Capabilities {
        Capabilities {
            private: true,
            custom_user: false,
            choose_domain: true,
            attachments: true,
            raw: true,
            delete_message: false,
            delete_inbox: false,
            login: false,
            lifetime: "session renewed on each check; address restorable later",
        }
    }

    fn domains(&self) -> Result<Vec<String>> {
        Ok(self
            .domain_ids()?
            .into_iter()
            .map(|(_, name)| name)
            .collect())
    }

    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        if opts.user.is_some() {
            return Err(unsupported(self.id(), "choosing the username"));
        }
        let domain_id = match &opts.domain {
            None => None,
            Some(d) => {
                let (ids, names): (Vec<_>, Vec<_>) = self.domain_ids()?.into_iter().unzip();
                match names.iter().position(|name| name == d) {
                    Some(k) => Some(ids[k].clone()),
                    None => return Err(domain_unavailable(self.id(), d, &names)),
                }
            }
        };
        let (token, data) = self.gql_any(
            "mutation($d: ID) { introduceSession(input: {withAddress: true, domainId: $d}) { \
             id expiresAt addresses { address restoreKey } } }",
            &json!({ "d": domain_id }),
        )?;
        let s = &data["introduceSession"];
        let addr = &s["addresses"][0];
        let address = str_field(addr, "address");
        if address.is_empty() {
            return Err(self.http.bad("no address in introduceSession response"));
        }
        let mut inbox = Inbox::new(
            self.id(),
            address.to_string(),
            json!({
                "token": token,
                "session_id": str_field(s, "id"),
                "restore_key": str_field(addr, "restoreKey"),
            }),
        );
        inbox.expires_at = parse_date(&s["expiresAt"]).or(Some(Utc::now() + Duration::minutes(10)));
        Ok(inbox)
    }

    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        Ok(self
            .mails(inbox)?
            .iter()
            .map(|m| Self::message(m).summary())
            .collect())
    }

    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        Ok(Self::message(&self.find_mail(inbox, id)?))
    }

    fn raw(&self, inbox: &mut Inbox, id: &str) -> Result<Vec<u8>> {
        let mail = self.find_mail(inbox, id)?;
        let url = self.checked_download(str_field(&mail, "downloadUrl"))?;
        self.http.bytes(|| self.http.client().get(&url))
    }

    fn push(&self, inbox: &Inbox) -> Option<Box<dyn PushStream>> {
        Some(Box::new(MailReceived {
            url: format!("{}/api/graphql/{}", self.base, inbox.cred_str("token")?),
            session: inbox.cred_str("session_id")?.to_string(),
            http: self.http.clone(),
        }))
    }

    fn attachment(&self, _inbox: &mut Inbox, _msg: &Message, att: &Attachment) -> Result<Vec<u8>> {
        let url = att
            .url
            .clone()
            .ok_or_else(|| self.http.bad("attachment has no download URL"))?;
        let url = self.checked_download(&url)?;
        self.http.bytes(|| self.http.client().get(&url))
    }
}

impl DropMail {
    /// A download address from DropMail's answer, if it is on DropMail's
    /// own site (it carries the session).
    fn checked_download(&self, url: &str) -> Result<String> {
        if super::on_provider_host(url, &self.base) {
            Ok(url.to_string())
        } else {
            Err(self
                .http
                .bad(format!("download address isn't on {}: refused", self.base)))
        }
    }

    fn domain_ids(&self) -> Result<Vec<(String, String)>> {
        let (_, data) = self.gql_any("{ domains { id name availableVia } }", &json!({}))?;
        let list: Vec<(String, String)> = data["domains"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| {
                d["availableVia"]
                    .as_array()
                    .is_none_or(|via| via.iter().any(|v| v == "API"))
            })
            .map(|d| {
                (
                    str_field(d, "id").to_string(),
                    str_field(d, "name").to_string(),
                )
            })
            .collect();
        if list.is_empty() {
            return Err(self.http.bad("no domains"));
        }
        Ok(list)
    }
}

/// The `sessionMailReceived` subscription, over HTTP long-polling: each
/// request returns one new mail, or nothing once the server's wait runs out.
/// (The API also takes it over WebSocket; this needs no extra client.)
struct MailReceived {
    url: String,
    session: String,
    http: Http,
}

impl PushStream for MailReceived {
    fn run(self: Box<Self>, event: &mut dyn FnMut(PushEvent) -> bool) -> Result<()> {
        if !event(PushEvent::Connected) {
            return Ok(());
        }
        loop {
            let started = Instant::now();
            let data = match self.http.graphql_within(
                &self.url,
                "subscription($id: ID!) { sessionMailReceived(id: $id) { id } }",
                &json!({ "id": self.session }),
                Some(LONG_POLL),
            ) {
                Ok(data) => data,
                // Our own timeout, not a failure: ask again.
                Err(_) if started.elapsed() >= LONG_POLL => continue,
                Err(e) => return Err(e),
            };
            if data["sessionMailReceived"].is_object() {
                if !event(PushEvent::Mail) {
                    return Ok(());
                }
            } else if started.elapsed() < StdDuration::from_secs(5) {
                // An immediate empty answer would make this loop spin; let
                // the caller reconnect later instead.
                return Ok(());
            } else if !event(PushEvent::Alive) {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn create_then_restore_expired_session() {
        let mut s = Server::new();
        s.mock("POST", "/api/token/generate")
            .with_body(r#"{"token":"af_tok"}"#)
            .create();
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex(
                "introduceSession\\(input: \\{withAddress: true".into(),
            ))
            .with_body(
                r#"{"data":{"introduceSession":{"id":"S1","expiresAt":"2030-01-01T00:00:00+00:00",
                "addresses":[{"address":"abc@10mail.org","restoreKey":"rk"}]}}}"#,
            )
            .create();
        let p = DropMail {
            base: s.url(),
            http: Http::new("dropmail"),
            cache: None,
            last_mails: RefCell::default(),
        };
        let mut inbox = p.create(&CreateOpts::default()).unwrap();
        assert_eq!(inbox.address, "abc@10mail.org");
        assert_eq!(inbox.cred_str("restore_key"), Some("rk"));

        // Session gone: list must restore the address into a new session.
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex("session\\(id".into()))
            .with_body(r#"{"errors":[{"message":"session_not_found"}],"data":{"session":null}}"#)
            .create();
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex("withAddress: false".into()))
            .with_body(r#"{"data":{"introduceSession":{"id":"S2","expiresAt":"2030-01-01T00:00:00+00:00"}}}"#)
            .create();
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex("restoreAddress".into()))
            .with_body(r#"{"data":{"restoreAddress":{"address":"abc@10mail.org"}}}"#)
            .create();
        assert!(p.list(&mut inbox).unwrap().is_empty());
        assert_eq!(inbox.cred_str("session_id"), Some("S2"));
    }

    #[test]
    fn subscription_reports_new_mail() {
        let mut s = Server::new();
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex("sessionMailReceived".into()))
            .match_body(Matcher::PartialJson(json!({"variables": {"id": "S1"}})))
            .with_body(r#"{"data":{"sessionMailReceived":{"id":"M1"}}}"#)
            .create();
        let p = DropMail {
            base: s.url(),
            http: Http::new("dropmail"),
            cache: None,
            last_mails: RefCell::default(),
        };
        let inbox = Inbox::new(
            "dropmail",
            "abc@10mail.org".into(),
            json!({"token":"af_tok","session_id":"S1"}),
        );
        let mut events = Vec::new();
        p.push(&inbox)
            .expect("dropmail has push")
            .run(&mut |e| {
                events.push(e);
                e != PushEvent::Mail
            })
            .unwrap();
        assert_eq!(events, [PushEvent::Connected, PushEvent::Mail]);
    }

    #[test]
    fn graphql_errors_are_surfaced() {
        let mut s = Server::new();
        s.mock("POST", "/api/token/generate")
            .with_body(r#"{"token":"af_tok"}"#)
            .create();
        s.mock("POST", "/api/graphql/af_tok")
            .with_body(r#"{"errors":[{"message":"RATE_LIMIT_EXCEEDED"}]}"#)
            .create();
        let p = DropMail {
            base: s.url(),
            http: Http::new("dropmail"),
            cache: None,
            last_mails: RefCell::default(),
        };
        assert!(matches!(
            p.domains().unwrap_err(),
            Error::RateLimited { .. }
        ));
    }
}
