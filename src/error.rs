use std::fmt;

use thiserror::Error;

/// Every failure the CLI can report. Each variant maps to a stable exit code
/// so scripts can tell "no mail yet" apart from "provider is down".
#[derive(Debug, Error)]
pub enum Error {
    #[error("{provider}: unreachable ({detail})")]
    Unreachable { provider: String, detail: String },

    #[error("{provider}: rate limited, try again later")]
    RateLimited { provider: String },

    #[error("{provider}: authentication failed ({detail})")]
    Auth { provider: String, detail: String },

    /// `status` is the HTTP status when the failure came from one.
    #[error("{provider}: unexpected response ({detail})")]
    BadResponse {
        provider: String,
        detail: String,
        status: Option<u16>,
    },

    #[error("{0}")]
    NotFound(String),

    #[error("{provider}: {what} is not supported by this provider")]
    Unsupported { provider: String, what: String },

    #[error("{0}")]
    AddressTaken(String),

    #[error("{provider}: domain {domain:?} not available, choose one of: {}", available.join(", "))]
    DomainUnavailable {
        provider: String,
        domain: String,
        available: Vec<String>,
    },

    #[error("inbox {0} has expired")]
    Expired(String),

    #[error("no inbox yet, create one with `tmpmail new`")]
    NoInbox,

    #[error("timed out after {0}s without a matching message")]
    Timeout(u64),

    #[error("cancelled")]
    Cancelled,

    #[error("{0}")]
    Usage(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Timeout(_) => 2,
            Error::Unreachable { .. }
            | Error::RateLimited { .. }
            | Error::BadResponse { .. }
            | Error::Auth { .. } => 3,
            Error::NotFound(_) | Error::NoInbox | Error::Expired(_) => 4,
            Error::Usage(_)
            | Error::Unsupported { .. }
            | Error::AddressTaken(_)
            | Error::DomainUnavailable { .. } => 64,
            Error::Cancelled => 130,
            Error::Other(_) => self.noted().map_or(1, Error::exit_code),
        }
    }

    /// Errors that justify trying the next provider during `new`.
    pub fn is_provider_failure(&self) -> bool {
        matches!(
            self,
            Error::Unreachable { .. }
                | Error::RateLimited { .. }
                | Error::BadResponse { .. }
                | Error::Auth { .. }
        )
    }

    /// This error followed by `; NOTE`, keeping its kind and exit code:
    /// `mailgw: unreachable (…); the other providers don't offer …`.
    pub(crate) fn with_note(self, note: String) -> Error {
        Error::Other(anyhow::Error::new(Noted { error: self, note }))
    }

    /// The error a [`Error::with_note`] wraps.
    fn noted(&self) -> Option<&Error> {
        match self {
            Error::Other(e) => e.downcast_ref::<Noted>().map(|n| &n.error),
            _ => None,
        }
    }

    /// Short machine-readable kind, used in `--json` error output.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Unreachable { .. } => "unreachable",
            Error::RateLimited { .. } => "rate_limited",
            Error::Auth { .. } => "auth",
            Error::BadResponse { .. } => "bad_response",
            Error::NotFound(_) => "not_found",
            Error::Unsupported { .. } => "unsupported",
            Error::AddressTaken(_) => "address_taken",
            Error::Expired(_) => "expired",
            Error::NoInbox => "no_inbox",
            Error::Timeout(_) => "timeout",
            Error::Cancelled => "cancelled",
            Error::Usage(_) | Error::DomainUnavailable { .. } => "usage",
            Error::Other(_) => self.noted().map_or("error", Error::kind),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An error with a note after it. It has no `source`, so `{:#}` prints it
/// once.
#[derive(Debug)]
struct Noted {
    error: Error,
    note: String,
}

impl fmt::Display for Noted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.error {
            // Keep the cause chain, as the report of an `Other` does.
            Error::Other(e) => write!(f, "{e:#}; {}", self.note),
            e => write!(f, "{e}; {}", self.note),
        }
    }
}

impl std::error::Error for Noted {}

pub fn unsupported(provider: &str, what: &str) -> Error {
    Error::Unsupported {
        provider: provider.to_string(),
        what: what.to_string(),
    }
}

pub fn message_not_found(id: &str) -> Error {
    Error::NotFound(format!("message {id} not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_goes_after_the_error_and_keeps_its_kind() {
        let e = Error::Unreachable {
            provider: "p".into(),
            detail: "down".into(),
        }
        .with_note("NOTE".into());
        assert_eq!(e.to_string(), "p: unreachable (down); NOTE");
        let Error::Other(inner) = &e else {
            panic!("{e:?}")
        };
        // As main reports it: once, not once more as a cause.
        assert_eq!(format!("{inner:#}"), "p: unreachable (down); NOTE");
        assert_eq!((e.kind(), e.exit_code()), ("unreachable", 3));
        let plain = Error::Other(anyhow::anyhow!("x"));
        assert_eq!((plain.kind(), plain.exit_code()), ("error", 1));
        let chained =
            Error::Other(anyhow::anyhow!("cause").context("outer")).with_note("NOTE".into());
        assert_eq!(chained.to_string(), "outer: cause; NOTE");
    }
}
