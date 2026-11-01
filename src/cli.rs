use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

const AFTER_HELP: &str = "\
Providers: mail.tm (default), DropMail.me, Guerrilla Mail, tempmail.lol, mail.gw,
and Maildrop (public inboxes, opt-in). `new` falls back to the next provider when
one is down. Thanks to mail.tm (https://mail.tm) for its free API.

Exit codes: 0 ok, 1 error, 2 wait timed out, 3 provider failure,
4 nothing found (no inbox, message or code), 64 usage error.";

#[derive(Debug, Parser)]
#[command(
    name = "tmpmail",
    version,
    about = "Temporary email from the terminal",
    flatten_help = true,
    after_help = AFTER_HELP
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,

    #[command(subcommand)]
    pub command: Command,
}

pub use crate::app::Global;

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a new address and make it active
    #[command(visible_alias = "generate")]
    New(NewArgs),

    /// List messages in the inbox (or all inboxes with -a)
    #[command(visible_alias = "inbox")]
    Ls(LsArgs),

    /// Show a message (by id, list number, or --recent)
    #[command(visible_alias = "show")]
    Read(ReadArgs),

    /// Wait for a matching message to arrive
    Wait(WaitArgs),

    /// Extract a verification code (OTP) from a message
    Otp(OtpArgs),

    /// Extract links from a message, verification links first
    Links(LinksArgs),

    /// Stream new messages as they arrive
    Watch(WatchArgs),

    /// List a message's attachments
    Attachments(MessageRef),

    /// Download attachments
    Save(SaveArgs),

    /// Delete one message or all of them
    Delete(DeleteArgs),

    /// Print the active address
    Whoami,

    /// Copy the active address to the clipboard
    Copy,

    /// Show the active address as a QR code
    Qr,

    /// List available domains
    Domains(ProviderArg),

    /// List stored inboxes
    Inboxes(InboxesArgs),

    /// Switch the active inbox
    Use {
        /// Address, local part or label of a stored inbox
        address: String,
    },

    /// Attach an existing account (mail.tm / mail.gw, or a Maildrop name)
    Login(LoginArgs),

    /// Delete the inbox at the provider and forget it locally
    Drop(DropArgs),

    /// Replace the active address with a fresh one
    Rotate(RotateArgs),

    /// List providers, their capabilities and (with --check) their status
    Providers(ProvidersArgs),

    /// Browse inboxes and mail in a full-screen terminal UI
    Tui,

    /// Run an MCP server on stdin/stdout for AI agents
    Mcp,

    /// Print shell completions
    Completions { shell: Shell },

    /// Print the man page, or write one page per command with --output
    Man {
        /// Directory to write tmpmail.1 and tmpmail-COMMAND.1 into
        #[arg(long, short)]
        output: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// Username (local part); random and readable by default
    #[arg(long, short)]
    pub user: Option<String>,

    /// Domain, see `tmpmail domains`
    #[arg(long, short)]
    pub domain: Option<String>,

    /// Use only this provider (no fallback)
    #[arg(long, short)]
    pub provider: Option<String>,

    /// Label to find this inbox later (e.g. github)
    #[arg(long, short)]
    pub label: Option<String>,

    /// Account password (mail.tm / mail.gw); random by default
    #[arg(long, env = "TMPMAIL_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// Copy the new address to the clipboard
    #[arg(long, short)]
    pub copy: bool,

    /// Full address, e.g. otter42@maxxspace.com (sets user and domain)
    #[arg(conflicts_with_all = ["user", "domain"])]
    pub address: Option<String>,
}

#[derive(Debug, Args)]
pub struct MessageRef {
    /// Message id or its number in `tmpmail ls`; newest when omitted
    pub id: Option<String>,

    /// Use the newest message
    #[arg(long, short, conflicts_with = "id")]
    pub recent: bool,
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    #[command(flatten)]
    pub msg: MessageRef,

    /// Open the sanitized HTML in a browser
    #[arg(long, conflicts_with = "raw")]
    pub html: bool,

    /// Load remote images in --html (off by default: they are tracking pixels)
    #[arg(long, requires = "html")]
    pub images: bool,

    /// Print the raw RFC 822 source
    #[arg(long)]
    pub raw: bool,

    /// Print long messages straight to the terminal instead of a pager
    #[arg(long)]
    pub no_pager: bool,

    /// Text only (the default; kept for tmpmail compatibility)
    #[arg(long, short, hide = true)]
    pub text: bool,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    /// List every stored inbox, not just the active one
    #[arg(long, short = 'a')]
    pub all_inboxes: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct WaitFilter {
    /// Watch every stored inbox; the first match in any of them wins
    #[arg(long, short = 'a')]
    pub all_inboxes: bool,

    /// Regex the sender must match (case-insensitive)
    #[arg(long)]
    pub from: Option<String>,

    /// Regex the subject must match (case-insensitive)
    #[arg(long)]
    pub subject: Option<String>,

    /// Regex the body must match (case-insensitive)
    #[arg(long)]
    pub body: Option<String>,

    /// Give up after this many seconds [default: 180]
    #[arg(long, short)]
    pub timeout: Option<u64>,

    /// Seconds between checks [default: 5]
    #[arg(long)]
    pub interval: Option<u64>,

    /// Also consider messages that were already there (the newest match wins)
    #[arg(long)]
    pub include_existing: bool,
}

#[derive(Debug, Args)]
pub struct WaitArgs {
    #[command(flatten)]
    pub filter: WaitFilter,

