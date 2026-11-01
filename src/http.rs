use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread::sleep;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::render::clean_line;

const USER_AGENT: &str = concat!("tmpmail/", env!("CARGO_PKG_VERSION"));
const MAX_RETRIES: u32 = 3;

/// The config's `http_timeout`, 0 while it isn't set.
static CONFIGURED_TIMEOUT: AtomicU64 = AtomicU64::new(0);

/// Sets the request timeout from the config (`TMPMAIL_HTTP_TIMEOUT` still
/// wins). Requests made after this use it.
pub(crate) fn set_timeout(secs: Option<u64>) {
    CONFIGURED_TIMEOUT.store(secs.unwrap_or(0), Ordering::Relaxed);
}

/// Seconds per request: the environment, else the config, else 20.
fn timeout_secs(env: Option<&str>, configured: u64) -> u64 {
    env.and_then(|v| v.parse().ok())
        .filter(|&secs| secs > 0)
        .or((configured > 0).then_some(configured))
        .unwrap_or(crate::config::DEFAULT_HTTP_TIMEOUT)
}

/// One client for the whole process, so polling reuses pooled connections
/// instead of paying a TLS handshake per request. It is built again only if
/// the timeout changes (the desktop app's settings).
fn shared_client() -> Client {
    static CLIENT: Mutex<Option<(Duration, Client)>> = Mutex::new(None);
    let env = std::env::var("TMPMAIL_HTTP_TIMEOUT").ok();
    let secs = timeout_secs(env.as_deref(), CONFIGURED_TIMEOUT.load(Ordering::Relaxed));
    cached(&CLIENT, Duration::from_secs(secs))
}

/// The client in `cache`, built again when `timeout` differs from the one
/// it was built with. Every client has the same settings but the timeout.
fn cached(cache: &Mutex<Option<(Duration, Client)>>, timeout: Duration) -> Client {
    let mut client = cache.lock().unwrap_or_else(PoisonError::into_inner);
    match &*client {
        Some((t, c)) if *t == timeout => c.clone(),
        _ => {
            let c = Client::builder()
                .user_agent(USER_AGENT)
                .timeout(timeout)
                .connect_timeout(Duration::from_secs(10))
                .https_only(!cfg!(test) && std::env::var_os("TMPMAIL_ALLOW_HTTP").is_none())
                .build()
                .expect("HTTP client configuration is valid");
            *client = Some((timeout, c.clone()));
            c
        }
    }
}

/// A client for responses the server keeps open (mail.tm's push stream).
/// Its timeout bounds each read, not the whole response: a request's own
/// `timeout` would end the stream that long after it started, however busy
/// it was.
fn stream_client(idle: Duration) -> Client {
    static CLIENT: Mutex<Option<(Duration, Client)>> = Mutex::new(None);
    cached(&CLIENT, idle)
}

/// Thin wrapper around a blocking client that turns every failure into a typed
/// [`Error`] naming the provider, so a dead backend never surfaces as garbage.
#[derive(Clone)]
pub struct Http {
    provider: &'static str,
}

impl Http {
    pub fn new(provider: &'static str) -> Self {
        Http { provider }
    }

    /// The shared client (a cheap handle to the same connection pool).
    pub fn client(&self) -> Client {
        shared_client()
    }

    /// A client for a stream that may stay open for hours, with no read
    /// taking longer than `idle` (see [`stream_client`]).
    pub fn stream_client(&self, idle: Duration) -> Client {
        stream_client(idle)
    }

