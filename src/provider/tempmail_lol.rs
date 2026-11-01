//! tempmail.lol v2 free API: https://github.com/tempmail-lol/server/wiki/v2-API-Endpoints
//! One POST to create, one GET to poll. Free inboxes live one hour and their
//! messages carry no id, so ids are derived from their content.

use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{
    Capabilities, CreateOpts, Inbox, Message, Provider, Summary, base_url, opt_str, parse_date,
    str_field, synthetic_id,
};
use crate::error::{Error, Result, message_not_found, unsupported};
use crate::http::Http;

const BASE: &str = "https://api.tempmail.lol/v2";

pub struct TempmailLol {
    base: String,
    http: Http,
}

impl TempmailLol {
    pub fn new() -> Self {
        TempmailLol {
            base: base_url("tempmail_lol", BASE),
            http: Http::new("tempmail_lol"),
        }
    }

    /// Every email the inbox has received. The API hands out each email
    /// once, so the ones fetched are kept with the inbox (it lives an hour)
    /// and merged with whatever the API returns now.
    fn emails(&self, inbox: &mut Inbox) -> Result<Vec<Message>> {
        let token = inbox.cred_str("token").unwrap_or_default().to_string();
        let v: Value = match self.http.json(|| {
            self.http
                .client()
                .get(format!("{}/inbox", self.base))
                .query(&[("token", &token)])
        }) {
            Err(Error::NotFound(_)) => return Err(Error::Expired(inbox.address.clone())),
            other => other?,
        };
        if v["expired"].as_bool() == Some(true) {
            return Err(Error::Expired(inbox.address.clone()));
        }
        let mut kept = kept(inbox);
        let before = kept.len();
        for e in v["emails"].as_array().into_iter().flatten() {
            if !kept.contains(e) {
                kept.push(e.clone());
            }
        }
        if kept.len() != before {
            inbox.set_cred("emails", kept.clone());
        }
        Ok(kept.iter().map(|e| to_message(e, &inbox.address)).collect())
    }
}

/// The raw emails stored with the inbox by earlier fetches.
fn kept(inbox: &Inbox) -> Vec<Value> {
    inbox
        .credential
        .get("emails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn to_message(e: &Value, address: &str) -> Message {
    let date = parse_date(&e["date"]);
    let from = str_field(e, "from");
    let subject = str_field(e, "subject");
    let id = match e.get("_id").or_else(|| e.get("id")).and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => synthetic_id(&[&e["date"].to_string(), from, subject]),
    };
    Message {
        id,
        from: from.to_string(),
        to: vec![match str_field(e, "to") {
            "" => address.to_string(),
            to => to.to_string(),
        }],
        subject: subject.to_string(),
        date,
        text: opt_str(e, "body"),
        html: opt_str(e, "html"),
        attachments: Vec::new(),
    }
}

