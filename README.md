<p align="center"><img src="assets/logo.svg" alt="tmpmail logo" width="88" height="88"></p>

<h1 align="center">tmpmail</h1>

<p align="center">
  <b>Disposable email in one command: make an address, get the verification code back.</b><br>
  From the terminal, a full-screen terminal view (TUI) or a desktop app, on Linux, macOS and Windows.
</p>

<p align="center">
  <a href="https://github.com/qorvyx/tmpmail/actions/workflows/ci.yml"><img src="https://github.com/qorvyx/tmpmail/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/qorvyx/tmpmail/actions/workflows/providers.yml"><img src="https://github.com/qorvyx/tmpmail/actions/workflows/providers.yml/badge.svg" alt="Providers"></a>
  <a href="https://github.com/qorvyx/tmpmail/releases/latest"><img src="https://img.shields.io/github/v/release/qorvyx/tmpmail" alt="Latest release"></a>
  <a href="https://crates.io/crates/tmpmail"><img src="https://img.shields.io/crates/v/tmpmail" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue" alt="License: GPL-3.0-or-later"></a>
</p>

```sh
addr=$(tmpmail new -q)                   # quickfox605@maxxspace.com
./signup.sh "$addr"                      # anything that sends a mail to it
tmpmail otp --wait --include-existing    # waits for the mail, then prints 482913
```

![tmpmail creating an address, waiting for a mail and printing its code and link](assets/demo.gif)

## Highlights

