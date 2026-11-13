<p align="center"><img src="assets/logo.svg" alt="tmpmail logo" width="88" height="88"></p>

# tmpmail

Disposable email from the terminal and a desktop app: make an address, get the verification code back.

[![CI](https://github.com/qorvyx/tmpmail/actions/workflows/ci.yml/badge.svg)](https://github.com/qorvyx/tmpmail/actions/workflows/ci.yml)
[![Providers](https://github.com/qorvyx/tmpmail/actions/workflows/providers.yml/badge.svg)](https://github.com/qorvyx/tmpmail/actions/workflows/providers.yml)
[![Latest release](https://img.shields.io/github/v/release/qorvyx/tmpmail)](https://github.com/qorvyx/tmpmail/releases/latest)
[![crates.io](https://img.shields.io/crates/v/tmpmail)](https://crates.io/crates/tmpmail)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

```sh
addr=$(tmpmail new -q)                   # quickfox605@maxxspace.com
./signup.sh "$addr"                      # anything that sends a mail to it
tmpmail otp --wait --include-existing    # waits for the mail, then prints 482913
```

![tmpmail creating an address, waiting for a mail and printing its code and link](assets/demo.gif)

## Highlights

- **A code in one line.** `otp --wait` and `wait --link` block until the mail arrives and
  print just the code or the link. Every command has `--json` and clear exit codes.
- **Six free services, automatic fallback.** mail.tm, DropMail, Guerrilla Mail and
  [others](#providers); if one is down, tmpmail uses the next.
- **Mail arrives live**, within about a second on mail.tm and DropMail.
- **For people too:** a full-screen terminal view and a desktop app for Linux, macOS and
  Windows, on the same inboxes.
- **For AI agents:** a built-in [MCP](https://modelcontextprotocol.io) server.
- **Safe by default:** HTML mail is cleaned, remote images (trackers) are blocked, and
  hidden characters can't change your terminal.
- **One small program** with nothing else to install, and verifiable builds.

**Contents**

- [Install](#install)
- [Quick start](#quick-start)
- [Desktop app](#the-desktop-app)
- [Guide](#guide)
- [Scripts and CI](#in-a-script-or-ci)
- [AI agents](#with-an-ai-agent)
- [Providers](#providers)
- [Reference](#reference)
- [When not to use tmpmail](#when-not-to-use-tmpmail)
- [Privacy and security](#privacy-and-security)
- [Contributing](#contributing)

## Install

tmpmail works on Linux (Fedora, Debian, Ubuntu, Arch and others), macOS and Windows. It
comes in two parts that share the same inboxes: the **command-line tool** (with the
full-screen view and the AI agent server) and the **desktop app**. Install one or both.

### Command-line tool

| System | Recommended | Other ways |
|---|---|---|
| **Linux** | `brew install qorvyx/tap/tmpmail` | `cargo install --locked tmpmail`, or the [download](#download-on-linux) |
| **macOS** | `brew install qorvyx/tap/tmpmail` | `cargo install --locked tmpmail`, or the `.tar.gz` from the [releases page](https://github.com/qorvyx/tmpmail/releases/latest) |
| **Windows** | `cargo install --locked tmpmail` | the `.zip` from the [releases page](https://github.com/qorvyx/tmpmail/releases/latest): unzip it and put `tmpmail.exe` in a folder on your `PATH` |

With Homebrew, use the full name `qorvyx/tap/tmpmail`: Homebrew's own `tmpmail` is a
different program. [cargo-binstall](https://github.com/cargo-bins/cargo-binstall) users can
run `cargo binstall tmpmail` to get the ready-made program instead of compiling it.

#### Download on Linux

For x86_64 (on ARM, use `aarch64` in place of `x86_64`):

```sh
curl -LO https://github.com/qorvyx/tmpmail/releases/download/v0.3.5/tmpmail-0.3.5-x86_64-unknown-linux-musl.tar.gz
tar xzf tmpmail-0.3.5-x86_64-unknown-linux-musl.tar.gz
install -D -t ~/.local/bin tmpmail-0.3.5-x86_64-unknown-linux-musl/tmpmail
```

#### Check a download

Each release has a `SHA256SUMS` file, and every file has a GitHub build attestation that
proves it was built from this repository:

```sh
gh attestation verify tmpmail-0.3.5-x86_64-unknown-linux-musl.tar.gz --repo qorvyx/tmpmail \
  --signer-workflow qorvyx/tmpmail/.github/workflows/release.yml
```

#### Shell completions (optional)

```sh
tmpmail completions bash > ~/.local/share/bash-completion/completions/tmpmail
tmpmail completions zsh  > ~/.zfunc/_tmpmail
tmpmail completions fish > ~/.config/fish/completions/tmpmail.fish
```

#### Upgrade and uninstall

| Installed with | Upgrade | Uninstall |
|---|---|---|
| Homebrew | `brew upgrade qorvyx/tap/tmpmail` | `brew uninstall qorvyx/tap/tmpmail` |
| cargo | `cargo install --locked tmpmail` | `cargo uninstall tmpmail` |
| A download | download the new version the same way | delete the `tmpmail` file |

Your inboxes and settings stay in `~/.local/state/tmpmail` and `~/.config/tmpmail`; delete
those folders to remove everything.

Optional helpers you probably have already: `copy` uses `wl-copy`, `xclip` or `xsel` on
Linux (`pbcopy` on macOS, `clip.exe` on Windows); `read --html` opens your browser;
`watch --notify` uses `notify-send` on Linux; long messages open in `less`.

### Desktop app

**[Download the desktop app (version 0.1.4)](https://github.com/qorvyx/tmpmail/releases/tag/gui-v0.1.4)**
and pick the file for your system. (The release GitHub marks "Latest" belongs to the
command-line tool and has no desktop files.)

The app isn't code-signed, so Windows and macOS warn the first time you open it; the table
says how to get past the warning.

| System | File | How to install |
|---|---|---|
| **Windows 10/11** | `tmpmail-desktop-VERSION-x64-setup.exe` | Run it. When Windows says "Windows protected your PC", click **More info**, then **Run anyway**. |
| **macOS 10.15+** | `tmpmail-desktop-VERSION-universal.dmg` | Open it and drag tmpmail to Applications. Open tmpmail once (macOS refuses), then go to System Settings → Privacy & Security and click **Open Anyway**. |
| **Debian, Ubuntu** | `tmpmail-desktop-VERSION-amd64.deb` | `sudo apt install ./tmpmail-desktop-VERSION-amd64.deb` |
| **Fedora** and other RPM systems | `tmpmail-desktop-VERSION-x86_64.rpm` | `sudo dnf install ./tmpmail-desktop-VERSION-x86_64.rpm` |
| **Other Linux** (Arch and others) | `tmpmail-desktop-VERSION-x86_64.AppImage` | `chmod +x` the file and run it. |

<details>
<summary>More install details</summary>

- **Windows:** the installer installs for your user only (no admin prompt) and fetches
  WebView2 if it's missing. **Smart App Control** blocks unsigned apps completely; it can
  only be turned off as a whole.
- **macOS 15 and later:** Control-click → Open no longer skips the warning. From a
  terminal, `xattr -dr com.apple.quarantine /Applications/tmpmail.app` does the same as
  **Open Anyway**.
- **AppImage:** it may need `libfuse2`; without FUSE, run it with
  `--appimage-extract-and-run` ([details](gui/README.md#without-fuse---appimage-extract-and-run)).
  It adds no menu entry by itself; AppImageLauncher or Gear Lever add one
  ([or by hand](gui/README.md#the-appimage)). It shows COLRv1 color emoji (Fedora's) in black
  and white, since the WebKit it carries can't draw them ([why](gui/README.md#the-appimage)).
- **NVIDIA:** if the window stays blank, start it with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
- **Uninstalling** keeps your settings and inboxes, since the command-line tool shares them.
  [How to remove everything](gui/README.md#removing-it).
- **With the command-line tool:** use version 0.3.2 or later of it.
- **Check a download:** each release has `SHA256SUMS` and build attestations
  (`gh attestation verify FILE --repo qorvyx/tmpmail --signer-workflow qorvyx/tmpmail/.github/workflows/gui-release.yml`).
- **From source** (Rust 1.90+, Node 24, pnpm): `cd gui && pnpm install && pnpm tauri dev`.

</details>

## Quick start

```console
$ tmpmail new
noblesummit274@fldubai.com
via mail.tm (until deleted; messages kept 7 days)
$ tmpmail otp --wait
482913
```

Then:

```sh
tmpmail                # list the mail that arrived
tmpmail read 1         # read the newest message
tmpmail tui            # browse everything in a full-screen view
```

The address you made last is the *active* one; every command uses it unless you
[pick another](#several-inboxes).

![The tmpmail TUI receiving a mail live and copying its code](assets/tui.gif)

In the full-screen view (`tmpmail tui`), mail shows up as it arrives, the code is
highlighted, and `y` copies it. Press `?` for the keys.

## The desktop app

Rather have a window? The desktop app does the same with the same inboxes: it shows your
mail live, puts the code in a big banner with a Copy button, shows HTML mail safely (light
or dark), saves attachments, notifies you of new mail, and can stay in the tray.
[More about the app](gui/README.md). To install it, see
[Install → Desktop app](#desktop-app).

![The tmpmail desktop app showing a verification code in a banner above the mail](assets/desktop.png)

## Guide

Each part is a short explanation and the commands for it. Not every service supports every
feature; the [provider table](#providers) shows which do.

### Create an address

```sh
tmpmail new              # a random, readable address, e.g. quickfox605@maxxspace.com
tmpmail new -c           # the same, and copy it to the clipboard
tmpmail whoami           # show it again later
```

### Choose the username or the domain

```sh
tmpmail new -u otter42                 # your own name before the @
tmpmail new otter42@maxxspace.com      # or the whole address
tmpmail domains                        # the domains on offer
tmpmail domains -p dropmail            # DropMail's (about 20)
tmpmail new -d yomail.info             # an address on that domain
```

If a site refuses one disposable domain, try another. Your own username works on mail.tm,
mail.gw, Guerrilla Mail and Maildrop.

### Choose the provider

`tmpmail new` tries the services in this order and uses the first one that works:
`mailtm, dropmail, guerrilla, tempmail_lol, mailgw`. To pick one, or change the order:

```sh
tmpmail new -p guerrilla               # only Guerrilla Mail
tmpmail providers --check              # which ones are up right now
TMPMAIL_PROVIDER=dropmail,mailtm tmpmail new
```

The order can also be set in the [config file](#configuration). Maildrop is never picked
unless you ask for it (`-p maildrop`), because its inboxes are public.

### Read your mail

```sh
tmpmail                  # list messages, newest first (same as tmpmail ls)
tmpmail read 1           # read the newest; long ones open in a pager
tmpmail read 1 --html    # the HTML version in your browser, made safe, no remote images
```

A message can be named by its number in the list or by its id (or the start of it). With
no number, commands use the newest message.

### Wait for a code or link

```sh
tmpmail otp --wait                    # wait for a new mail with a code, print the code
tmpmail wait --link --from github     # wait for a mail from GitHub, print its best link
tmpmail otp 1                         # the code in a message you already have
tmpmail links 1                       # every link in it, verification links first
```

Waiting stops after 180 seconds (change it with `-t`) and exits with code 2. It only counts
mail that arrives after you start; add `--include-existing` if the mail may already be
there. `--from`, `--subject` and `--body` pick the right mail.

With mail.tm and DropMail, new mail shows up within about a second (the service tells
tmpmail right away). The others are checked every 5 seconds.

### Attachments

```sh
tmpmail attachments 1                 # list them
tmpmail save 1 -o ~/Downloads         # download all of them
tmpmail save 1 invoice.pdf            # or just one
```

Existing files are never overwritten.

### Several inboxes

Give inboxes labels and switch between them:

```sh
tmpmail new -l github
tmpmail new -l shop
tmpmail inboxes                       # all of them; * marks the active one
tmpmail -i github ls                  # one command on another inbox
tmpmail use shop                      # make it the active one
tmpmail ls -a                         # mail in every inbox
tmpmail otp --wait -a                 # a code arriving at any of them
```

Each label belongs to one inbox. Inboxes can also be named by address or by the part
before `@`.

### Log back in

mail.tm and mail.gw inboxes are real accounts that last until you delete them. Set a
password to use one on their website too, or log in to one you made there:

```sh
tmpmail new -p mailtm --password 'correct horse'
tmpmail login otter42@maxxspace.com --password 'correct horse'
```

DropMail and Guerrilla Mail come back to the same address by themselves (mail from an expired
session is gone). Maildrop inboxes are public, so `tmpmail login name@maildrop.cc` needs no
password.

### Clean up

```sh
tmpmail delete 1                      # delete one message
tmpmail delete --all                  # every message in the inbox
tmpmail drop                          # delete the inbox and forget it
tmpmail rotate                        # swap the active address for a fresh one
tmpmail watch --rotate 30m            # a fresh address every 30 minutes
tmpmail inboxes --prune               # forget inboxes that have expired
```

### In a script or CI

Every command takes `--json`, and the exit code tells a timeout (2) from a service that is
down (3):

```sh
addr=$(tmpmail new -q)
./signup.sh "$addr"
code=$(tmpmail otp --wait --include-existing --from 'acme' --timeout 120)
link=$(tmpmail wait --link --include-existing --subject 'confirm')
tmpmail --json ls | jq -r '.messages[0].subject'
```

Use `--include-existing` when the mail might arrive before `wait` starts. Then the newest
matching mail wins, so an old code doesn't beat a resent one.

In GitHub Actions:

```yaml
- name: Install tmpmail
  run: |
    mkdir -p ~/.local/bin && cd "$RUNNER_TEMP"
    curl -sSfL -O https://github.com/qorvyx/tmpmail/releases/download/v0.3.5/tmpmail-0.3.5-x86_64-unknown-linux-musl.tar.gz \
      -O https://github.com/qorvyx/tmpmail/releases/download/v0.3.5/SHA256SUMS
    sha256sum -c --ignore-missing SHA256SUMS
    tar xzf tmpmail-0.3.5-x86_64-unknown-linux-musl.tar.gz --strip-components=1 \
      -C ~/.local/bin tmpmail-0.3.5-x86_64-unknown-linux-musl/tmpmail
    echo "$HOME/.local/bin" >> "$GITHUB_PATH"

- name: Sign-up test
  run: |
    addr=$(tmpmail new -q)
    curl -sSf https://staging.example.com/signup -d "email=$addr"
    code=$(tmpmail otp --wait --include-existing --timeout 120)
    curl -sSf https://staging.example.com/verify -d "email=$addr" -d "code=$code"
```

A [daily check](.github/workflows/providers.yml) makes sure the services work from GitHub's
Linux, macOS and Windows runners.

### With an AI agent

`tmpmail mcp` is an [MCP](https://modelcontextprotocol.io) server, so an AI agent can make
an inbox, sign up somewhere and read the code by itself:

```sh
claude mcp add tmpmail -- tmpmail mcp
```

Other clients take `{ "command": "tmpmail", "args": ["mcp"] }`. Anyone can send mail to a
throwaway address, so the agent should treat that mail as untrusted: tmpmail removes hidden
characters, shortens very long fields and marks mail as `untrusted_content`.
[Tools and details](docs/usage.md#mcp-server-for-ai-agents).

### Coming from the original tmpmail

[sdushantha/tmpmail](https://github.com/sdushantha/tmpmail) was a shell script whose only
service, 1secmail, has shut down. This is a rewrite, and its old flags still work:
`tmpmail -g [ADDR]`, `tmpmail`, `tmpmail -r`, `tmpmail ID`, `-d`, `-c`, `-t`.
[How the old flags map](docs/usage.md#original-tmpmail-flags).

## Providers

| Provider | Lifetime | Own username | Pick domain | Attachments | Instant | Delete | Raw source | Log back in |
|---|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| [mail.tm](https://mail.tm) `mailtm` | until deleted; mail kept 7 days | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | password |
| [DropMail.me](https://dropmail.me) `dropmail` | renewed while used | | ✓ | ✓ | ✓ | | ✓ | automatic |
| [Guerrilla Mail](https://www.guerrillamail.com) `guerrilla` | mail kept 60 min | ✓ | ✓ | ✓ | | ✓ | | automatic |
| [tempmail.lol](https://tempmail.lol) `tempmail_lol` | 1 hour | | | | | | | |
| [mail.gw](https://mail.gw) `mailgw` | until deleted; mail kept 7 days | ✓ | ✓ | ✓ | | ✓ | ✓ | password |
| [Maildrop](https://maildrop.cc) `maildrop` | 24 h without mail | ✓ | | | | ✓ | ✓ | by name |

- **Maildrop is public:** anyone who guesses the name can read the inbox, so it is only used
  when you ask for it.
- **tempmail.lol** is the last resort: it has no attachments.
- Many sites block known disposable domains; [another domain](#choose-the-username-or-the-domain)
  or provider usually gets through.

Thanks to **mail.tm** for its free API; as its terms ask, this program links back to it.

## Reference

<a id="commands"></a>
<details>
<summary><b>Commands</b></summary>

| Command | What it does |
|---|---|
| `new [ADDR] [-u USER] [-d DOMAIN] [-p PROVIDER] [-l LABEL] [-c]` | Create an inbox (alias `generate`) |
| `ls [-a]` | List messages, newest first, `*` = unread; `-a` = every inbox |
| `read [ID\|N] [--recent] [--html [--images]] [--raw] [--no-pager]` | Show a message |
| `wait [-a] [--from RE] [--subject RE] [--body RE] [--timeout S] [--otp\|--link] [--delete]` | Wait for a matching message |
| `otp [ID] [--regex RE] [--wait ...]` | Print a verification code |
| `links [ID] [--match RE] [--first] [--open]` | Print links, verification links first |
| `watch [-a] [--notify] [--rotate 30m]` | Print new messages as they arrive |
| `attachments ID`, `save ID [ATT...] [-o DIR]` | List / download attachments |
| `delete ID`, `delete --all` | Delete messages |
| `whoami`, `copy`, `qr` | Show / copy / show as QR code the active address |
| `inboxes [--prune]`, `use ADDR`, `-i ADDR` | Manage several inboxes |
| `login ADDR --password PW` | Use an existing mail.tm / mail.gw account |
| `rotate [--keep]` | Replace the active address with a fresh one |
| `domains [-p PROVIDER]`, `providers [--check]` | Domains, and which services are up |
| `tui` | Full-screen view, updated live |
| `mcp` | MCP server for AI agents |
| `completions bash\|zsh\|fish`, `man [-o DIR]` | Shell completions, man pages |

Every option is explained in [docs/usage.md](docs/usage.md), and the `--json` output in
[docs/json.md](docs/json.md).

</details>

<details>
<summary><b>Exit codes</b></summary>

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | other error |
| 2 | `wait` timed out (rate limits are waited out until then) |
| 3 | the service failed (down, rate limited, login refused, bad answer) |
| 4 | nothing found (no inbox, no such message, no code in the message) |
| 64 | usage error (bad flag, or an option this service doesn't support) |

With `--json`, errors are printed as `{"error":{"kind":"...","message":"..."}}`.

</details>

<a id="configuration"></a>
<details>
<summary><b>Configuration and environment</b></summary>

`~/.config/tmpmail/config.toml` (or `$TMPMAIL_CONFIG`), all optional:

```toml
providers = ["mailtm", "guerrilla"]   # order for `new`
browser = "firefox"                   # for --html and links --open
clipboard_cmd = "wl-copy"
pager = "less"                        # for long messages (default $PAGER, else less)
poll_interval = 5                     # seconds between checks
timeout = 180                         # default wait timeout
push = true                           # instant new mail from mail.tm/DropMail
http_timeout = 20                     # seconds per request to a service
```

An unknown key is ignored with a warning. A bad value is an error.

Environment: `TMPMAIL_PROVIDER` (comma list), `TMPMAIL_TIMEOUT`, `TMPMAIL_POLL_INTERVAL`,
`TMPMAIL_PUSH`, `TMPMAIL_HTTP_TIMEOUT`, `TMPMAIL_INBOX`, `TMPMAIL_PASSWORD`,
`TMPMAIL_STATE_DIR`, `TMPMAIL_DROPMAIL_TOKEN`, `PAGER`, `NO_COLOR`.

</details>

## When not to use tmpmail

- **For anything you need to keep.** An address and its mail last only as long as the
  service keeps them (from an hour to until you delete it; see [Providers](#providers)), so
  don't use one for an account you want to recover later.
- **For anything private.** The services can read the mail, and Maildrop inboxes are public.
- **To send mail.** tmpmail only receives.
- **Where disposable addresses are blocked.** Many sites refuse known disposable domains;
  [another domain](#choose-the-username-or-the-domain) or service sometimes gets through.

## Privacy and security

- **No telemetry.** tmpmail only talks to the mail service you use.
- **Your inboxes stay private.** Addresses and their passwords are kept in
  `~/.local/state/tmpmail/state.json`, readable only by you.
- **Mail can't take over your terminal.** Hidden characters and terminal control codes are
  removed before anything is shown, so a sender can't change your screen or fake a name.
- **HTML mail is made safe.** `read --html` and the desktop app remove scripts, forms and
  anything that could cover the mail, and **block remote images** (tracking pixels) unless
  you allow them (`--images`). Links only open if they are real web addresses.
- **Attachments are saved safely.** Names can't leave the folder you chose, existing files
  are never overwritten, and on Windows and macOS saved files are marked as downloaded.
- **AI agents:** anyone can mail a throwaway address, so treat what an agent reads there as
  untrusted.

The details are in [docs/usage.md](docs/usage.md#privacy-and-security-in-detail) and the app's
[security model](gui/README.md).

## Development

```sh
cargo test                  # unit tests + provider tests against a mock server
cargo clippy --all-targets
./scripts/smoke.sh          # a live check against the real services
vhs assets/demo/demo.tape   # re-record the demo
```

CI checks `Cargo.lock` against the RustSec advisory database on every push and every week
([audit workflow](.github/workflows/audit.yml); accepted advisories are listed with their
reasons in [`deny.toml`](deny.toml)).

The crate is also a library: `tmpmail = { version = "0.3", default-features = false }`
gives the providers, inbox storage, watcher and mail cleaning without the command-line
parts. Its API isn't stable yet. The desktop app is in [`gui/`](gui/README.md), with its own
`gui-v*` releases.

## Contributing

Bug reports, ideas and pull requests are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md).
To report a security problem privately, see [SECURITY.md](SECURITY.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
