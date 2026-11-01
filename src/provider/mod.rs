mod dropmail;
mod guerrilla;
mod maildrop;
mod mailtm;
mod mime;
mod tempmail_lol;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result, unsupported};

/// Providers tried by `new` when none is configured. Public-inbox providers
/// (maildrop) are left out on purpose: anyone who guesses the name reads the mail.
pub const DEFAULT_ORDER: &[&str] = &["mailtm", "dropmail", "guerrilla", "tempmail_lol", "mailgw"];

type Constructor = fn() -> Box<dyn Provider>;

/// Every provider, in the order `tmpmail providers` lists them.
const REGISTRY: &[(&str, Constructor)] = &[
    ("mailtm", || Box::new(mailtm::MailTm::tm())),
    ("mailgw", || Box::new(mailtm::MailTm::gw())),
    ("dropmail", || Box::new(dropmail::DropMail::new())),
    ("guerrilla", || Box::new(guerrilla::Guerrilla::new())),
    (
        "tempmail_lol",
        || Box::new(tempmail_lol::TempmailLol::new()),
    ),
    ("maildrop", || Box::new(maildrop::Maildrop::new())),
];

pub fn all() -> impl Iterator<Item = &'static str> {
    REGISTRY.iter().map(|(id, _)| *id)
}