- **Six free services, automatic fallback.** mail.tm, DropMail, Guerrilla Mail and
  [others](#providers); if one is down, tmpmail uses the next.
- **Mail arrives live**, within about a second on mail.tm and DropMail.
- **For scripts and CI:** a code in one line. `otp --wait` and `wait --link` block until the
  mail arrives and print just the code or the link; every command has `--json` and clear exit
  codes.
- **For people:** a full-screen terminal view (TUI, `tmpmail tui`) and a desktop app for
  Linux, macOS and Windows, on the same inboxes.
- **For AI agents:** a built-in [MCP](https://modelcontextprotocol.io) server.
- **Safe by default:** HTML mail is cleaned, remote images (trackers) are blocked, and
  hidden characters can't change your terminal.
- **One small program** with nothing else to install, and verifiable builds.
- **An independent rewrite of the original tmpmail** shell script, whose only service shut down;
  [its old flags still work](#coming-from-the-original-tmpmail).

## Contents

- 📦 **[Install](#install)**
  <details><summary>Debian, Fedora, Arch, macOS, Windows</summary>

  - [Debian and Debian-based](#debian-and-debian-based)
  - [Fedora and RPM-based](#fedora-and-rpm-based)
  - [Arch and other Linux](#arch-and-other-linux)
  - [macOS](#macos)
  - [Windows](#windows)
  - [More options](#more-install-options)

  </details>
- 🚀 **[Quick start](#quick-start)**
- 🖥️ **[Desktop app](#the-desktop-app)**
- 📖 **[Guide](#guide)**
  <details><summary>Addresses, reading, waiting, inboxes, scripts, AI agents</summary>

  - [Create an address](#create-an-address)
  - [Choose the username or the domain](#choose-the-username-or-the-domain)
  - [Choose the provider](#choose-the-provider)
  - [Read your mail](#read-your-mail)
  - [Wait for a code or link](#wait-for-a-code-or-link)
  - [Attachments](#attachments)
  - [Several inboxes](#several-inboxes)
  - [Log back in](#log-back-in)
  - [Clean up](#clean-up)
  - [Scripts and CI](#in-a-script-or-ci)
  - [AI agents](#with-an-ai-agent)
  - [Coming from the original tmpmail](#coming-from-the-original-tmpmail)

  </details>
- 📮 **[Providers](#providers)**
- 📚 **[Reference](#reference)**
  <details><summary>Commands, exit codes, configuration</summary>

  - [Commands](#commands)
  - [Exit codes](#exit-codes)
  - [Configuration and environment](#configuration)

  </details>
- ⚠️ **[When not to use tmpmail](#when-not-to-use-tmpmail)**
- 🔒 **[Privacy and security](#privacy-and-security)**
- 🛠️ **[Development](#development)**
- 🤝 **[Contributing](#contributing)**
- 📝 **[To do](#to-do)**

## Install

tmpmail comes in two parts. They share the same inboxes and settings; install one or both.

| Part | Command | What you get |
|---|---|---|
| **Command-line tool** | `tmpmail` | the commands below, a full-screen view in the terminal (`tmpmail tui`) and the [AI agent server](#with-an-ai-agent) |
| **Desktop app** | **tmpmail** in your apps menu (`tmpmail-gui` from a terminal with the `.deb` or `.rpm`; the AppImage runs from its own file) | a window with your mail, the code in a banner with a Copy button, notifications and a tray icon |

Pick your system: [Debian and Debian-based](#debian-and-debian-based) ·
[Fedora and RPM-based](#fedora-and-rpm-based) ·
[Arch and other Linux](#arch-and-other-linux) · [macOS](#macos) · [Windows](#windows).
The desktop app's files are on its own
**[release page (version 0.1.6)](https://github.com/qorvyx/tmpmail/releases/tag/gui-v0.1.6)**.

### Debian and Debian-based

For Debian and the systems built on it that use `apt`: Ubuntu, Linux Mint, Pop!_OS,
elementary OS, Zorin OS and others. The desktop app needs Debian 12 or Ubuntu 22.04 or later
(or a system based on them).

#### ⌨️ Command-line tool

```sh
curl -fLO https://github.com/qorvyx/tmpmail/releases/download/v0.3.7/tmpmail-cli_0.3.7_amd64.deb
sudo apt install ./tmpmail-cli_0.3.7_amd64.deb
```

#### 🖥️ Desktop app

```sh
curl -fLO https://github.com/qorvyx/tmpmail/releases/download/gui-v0.1.6/tmpmail-desktop-0.1.6-amd64.deb
sudo apt install ./tmpmail-desktop-0.1.6-amd64.deb
```

### Fedora and RPM-based

For Fedora and the other systems that use `dnf`. The command-line tool also installs on
RHEL, CentOS Stream, Rocky Linux and AlmaLinux; the desktop app needs version 9 or later of
those, with [EPEL](https://docs.fedoraproject.org/en-US/epel/) turned on.

#### ⌨️ Command-line tool

```sh
curl -fLO https://github.com/qorvyx/tmpmail/releases/download/v0.3.7/tmpmail-cli-0.3.7-1.x86_64.rpm
sudo dnf install ./tmpmail-cli-0.3.7-1.x86_64.rpm
```

#### 🖥️ Desktop app

```sh
curl -fLO https://github.com/qorvyx/tmpmail/releases/download/gui-v0.1.6/tmpmail-desktop-0.1.6-x86_64.rpm
sudo dnf install ./tmpmail-desktop-0.1.6-x86_64.rpm
```

<details>
<summary>More about the .deb and .rpm packages</summary>

- **On ARM** (command-line tool only): use `arm64` in place of `amd64` in the `.deb` name, or
  `aarch64` in place of `x86_64` in the `.rpm` name.
- **Package names** (what `apt remove` and `dnf remove` take) aren't the commands:

  | Package name | Command | What it is |
  |---|---|---|
  | `tmpmail-cli` | `tmpmail` | the command-line tool |
  | `tmpmail` | `tmpmail-gui` | the desktop app |

- **What the command-line package installs:** `/usr/bin/tmpmail`, its man pages
  (`man tmpmail`) and the bash, zsh and fish completions. It needs no other package;
  `xdg-utils` (to open mail and links in the browser) is recommended, so apt and dnf install
  it too, and `wl-clipboard` or `xclip` lets `tmpmail copy` use the clipboard.
- **openSUSE:** the `.rpm` files install with
  `sudo zypper install --allow-unsigned-rpm ./FILE.rpm`.
- **Updates:** the packages don't update by themselves; install the next version's file the
  same way.

</details>

### Arch and other Linux

#### ⌨️ Command-line tool

Either with [Homebrew](https://brew.sh):

```sh
brew install qorvyx/tap/tmpmail
```

or by downloading it (on ARM, use `aarch64` in place of `x86_64`):

```sh
curl -fLO https://github.com/qorvyx/tmpmail/releases/download/v0.3.7/tmpmail-0.3.7-x86_64-unknown-linux-musl.tar.gz
tar xzf tmpmail-0.3.7-x86_64-unknown-linux-musl.tar.gz
install -D -t ~/.local/bin tmpmail-0.3.7-x86_64-unknown-linux-musl/tmpmail
```

If a new terminal then says `tmpmail: command not found`, add `~/.local/bin` to your `PATH`
once (bash; for zsh use `~/.zshrc`, and in fish run `fish_add_path ~/.local/bin`):

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

#### 🖥️ Desktop app

The AppImage, which runs on its own:

```sh
curl -fLO https://github.com/qorvyx/tmpmail/releases/download/gui-v0.1.6/tmpmail-desktop-0.1.6-x86_64.AppImage
chmod +x tmpmail-desktop-0.1.6-x86_64.AppImage
./tmpmail-desktop-0.1.6-x86_64.AppImage
```

<details>
<summary>More about the AppImage</summary>

- It may need `libfuse2`; without FUSE, run it with `--appimage-extract-and-run`
  ([details](gui/README.md#without-fuse---appimage-extract-and-run)).
- It adds no menu entry by itself; AppImageLauncher or Gear Lever add one
  ([or by hand](gui/README.md#the-appimage)).
- It shows COLRv1 color emoji (Fedora's) in black and white, since the WebKit it carries
  can't draw them ([why](gui/README.md#the-appimage)).
- On NVIDIA, if the window stays blank, start it with `WEBKIT_DISABLE_DMABUF_RENDERER=1`
  (this goes for the `.deb` and `.rpm` too).

</details>

### macOS

#### ⌨️ Command-line tool

With [Homebrew](https://brew.sh):

```sh
brew install qorvyx/tap/tmpmail
```

#### 🖥️ Desktop app

With Homebrew (macOS 11 or later):

```sh
brew install --cask qorvyx/tap/tmpmail-desktop
```

The app isn't notarized by Apple, so macOS refuses to open it the first time: open
it once, then go to **System Settings → Privacy & Security** and click **Open Anyway**.
macOS asks again after each update.

<details>
<summary>More macOS details</summary>

- **Use the full names** (`qorvyx/tap/...`): Homebrew's own `tmpmail` is a different program.
- **Without Homebrew:** the command-line tool's `.tar.gz` is on the
  [releases page](https://github.com/qorvyx/tmpmail/releases/latest); the desktop app's
  `tmpmail-desktop-0.1.6-universal.dmg` (macOS 10.15 or later) is on
  [its release page](https://github.com/qorvyx/tmpmail/releases/tag/gui-v0.1.6): open it
  and drag tmpmail to Applications.
- **From a terminal**, `xattr -dr com.apple.quarantine /Applications/tmpmail.app` does the
  same as **Open Anyway** (on macOS 15 and later, Control-click → Open no longer does).

</details>

### Windows

#### ⌨️ Command-line tool

1. Download
   [`tmpmail-0.3.7-x86_64-pc-windows-msvc.zip`](https://github.com/qorvyx/tmpmail/releases/download/v0.3.7/tmpmail-0.3.7-x86_64-pc-windows-msvc.zip)
   and unzip it.
2. Open File Explorer, paste `%LOCALAPPDATA%\Programs` into its address bar and press
   Enter. Make a new folder there named `tmpmail`, and move `tmpmail.exe` into it.
3. Add that folder to your `Path`: open Start, search for **Edit environment variables for
   your account**, select **Path**, click **Edit**, then **New**, and paste
   `%LOCALAPPDATA%\Programs\tmpmail`.
   Click **OK** twice.
4. Open a new terminal and run `tmpmail --version` to check.

#### 🖥️ Desktop app

Download
[`tmpmail-desktop-0.1.6-x64-setup.exe`](https://github.com/qorvyx/tmpmail/releases/download/gui-v0.1.6/tmpmail-desktop-0.1.6-x64-setup.exe)
and run it. The app isn't code-signed, so Windows warns:

1. Edge may say the file "isn't commonly downloaded": open the downloads list, click **⋯**
   next to the file, then **Keep**, **Show more**, **Keep anyway**.
2. If Windows says "Windows protected your PC", click **More info**, then **Run anyway**.

<details>
<summary>More Windows details</summary>

- The installer installs for your user only (no admin prompt) and fetches WebView2 if it's
  missing. **Smart App Control** blocks unsigned apps completely; it can only be turned off
  as a whole.
- The app installs into `%LOCALAPPDATA%\tmpmail`, the same folder that holds the
  command-line tool's inboxes.
- `cargo install --locked tmpmail` needs Rust and the Visual Studio C++ Build Tools;
  cargo-binstall's own Windows program needs the Visual C++ Redistributable.

</details>

### More install options

<details>
<summary>cargo, shell completions, checking a download, updating and uninstalling</summary>

**With Rust:** `cargo install --locked tmpmail` builds the command-line tool on any system;
[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) users can run
`cargo binstall tmpmail` to get the ready-made program instead.

**Shell completions:** Homebrew and the `.deb` and `.rpm` install them already. For the
other ways:

```sh
tmpmail completions bash > ~/.local/share/bash-completion/completions/tmpmail
tmpmail completions zsh  > ~/.zfunc/_tmpmail
tmpmail completions fish > ~/.config/fish/completions/tmpmail.fish
```

In PowerShell, write them to a file of their own and load it from your profile:

```powershell
tmpmail completions powershell | Out-File -Encoding utf8 $HOME\tmpmail-completions.ps1
if (!(Test-Path $PROFILE)) { New-Item -Force $PROFILE }
Add-Content $PROFILE '. $HOME\tmpmail-completions.ps1'
```

If a new PowerShell window then says "running scripts is disabled on this system", allow
local scripts once: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.

**Check a download:** each release has a `SHA256SUMS` file. Download it next to the file
and run `sha256sum -c --ignore-missing SHA256SUMS` (on Windows, `certutil -hashfile FILE
SHA256` prints the hash to compare). Every file also has a GitHub build attestation that
proves it was built from this repository (needs the GitHub CLI 2.49 or later, logged in):

```sh
gh attestation verify tmpmail-0.3.7-x86_64-unknown-linux-musl.tar.gz --repo qorvyx/tmpmail \
  --signer-workflow qorvyx/tmpmail/.github/workflows/release.yml
```

For the desktop app's files, end it with `gui-release.yml` instead.

**Update and uninstall the command-line tool:**

| Installed with | Update | Uninstall |
|---|---|---|
| Homebrew | `brew upgrade qorvyx/tap/tmpmail` | `brew uninstall qorvyx/tap/tmpmail` |
| The `.deb` | install the new version's file | `sudo apt remove tmpmail-cli` |
| The `.rpm` | install the new version's file | `sudo dnf remove tmpmail-cli` |
| cargo | `cargo install --locked tmpmail` | `cargo uninstall tmpmail` |
| A download | download the new version the same way | delete `tmpmail` (`tmpmail.exe`) |

The desktop app: update by installing the new version's file (Homebrew:
`brew upgrade --cask qorvyx/tap/tmpmail-desktop`); uninstall it like any app
(`sudo apt remove tmpmail`, `sudo dnf remove tmpmail`, Windows Settings → Apps, or
`brew uninstall --cask qorvyx/tap/tmpmail-desktop`).

Uninstalling either part keeps your inboxes and settings, since the other may use them. To
remove everything, delete `~/.local/state/tmpmail` and `~/.config/tmpmail` on Linux,
`~/Library/Application Support/tmpmail` on macOS, or `%LOCALAPPDATA%\tmpmail` and
`%APPDATA%\tmpmail` on Windows (uninstall the desktop app first there: it is installed in
`%LOCALAPPDATA%\tmpmail` too). [The app's own data](gui/README.md#removing-it).

**Optional helpers** you probably have already: `copy` uses `wl-copy`, `xclip` or `xsel` on
Linux (`pbcopy` on macOS, `clip.exe` on Windows); `read --html` opens your browser;
`watch --notify` uses `notify-send` on Linux; long messages open in `less`.

**Versions:** the desktop app works with the command-line tool from version 0.3.2 on.

**Building the desktop app from source** (Rust 1.90+, Node 24, pnpm):
`cd gui && pnpm install && pnpm tauri dev`.

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
[More about the app](gui/README.md). To install it, see [Install](#install).

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
    curl -sSfL -O https://github.com/qorvyx/tmpmail/releases/download/v0.3.7/tmpmail-0.3.7-x86_64-unknown-linux-musl.tar.gz \
      -O https://github.com/qorvyx/tmpmail/releases/download/v0.3.7/SHA256SUMS
    sha256sum -c --ignore-missing SHA256SUMS
    tar xzf tmpmail-0.3.7-x86_64-unknown-linux-musl.tar.gz --strip-components=1 \
      -C ~/.local/bin tmpmail-0.3.7-x86_64-unknown-linux-musl/tmpmail
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
`tmpmail -g [ADDR]`, `tmpmail`, `tmpmail -r`, `tmpmail ID`, `-d`, `-c`, `-t`, `-b BROWSER`.
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
| `completions bash\|zsh\|fish\|powershell`, `man [-o DIR]` | Shell completions, man pages |

Every option is explained in [docs/usage.md](docs/usage.md), and the `--json` output in
[docs/json.md](docs/json.md).

</details>

<a id="exit-codes"></a>
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

`config.toml`, all optional: in `~/.config/tmpmail/` on Linux,
`~/Library/Application Support/tmpmail/` on macOS, `%APPDATA%\tmpmail\` on Windows (or the
file `$TMPMAIL_CONFIG` names):

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
- **Your inboxes stay private.** Addresses and their passwords are kept in a `state.json`
  only you can read (`~/.local/state/tmpmail/` on Linux; [elsewhere](#more-install-options)).
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

## To do

- [ ] Windows: Scoop and winget packages, so the command-line tool installs in one command
- [ ] Arch: an AUR package (under another name: `tmpmail` there is the original script)
- [ ] The desktop app on Flathub
- [ ] A Nix package
- [ ] The desktop app for arm64 Linux

Want to help with one? See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