    /// Print only the verification code (waits for a message that has one)
    #[arg(long, conflicts_with = "link")]
    pub otp: bool,

    /// Print only the best link
    #[arg(long)]
    pub link: bool,

    /// Custom regex for --otp; first capture group is the code
    #[arg(long, requires = "otp")]
    pub otp_regex: Option<String>,

    /// Delete the message once it has been printed
    #[arg(long)]
    pub delete: bool,
}

#[derive(Debug, Args)]
pub struct OtpArgs {
    #[command(flatten)]
    pub msg: MessageRef,

    /// Custom regex; its first capture group is the code
    #[arg(long)]
    pub regex: Option<String>,

    /// Wait for a new message containing a code instead of reading one
    #[arg(long, short, conflicts_with_all = ["id", "recent"])]
    pub wait: bool,

    #[command(flatten)]
    pub filter: WaitFilter,
}

#[derive(Debug, Args)]
pub struct LinksArgs {
    #[command(flatten)]
    pub msg: MessageRef,

    /// Only links matching this regex
    #[arg(long, short)]
    pub r#match: Option<String>,

    /// Print only the best link
    #[arg(long)]
    pub first: bool,

    /// Open the best link in the browser
    #[arg(long)]
    pub open: bool,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// Watch every stored inbox, not just the active one
    #[arg(long, short = 'a')]
    pub all_inboxes: bool,

    /// Desktop notification for each new message
    #[arg(long, short)]
    pub notify: bool,

    /// Seconds between checks [default: 5]
    #[arg(long)]
    pub interval: Option<u64>,

    /// Switch to a fresh address this often, e.g. 30m, 2h (at least 1m)
    #[arg(long, value_parser = parse_rotation, conflicts_with = "all_inboxes")]
    pub rotate: Option<std::time::Duration>,

    /// With --rotate: keep old inboxes instead of dropping them
    #[arg(long, requires = "rotate")]
    pub keep: bool,

    /// With --rotate: copy each new address to the clipboard
    #[arg(long, short, requires = "rotate")]
    pub copy: bool,
}

/// Parses `90`, `90s`, `30m`, `2h`, `1d` or combinations such as `1h30m`.
pub fn parse_duration(s: &str) -> Result<std::time::Duration, String> {
    let bad = || format!("invalid duration {s:?}, use e.g. 90s, 30m or 2h");
    let s = s.trim();
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(std::time::Duration::from_secs(secs));
    }
    let (mut total, mut digits) = (0u64, String::new());
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86400,
            _ => return Err(bad()),
        };
        let n: u64 = digits.parse().map_err(|_| bad())?;
        total = n
            .checked_mul(unit)
            .and_then(|v| total.checked_add(v))
            .ok_or_else(bad)?;
        digits.clear();
    }
    if !digits.is_empty() || s.is_empty() {
        return Err(bad());
    }
    Ok(std::time::Duration::from_secs(total))
}

/// A rotation period: rotating faster than once a minute only burns
/// through provider rate limits.
fn parse_rotation(s: &str) -> Result<std::time::Duration, String> {
    let d = parse_duration(s)?;
    if d.as_secs() < 60 {
        return Err("rotate at most once a minute (e.g. 1m, 30m, 2h)".into());
    }
    Ok(d)
}

#[derive(Debug, Args)]
pub struct SaveArgs {
    /// Message id or list number
    pub id: String,

    /// Attachment ids or file names (all when omitted)
    pub attachments: Vec<String>,

    /// Target directory
    #[arg(long, short, default_value = ".")]
    pub output: std::path::PathBuf,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Message id or list number
    #[arg(required_unless_present = "all")]
    pub id: Option<String>,

    /// Delete every message in the inbox
    #[arg(long, conflicts_with = "id")]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct ProviderArg {
    /// Provider to ask (default: first in your provider order)
    #[arg(long, short)]
    pub provider: Option<String>,
}

#[derive(Debug, Args)]
pub struct InboxesArgs {
    /// Forget inboxes that have expired
    #[arg(long)]
    pub prune: bool,
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Full address of the existing account
    pub address: String,

    /// Account password (or TMPMAIL_PASSWORD)
    #[arg(long, env = "TMPMAIL_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// mailtm (default), mailgw or maildrop
    #[arg(long, short)]
    pub provider: Option<String>,

    /// Label to find this inbox later (e.g. github)
    #[arg(long, short)]
    pub label: Option<String>,
}

#[derive(Debug, Args)]
pub struct DropArgs {
    /// Inbox to drop (default: the active one)
    pub address: Option<String>,

    /// Only forget it locally; leave it at the provider
    #[arg(long)]
    pub local: bool,
}

#[derive(Debug, Args)]
pub struct RotateArgs {
    /// Keep the old inbox instead of dropping it
    #[arg(long)]
    pub keep: bool,