impl Provider for TempmailLol {
    fn id(&self) -> &'static str {
        "tempmail_lol"
    }
    fn name(&self) -> &'static str {
        "tempmail.lol"
    }
    fn website(&self) -> &'static str {
        "https://tempmail.lol"
    }

    fn caps(&self) -> Capabilities {
        Capabilities {
            private: true,
            custom_user: false,
            choose_domain: false,
            attachments: false,
            raw: false,
            delete_message: false,
            delete_inbox: false,
            login: false,
            lifetime: "1 hour",
        }
    }

    fn domains(&self) -> Result<Vec<String>> {
        Err(unsupported(self.id(), "listing domains (they rotate)"))
    }

    fn health(&self) -> Result<()> {
        // An unknown token gets a JSON 404 `{"expired":true}`: the API is up.
        match self.http.json::<Value>(|| {
            self.http
                .client()
                .get(format!("{}/inbox", self.base))
                .query(&[("token", "tmpmail-health-check")])
        }) {
            Ok(_) | Err(Error::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        if opts.user.is_some() {
            return Err(unsupported(self.id(), "choosing the username"));
        }
        if opts.domain.is_some() {
            return Err(unsupported(self.id(), "choosing the domain"));
        }
        let v: Value = self.http.json(|| {
            self.http
                .client()
                .post(format!("{}/inbox/create", self.base))
                .json(&json!({}))
        })?;
        let (address, token) = (str_field(&v, "address"), str_field(&v, "token"));
        if address.is_empty() || token.is_empty() {
            return Err(self.http.bad("address or token missing"));
        }
        let mut inbox = Inbox::new(self.id(), address.to_string(), json!({ "token": token }));
        inbox.expires_at = Some(Utc::now() + Duration::hours(1));
        Ok(inbox)
    }

    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        Ok(self.emails(inbox)?.iter().map(Message::summary).collect())
    }

    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        // Messages never change, so a stored one is current.
        let address = inbox.address.clone();
        if let Some(m) = kept(inbox)
            .iter()
            .map(|e| to_message(e, &address))
            .find(|m| m.id == id)
        {
            return Ok(m);
        }
        self.emails(inbox)?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| message_not_found(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn create_and_list_with_synthetic_ids() {
        let mut s = Server::new();
        s.mock("POST", "/inbox/create")
            .with_body(r#"{"address":"a1@kf.imagesthere.com","token":"t1"}"#)
            .create();
        s.mock("GET", "/inbox")
            .match_query(Matcher::UrlEncoded("token".into(), "t1".into()))
            .with_body(
                r#"{"emails":[{"from":"x@y.z","to":"a1@kf.imagesthere.com","subject":"S",
                "body":"code 9999","html":null,"date":1791194910000}],"expired":false}"#,
            )
            .create();
        let p = TempmailLol {
            base: s.url(),
            http: Http::new("tempmail_lol"),
        };
        let mut inbox = p.create(&CreateOpts::default()).unwrap();
        let list = p.list(&mut inbox).unwrap();
        assert_eq!(list.len(), 1);
        let msg = p.get(&mut inbox, &list[0].id).unwrap();
        assert_eq!(msg.text.as_deref(), Some("code 9999"));
        assert!(msg.html.is_none());
    }

    #[test]
    fn keeps_emails_the_api_returns_only_once() {
        let mut s = Server::new();
        let first = s
            .mock("GET", "/inbox")
            .match_query(Matcher::Any)
            .with_body(
                r#"{"emails":[{"from":"x@y.z","subject":"S","body":"code 9999",
                "date":1791194910000}],"expired":false}"#,
            )
            .expect(1)
            .create();
        let p = TempmailLol {
            base: s.url(),
            http: Http::new("tempmail_lol"),
        };
        let mut inbox = Inbox::new("tempmail_lol", "a@b.c".into(), json!({"token":"t"}));
        let id = p.list(&mut inbox).unwrap()[0].id.clone();
        first.assert();
        s.mock("GET", "/inbox")
            .match_query(Matcher::Any)
            .with_body(r#"{"emails":[],"expired":false}"#)
            .create();
        assert_eq!(p.list(&mut inbox).unwrap().len(), 1);
        assert_eq!(
            p.get(&mut inbox, &id).unwrap().text.as_deref(),
            Some("code 9999")
        );
    }

    #[test]
    fn encoded_headers_are_decoded() {
        let mut s = Server::new();
        s.mock("GET", "/inbox")
            .match_query(Matcher::Any)
            .with_body(
                r#"{"emails":[{"from":"=?utf-8?q?Zo=C3=AB_=C3=9Cnicode?= <hello@example.org>",
                "subject":"=?utf-8?b?Q2Fmw6kgcsOpc3Vtw6kg4pyT?= code",
                "body":"Grüße, code 551177\r\n","date":1791194910000}],"expired":false}"#,
            )
            .create();
        let p = super::super::Decoded(Box::new(TempmailLol {
            base: s.url(),
            http: Http::new("tempmail_lol"),
        }));
        let mut inbox = Inbox::new("tempmail_lol", "a@b.c".into(), json!({"token":"t"}));
        let list = p.list(&mut inbox).unwrap();
        assert_eq!(list[0].from, "Zoë Ünicode <hello@example.org>");
        assert_eq!(list[0].subject, "Café résumé ✓ code");
        assert_eq!(list[0].intro, "Grüße, code 551177");
        let msg = p.get(&mut inbox, &list[0].id).unwrap();
        assert_eq!(msg.subject, "Café résumé ✓ code");
    }

    #[test]
    fn expired_inbox_is_reported() {
        let mut s = Server::new();
        s.mock("GET", "/inbox")
            .match_query(Matcher::Any)
            .with_body(r#"{"emails":[],"expired":true}"#)
            .create();
        let p = TempmailLol {
            base: s.url(),
            http: Http::new("tempmail_lol"),
        };
        let mut inbox = Inbox::new("tempmail_lol", "a@b.c".into(), json!({"token":"t"}));
        assert!(matches!(p.list(&mut inbox), Err(Error::Expired(_))));
    }

    #[test]
    fn expired_inbox_404_is_reported_as_expired() {
        let mut s = Server::new();
        s.mock("GET", "/inbox")
            .match_query(Matcher::Any)
            .with_status(404)
            .with_body(r#"{"expired":true}"#)
            .create();
        let p = TempmailLol {
            base: s.url(),
            http: Http::new("tempmail_lol"),
        };
        let mut inbox = Inbox::new("tempmail_lol", "a@b.c".into(), json!({"token":"t"}));
        assert!(matches!(p.list(&mut inbox), Err(Error::Expired(_))));
    }
}