    /// Sends a request, retrying on 429 (honouring Retry-After) and mapping
    /// transport errors and non-2xx statuses to typed errors.
    pub fn send(&self, build: impl Fn() -> RequestBuilder) -> Result<Response> {
        let mut attempt = 0;
        loop {
            let resp = build().send().map_err(|e| self.transport_error(e))?;
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                attempt += 1;
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1 << attempt)
                    .min(30);
                sleep(Duration::from_secs(wait));
                continue;
            }
            if status.is_success() {
                return Ok(resp);
            }
            // Only the start is quoted; the rest isn't read.
            let body = read_capped(resp, MAX_ERROR_BODY, 0)
                .map(|(b, _)| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            return Err(self.status_error(status, &body));
        }
    }

    pub fn json<T: DeserializeOwned>(&self, build: impl Fn() -> RequestBuilder) -> Result<T> {
        let resp = self.send(build)?;
        let body = self.capped(resp, MAX_JSON)?;
        let body = String::from_utf8_lossy(&body);
        self.parse(body.strip_prefix('\u{FEFF}').unwrap_or(&body))
    }

    /// A raw message or an attachment, up to [`MAX_BYTES`].
    pub fn bytes(&self, build: impl Fn() -> RequestBuilder) -> Result<Vec<u8>> {
        let resp = self.send(build)?;
        self.capped(resp, MAX_BYTES)
    }

    /// The body, or an error once it passes `limit` bytes (the rest isn't
    /// read): a provider (or anyone between) can't make tmpmail hold
    /// gigabytes.
    fn capped(&self, resp: Response, limit: usize) -> Result<Vec<u8>> {
        let hint = resp.content_length().unwrap_or(0);
        match read_capped(resp, limit, hint) {
            Ok((body, false)) => Ok(body),
            Ok((_, true)) => Err(self.bad(format!(
                "the response is larger than {} MiB, more than any mail needs; not read",
                limit >> 20
            ))),
            Err(e) => Err(Error::Unreachable {
                provider: self.provider.to_string(),
                detail: if timed_out(&e) {
                    "request timed out".to_string()
                } else {
                    root_cause(&e)
                },
            }),
        }
    }

    /// POSTs a GraphQL query and returns its `data`, turning a non-empty
    /// `errors` array into a typed error.
    pub fn graphql(&self, url: &str, query: &str, variables: &Value) -> Result<Value> {
        self.graphql_within(url, query, variables, None)
    }

    /// [`Http::graphql`] with its own timeout, for calls the server holds open.
    pub fn graphql_within(
        &self,
        url: &str,
        query: &str,
        variables: &Value,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let body = json!({ "query": query, "variables": variables });
        let client = self.client();
        let mut v: Value = self.json(|| {
            let req = client.post(url).json(&body);
            match timeout {
                Some(t) => req.timeout(t),
                None => req,
            }
        })?;
        if let Some(errors) = v
            .get("errors")
            .and_then(Value::as_array)
            .filter(|e| !e.is_empty())
        {
            let msg = errors
                .iter()
                .map(|e| e.get("message").and_then(Value::as_str).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("; ");
            if msg.contains("RATE_LIMIT") {
                return Err(Error::RateLimited {
                    provider: self.provider.to_string(),
                });
            }
            return Err(self.bad(format!("GraphQL error: {msg}")));
        }
        Ok(v.get_mut("data").map(Value::take).unwrap_or_default())
    }

    fn parse<T: DeserializeOwned>(&self, body: &str) -> Result<T> {
        serde_json::from_str(body).map_err(|e| {
            let trimmed = body.trim_start();
            let detail = if trimmed.is_empty() {
                "empty body".to_string()
            } else if trimmed.starts_with('<') {
                "got an HTML page instead of JSON, the API may be down or blocked".to_string()
            } else {
                format!("invalid JSON: {e}; body starts with {:?}", snippet(body))
            };
            self.bad(detail)
        })
    }

    pub fn bad(&self, detail: impl Into<String>) -> Error {
        Error::BadResponse {
            provider: self.provider.to_string(),
            // Often quotes the provider's own words: keep escapes out.
            detail: clean_line(&detail.into()),
            status: None,
        }
    }

    pub fn transport_error(&self, e: reqwest::Error) -> Error {
        let detail = if e.is_timeout() {
            "request timed out".to_string()
        } else if e.is_connect() {
            format!("connection failed: {}", root_cause(&e))
        } else {
            root_cause(&e)
        };
        Error::Unreachable {
            provider: self.provider.to_string(),
            detail,
        }
    }

    fn status_error(&self, status: StatusCode, body: &str) -> Error {
        let provider = self.provider.to_string();
        let code = status.as_u16();
        let detail = format!("HTTP {code}{}", describe_body(body));
        match code {
            401 | 403 => Error::Auth { provider, detail },
            404 => Error::NotFound(format!("{provider}: not found ({detail})")),
            429 => Error::RateLimited { provider },
            500..=599 => Error::Unreachable { provider, detail },
            _ => Error::BadResponse {
                provider,
                detail,
                status: Some(code),
            },
        }
    }
}

/// The most a JSON answer may be: a listing or a message is far smaller,
/// but mail.tm's and Maildrop's raw sources come as JSON, so it holds one
/// of [`MAX_BYTES`] with room for its escaping.
const MAX_JSON: usize = 80 << 20;

/// The most a raw message or an attachment may be: above the providers'
/// own message limits (mail.tm and the big mail services take 25-50 MB).
const MAX_BYTES: usize = 64 << 20;

/// The most of an error answer read, to quote its start.
const MAX_ERROR_BODY: usize = 64 << 10;