    /// Copy the new address to the clipboard
    #[arg(long, short)]
    pub copy: bool,
}

#[derive(Debug, Args)]
pub struct ProvidersArgs {
    /// Check whether each provider is reachable right now
    #[arg(long)]
    pub check: bool,
}

/// Whether `name` is a subcommand or one of its aliases.
fn is_subcommand(name: &str) -> bool {
    use clap::CommandFactory;
    name == "help"
        || Cli::command()
            .get_subcommands()
            .any(|c| c.get_name() == name || c.get_all_aliases().any(|a| a == name))
}

/// Global options that take a value, so their value is not mistaken for a
/// command when scanning legacy arguments.
const VALUE_GLOBALS: &[&str] = &["-i", "--inbox", "-b", "--browser", "--clipboard-cmd"];

/// Translates the original tmpmail interface (`tmpmail`, `tmpmail -g [ADDR]`,
/// `tmpmail -r`, `tmpmail ID`, ...) into subcommands, leaving everything else
/// untouched.
pub fn rewrite_legacy(args: Vec<String>) -> Vec<String> {
    let mut out = vec![args.first().cloned().unwrap_or_else(|| "tmpmail".into())];
    let mut rest = args.into_iter().skip(1).peekable();
    let mut globals = Vec::new();
    let mut command: Option<Vec<String>> = None;

    while let Some(arg) = rest.next() {
        if let Some(cmd) = &mut command {
            cmd.push(arg);
            continue;
        }
        let a = arg.as_str();
        if VALUE_GLOBALS.contains(&a) {
            globals.push(arg);
            if let Some(v) = rest.next() {
                globals.push(v);
            }
            continue;
        }
        if a.starts_with("--") && a.contains('=') {
            globals.push(arg);
            continue;
        }
        command = Some(match a {
            "-g" | "--generate" => {
                let mut c = vec!["new".to_string()];
                if let Some(addr) = rest.next_if(|n| !n.starts_with('-')) {
                    c.push(addr);
                }
                c
            }
            "-r" | "--recent" => vec!["read".into(), "--recent".into()],
            "-d" | "--domains" => vec!["domains".into()],
            "-c" | "--copy" => vec!["copy".into()],
            "-t" | "--text" => vec!["read".into()],
            "--json" | "--no-color" | "-q" | "--quiet" => {
                globals.push(arg);
                continue;
            }
            _ if a.starts_with('-') => vec![arg],
            _ if is_subcommand(a) => vec![arg],
            _ => vec!["read".into(), arg],
        });
    }

    let command = command.unwrap_or_else(|| vec!["ls".into()]);
    out.extend(globals);
    out.extend(command);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw(s: &str) -> String {
        let args = std::iter::once("tmpmail")
            .chain(s.split_whitespace())
            .map(String::from)
            .collect();
        rewrite_legacy(args)[1..].join(" ")
    }

    #[test]
    fn legacy_flags_map_to_subcommands() {
        assert_eq!(rw(""), "ls");
        assert_eq!(rw("-g"), "new");
        assert_eq!(
            rw("--generate otter@maxxspace.com"),
            "new otter@maxxspace.com"
        );
        assert_eq!(rw("-g --copy"), "new --copy");
        assert_eq!(rw("-r"), "read --recent");
        assert_eq!(rw("-t 42"), "read 42");
        assert_eq!(rw("42"), "read 42");
        assert_eq!(rw("-d"), "domains");
        assert_eq!(rw("-c"), "copy");
        assert_eq!(rw("-b w3m -r"), "-b w3m read --recent");
        assert_eq!(rw("--json"), "--json ls");
    }

    #[test]
    fn modern_usage_is_untouched() {
        assert_eq!(rw("new --provider dropmail"), "new --provider dropmail");
        assert_eq!(rw("--json wait --otp"), "--json wait --otp");
        assert_eq!(rw("-i github ls"), "-i github ls");
        assert_eq!(rw("--help"), "--help");
        assert_eq!(rw("--version"), "--version");
        assert_eq!(rw("read 3 --json"), "read 3 --json");
    }

    #[test]
    fn durations() {
        let secs = |s| parse_duration(s).map(|d| d.as_secs());
        assert_eq!(secs("90"), Ok(90));
        assert_eq!(secs("90s"), Ok(90));
        assert_eq!(secs("30m"), Ok(1800));
        assert_eq!(secs("1h30m"), Ok(5400));
        assert_eq!(secs("2d"), Ok(172800));
        assert!(secs("").is_err());
        assert!(secs("m").is_err());
        assert!(secs("10x").is_err());
        assert!(secs("5m3").is_err());
        assert!(parse_rotation("30s").is_err());
        assert!(parse_rotation("1m").is_ok());
    }

    #[test]
    fn man_page_lists_the_commands() {
        use clap::CommandFactory;
        let mut out = Vec::new();
        clap_mangen::Man::new(Cli::command())
            .render(&mut out)
            .unwrap();
        let page = String::from_utf8(out).unwrap();
        assert!(page.contains("tmpmail\\-watch(1)"), "{page}");
    }

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