pub fn get(id: &str) -> Result<Box<dyn Provider>> {
    match REGISTRY.iter().find(|(known, _)| *known == id) {
        Some((_, make)) => Ok(Box::new(Decoded(make()))),
        None => Err(Error::Usage(format!(
            "unknown provider {id:?}, expected one of: {}",
            all().collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// A stored inbox. `credential` is provider-specific (JWT, session token, ...)
/// and is what lets us resume the inbox later.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Inbox {
    pub provider: String,
    pub address: String,
    #[serde(default)]
    pub credential: Value,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Ids of the messages opened here (`read`, the TUI, the desktop app),
    /// oldest first, so every front end shows them read, with any
    /// provider. Only [`crate::app::App::mark_read`] changes it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
}

impl Inbox {
    pub fn new(provider: &str, address: String, credential: Value) -> Self {
        Inbox {
            provider: provider.to_string(),
            address,
            credential,
            created_at: Utc::now(),
            expires_at: None,
            label: None,
            read: Vec::new(),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|t| t <= Utc::now())
    }

    pub fn cred_str(&self, key: &str) -> Option<&str> {
        self.credential.get(key).and_then(Value::as_str)
    }

    pub fn set_cred(&mut self, key: &str, value: impl Into<Value>) {
        if !self.credential.is_object() {
            self.credential = Value::Object(Default::default());
        }
        self.credential[key] = value.into();
    }

    pub fn local_part(&self) -> &str {
        local_part(&self.address)
    }
}

pub fn local_part(address: &str) -> &str {
    address.split('@').next().unwrap_or(address)
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Summary {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub intro: String,
    pub date: Option<DateTime<Utc>>,
    pub seen: bool,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Message {
    pub id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub date: Option<DateTime<Utc>>,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
}

impl Message {
    /// Listing entry for providers that only return full messages.
    pub fn summary(&self) -> Summary {
        Summary {
            id: self.id.clone(),
            from: self.from.clone(),
            subject: self.subject.clone(),
            intro: self
                .text
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(100)
                .collect::<String>()
                .trim_end()
                .to_string(),
            date: self.date,
            seen: false,
            has_attachments: !self.attachments.is_empty(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub size: Option<u64>,
    /// Absolute URL when the provider exposes one directly.
    #[serde(skip)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    /// Only the holder of the credential can read the inbox.
    pub private: bool,
    pub custom_user: bool,
    pub choose_domain: bool,
    pub attachments: bool,
    pub raw: bool,
    pub delete_message: bool,
    pub delete_inbox: bool,
    pub login: bool,
    /// Human description of how long an address lives.
    pub lifetime: &'static str,
}

#[derive(Debug, Default, Clone)]
pub struct CreateOpts {
    pub user: Option<String>,
    pub domain: Option<String>,
    pub password: Option<String>,
}

/// What a [`PushStream`] reports to its listener.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PushEvent {
    /// The stream is open; mail will be announced from now on.
    Connected,
    /// Something arrived (or changed) in the inbox: time to list it.
    Mail,
    /// A keep-alive, so the listener can stop a stream that is otherwise idle.
    Alive,
}

/// A live connection that announces new mail, so `wait` and `watch` can list
/// the inbox at once instead of on their next tick. It runs on its own
/// thread, so it carries copies of the credentials it needs.
pub trait PushStream: Send {
    /// Blocks while the connection lasts, passing each event to `event`, and
    /// returns once the server closes it, it fails, or `event` returns false.
    /// The caller reconnects.
    fn run(self: Box<Self>, event: &mut dyn FnMut(PushEvent) -> bool) -> Result<()>;
}

/// One temp-mail backend. Methods take `&mut Inbox` so a provider can refresh
/// tokens or sessions in place; callers persist the inbox afterwards.
pub trait Provider {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn website(&self) -> &'static str;
    fn caps(&self) -> Capabilities;

    fn domains(&self) -> Result<Vec<String>>;
    fn create(&self, opts: &CreateOpts) -> Result<Inbox>;
    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>>;
    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message>;

    /// Cheap call used by `providers --check`. Must not create inboxes.
    fn health(&self) -> Result<()> {
        self.domains().map(|_| ())
    }

    fn raw(&self, _inbox: &mut Inbox, _id: &str) -> Result<Vec<u8>> {
        Err(unsupported(self.id(), "raw message source"))
    }

    fn attachment(&self, _inbox: &mut Inbox, _msg: &Message, _att: &Attachment) -> Result<Vec<u8>> {
        Err(unsupported(self.id(), "attachments"))
    }

    fn delete_message(&self, _inbox: &mut Inbox, _id: &str) -> Result<()> {
        Err(unsupported(self.id(), "deleting messages"))
    }

    /// Deletes the inbox on the provider side. Providers without such a call
    /// just let it expire, which is fine: we forget it locally either way.
    fn delete_inbox(&self, _inbox: &mut Inbox) -> Result<()> {
        Ok(())
    }

    fn login(&self, _address: &str, _password: &str) -> Result<Inbox> {
        Err(unsupported(self.id(), "logging in to an existing address"))
    }

    /// A push stream for this inbox, for providers that offer one. Polling
    /// stays the fallback, so a provider without push just returns `None`.
    fn push(&self, _inbox: &Inbox) -> Option<Box<dyn PushStream>> {
        None
    }

    /// Whether `login` can tell from the address alone that it belongs here.
    fn owns_address(&self, _address: &str) -> bool {
        false
    }
}

/// A provider whose headers come out decoded: RFC 2047 encoded-words in
/// senders, recipients, subjects and attachment names (tempmail.lol passes
/// them on as they are), and no line break at the end of the preview.
/// Every other call is passed through, so it must list every trait method.
struct Decoded(Box<dyn Provider>);

fn decoded_summary(mut s: Summary) -> Summary {
    s.from = mime::decode_words(&s.from);
    s.subject = mime::decode_words(&s.subject);
    s.intro.truncate(s.intro.trim_end().len());
    s
}

fn decoded_message(mut m: Message) -> Message {
    m.from = mime::decode_words(&m.from);
    m.subject = mime::decode_words(&m.subject);
    for to in &mut m.to {
        *to = mime::decode_words(to);
    }
    for a in &mut m.attachments {
        a.filename = mime::decode_words(&a.filename);
    }
    m
}

impl Provider for Decoded {
    fn id(&self) -> &'static str {
        self.0.id()
    }
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn website(&self) -> &'static str {
        self.0.website()
    }
    fn caps(&self) -> Capabilities {
        self.0.caps()
    }
    fn domains(&self) -> Result<Vec<String>> {
        self.0.domains()
    }
    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        self.0.create(opts)
    }
    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        Ok(self
            .0
            .list(inbox)?
            .into_iter()
            .map(decoded_summary)
            .collect())
    }
    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        self.0.get(inbox, id).map(decoded_message)
    }
    fn health(&self) -> Result<()> {
        self.0.health()
    }
    fn raw(&self, inbox: &mut Inbox, id: &str) -> Result<Vec<u8>> {
        self.0.raw(inbox, id)
    }
    fn attachment(&self, inbox: &mut Inbox, msg: &Message, att: &Attachment) -> Result<Vec<u8>> {
        self.0.attachment(inbox, msg, att)
    }
    fn delete_message(&self, inbox: &mut Inbox, id: &str) -> Result<()> {
        self.0.delete_message(inbox, id)
    }
    fn delete_inbox(&self, inbox: &mut Inbox) -> Result<()> {
        self.0.delete_inbox(inbox)
    }
    fn login(&self, address: &str, password: &str) -> Result<Inbox> {
        self.0.login(address, password)
    }
    fn push(&self, inbox: &Inbox) -> Option<Box<dyn PushStream>> {
        self.0.push(inbox)
    }
    fn owns_address(&self, address: &str) -> bool {
        self.0.owns_address(address)
    }
}

/// `TMPMAIL_<ID>_URL`, else the provider's real endpoint.
pub fn base_url(id: &str, default: &str) -> String {
    let env = format!("TMPMAIL_{}_URL", id.to_uppercase());
    std::env::var(env)
        .unwrap_or_else(|_| default.to_string())
        .trim_end_matches('/')
        .to_string()
}

pub fn domain_unavailable(provider: &str, domain: &str, available: &[String]) -> Error {
    Error::DomainUnavailable {
        provider: provider.to_string(),
        domain: domain.to_string(),
        available: available.to_vec(),
    }
}

/// Calls with the inbox's stored token; if the provider rejects it, gets a
/// fresh one from `refresh` (which stores it in the inbox) and tries once more.
pub fn with_token<T>(
    inbox: &mut Inbox,
    refresh: impl FnOnce(&mut Inbox) -> Result<String>,
    call: impl Fn(&str) -> Result<T>,
) -> Result<T> {
    if let Some(token) = inbox.cred_str("token") {
        match call(token) {
            Err(Error::Auth { .. }) => {}
            other => return other,
        }
    }
    let token = refresh(inbox)?;
    call(&token)
}

/// Parses timestamps as providers send them: RFC 3339 strings or unix seconds.
pub fn parse_date(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::String(s) => DateTime::parse_from_rfc3339(s)
            .or_else(|_| DateTime::parse_from_rfc2822(s))
            .map(|d| d.with_timezone(&Utc))
            .ok()
            .or_else(|| s.parse::<i64>().ok().and_then(from_unix)),
        Value::Number(n) => n.as_i64().and_then(from_unix),
        _ => None,
    }
}

fn from_unix(n: i64) -> Option<DateTime<Utc>> {
    // Values this large are milliseconds.
    let secs = if n > 100_000_000_000 { n / 1000 } else { n };
    if secs <= 0 {
        return None;
    }
    DateTime::from_timestamp(secs, 0)
}

pub fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// A string field, with empty treated as absent.
pub fn opt_str(v: &Value, key: &str) -> Option<String> {
    Some(str_field(v, key))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Stable short id for providers whose messages have none (FNV-1a).
pub fn synthetic_id(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{:012x}", h & 0xffff_ffff_ffff)
}

/// Readable random local part, e.g. `quietotter482`.
pub fn random_user() -> String {
    use rand::seq::IndexedRandom;
    const ADJ: &[&str] = &[
        "amber", "brave", "calm", "clever", "cosmic", "crisp", "dusty", "eager", "fancy", "fuzzy",
        "gentle", "golden", "happy", "hidden", "icy", "jolly", "lucky", "mellow", "misty", "noble",
        "odd", "plain", "quick", "quiet", "rapid", "rusty", "shy", "silent", "silver", "sleepy",
        "snowy", "solar", "steady", "sunny", "swift", "tidy", "urban", "vivid", "witty", "zesty",
    ];
    const NOUN: &[&str] = &[
        "badger", "beacon", "bison", "cactus", "comet", "coral", "falcon", "fern", "fox", "gecko",
        "harbor", "heron", "island", "koala", "lantern", "lemur", "lynx", "maple", "meadow",
        "moose", "nebula", "otter", "owl", "panda", "pebble", "pine", "quartz", "raven", "river",
        "robin", "salmon", "sparrow", "summit", "tiger", "tulip", "walrus", "willow", "wombat",
        "yak", "zebra",
    ];
    let mut rng = rand::rng();
    format!(
        "{}{}{}",
        ADJ.choose(&mut rng).unwrap(),
        NOUN.choose(&mut rng).unwrap(),
        rand::random_range(100..1000)
    )
}

pub fn random_password() -> String {
    use rand::distr::{Alphanumeric, SampleString};
    Alphanumeric.sample_string(&mut rand::rng(), 24)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_dates_in_all_provider_formats() {
        let iso = parse_date(&json!("2026-10-05T10:08:30+00:00")).unwrap();
        assert_eq!(iso.timestamp(), 1791194910);
        assert_eq!(parse_date(&json!(1791194910)).unwrap(), iso);
        assert_eq!(parse_date(&json!("1791194910")).unwrap(), iso);
        assert_eq!(parse_date(&json!(1791194910000i64)).unwrap(), iso);
        assert_eq!(parse_date(&json!(0)), None);
        assert_eq!(parse_date(&json!(null)), None);
    }

    #[test]
    fn synthetic_ids_are_stable_and_distinct() {
        assert_eq!(synthetic_id(&["a", "b"]), synthetic_id(&["a", "b"]));
        assert_ne!(synthetic_id(&["ab", ""]), synthetic_id(&["a", "b"]));
        assert_eq!(synthetic_id(&["x"]).len(), 12);
    }

    #[test]
    fn random_user_is_a_valid_local_part() {
        let u = random_user();
        assert!(
            u.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "{u}"
        );
    }

    #[test]
    fn headers_come_out_decoded() {
        let mut m = Message {
            from: "=?utf-8?q?Zo=C3=AB_=C3=9Cnicode?= <hello@example.org>".into(),
            to: vec!["=?utf-8?q?A?= <a@x.com>".into()],
            subject: "=?utf-8?b?Q2Fmw6kgcsOpc3Vtw6kg4pyT?= code".into(),
            text: Some("Grüße, code 551177\r\n".into()),
            ..Default::default()
        };
        m.attachments.push(Attachment {
            filename: "=?utf-8?q?r=C3=A9sum=C3=A9.pdf?=".into(),
            ..Default::default()
        });
        let s = decoded_summary(m.summary());
        assert_eq!(s.from, "Zoë Ünicode <hello@example.org>");
        assert_eq!(s.subject, "Café résumé ✓ code");
        assert_eq!(s.intro, "Grüße, code 551177");
        let m = decoded_message(m);
        assert_eq!(m.subject, "Café résumé ✓ code");
        assert_eq!(m.to, ["A <a@x.com>"]);
        assert_eq!(m.attachments[0].filename, "résumé.pdf");
    }

    #[test]
    fn every_listed_provider_resolves() {
        for id in all() {
            assert_eq!(get(id).unwrap().id(), id);
        }
        assert!(get("1secmail").is_err());
    }
}