/// Reads at most `limit` bytes of `r` (`hint` long, if known), and whether
/// there was more.
fn read_capped(r: impl std::io::Read, limit: usize, hint: u64) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let mut body = Vec::with_capacity(usize::try_from(hint).unwrap_or(0).min(limit + 1));
    r.take(limit as u64 + 1).read_to_end(&mut body)?;
    let more = body.len() > limit;
    body.truncate(limit);
    Ok((body, more))
}

/// Whether a read of a body failed for the request's timeout. reqwest's
/// blocking reader reports it as an error of kind `Other` wrapping its own
/// timeout error, and `io::Error::source` skips the wrapped error itself, so
/// it is reached through `get_ref` and its chain read from there.
fn timed_out(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::TimedOut {
        return true;
    }
    let mut cur = e
        .get_ref()
        .map(|inner| inner as &(dyn std::error::Error + 'static));
    while let Some(err) = cur {
        if err
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::TimedOut)
            || err
                .downcast_ref::<reqwest::Error>()
                .is_some_and(reqwest::Error::is_timeout)
        {
            return true;
        }
        cur = err.source();
    }
    false
}

fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

fn describe_body(body: &str) -> String {
    let t = body.trim();
    if t.is_empty() || t.starts_with('<') {
        return String::new();
    }
    // Hydra/JSON APIs put a human message in one of these fields.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        for key in ["hydra:description", "detail", "message", "error"] {
            if let Some(s) = v.get(key).and_then(|s| s.as_str()) {
                return format!(": {}", clean_line(s));
            }
        }
    }
    format!(": {}", snippet(t))
}

fn snippet(s: &str) -> String {
    clean_line(&s.chars().take(120).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    /// A server that answers one request with a line every 250 ms for
    /// `lines` lines. Returns its URL.
    fn trickle(lines: usize) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut req = BufReader::new(conn.try_clone().unwrap());
            let mut line = String::new();
            while req.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let _ = conn.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
            );
            for _ in 0..lines {
                if conn.write_all(b":\n").and_then(|()| conn.flush()).is_err() {
                    return;
                }
                sleep(Duration::from_millis(250));
            }
        });
        url
    }

    #[test]
    fn a_stream_outlives_its_idle_bound_while_data_flows() {
        // A client of its own, so the process-wide stream client stays as
        // it is for other tests.
        let idle = Duration::from_secs(1);
        let cache = Mutex::new(None);
        let resp = cached(&cache, idle).get(trickle(12)).send().unwrap();
        // 12 lines over ~3 s, three times the idle bound.
        assert_eq!(BufReader::new(resp).lines().map(Result::unwrap).count(), 12);

        // A request's own timeout cuts the same stream off (what mail.tm's
        // push stream did every 75 s).
        let resp = cached(&cache, idle)
            .get(trickle(12))
            .timeout(idle)
            .send()
            .unwrap();
        assert!(BufReader::new(resp).lines().any(|l| l.is_err()));
    }

    #[test]
    fn the_environment_wins_over_the_config_timeout() {
        assert_eq!(timeout_secs(None, 0), 20);
        assert_eq!(timeout_secs(None, 45), 45);
        assert_eq!(timeout_secs(Some("7"), 45), 7);
        // `Config::load` refuses these; a lib user that skips it gets the
        // config's value, not a timeout of 0.
        assert_eq!(timeout_secs(Some("soon"), 45), 45);
        assert_eq!(timeout_secs(Some("soon"), 0), 20);
        assert_eq!(timeout_secs(Some("0"), 45), 45);
    }

    #[test]
    fn a_body_that_stalls_is_a_timeout() {
        // Review of the security round: reqwest reports a body read's
        // timeout as `Other`, so it was shown as its inner message.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut req = BufReader::new(conn.try_clone().unwrap());
            let mut line = String::new();
            while req.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let _ = conn.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n{\"a\":");
            let _ = conn.flush();
            // Stalls past the client's timeout, then goes.
            sleep(Duration::from_secs(3));
        });
        let cache = Mutex::new(None);
        let resp = cached(&cache, Duration::from_secs(1))
            .get(url)
            .send()
            .unwrap();
        match Http::new("test").capped(resp, 1000) {
            Err(Error::Unreachable { detail, .. }) => assert_eq!(detail, "request timed out"),
            other => panic!("{other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn bodies_are_read_up_to_a_limit() {
        // Security review A5: no size limit on provider responses.
        let (body, more) = read_capped(std::io::repeat(b'x'), 1000, 0).unwrap();
        assert_eq!((body.len(), more), (1000, true));
        let (body, more) = read_capped(&b"small"[..], 1000, 5).unwrap();
        assert_eq!((body.as_slice(), more), (&b"small"[..], false));
        let (_, more) = read_capped(&[0u8; 1000][..], 1000, u64::MAX).unwrap();
        assert!(!more);
    }
}
