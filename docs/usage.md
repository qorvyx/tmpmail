# tmpmail usage reference

Every command and option, explained. `tmpmail --help` and `tmpmail <command> --help` show
the same information in short form.

- [Basics](#basics): how messages and inboxes are referenced
- [Global options](#global-options): work with every command, in any position
- [Commands](#commands)
- [Short flags at a glance](#short-flags-at-a-glance)
- [Original tmpmail flags](#original-tmpmail-flags)
- [Environment variables](#environment-variables)
- [Exit codes](#exit-codes)

## Basics

**Several inboxes at once.** `ls`, `wait`, `otp --wait` and `watch` accept `-a`
(`--all-inboxes`) to cover every saved inbox instead of just the active one:

```sh
tmpmail ls -a          # check all my addresses
tmpmail wait -a        # block until any of them receives a message
tmpmail watch -a -n    # live feed of all of them, with desktop notifications
```

**Active inbox.** `tmpmail new` creates an address and makes it the *active* inbox. Every
other command works on the active inbox unless you pick another one with `-i`.

**Choosing an inbox** (`-i`, `use`, `drop`). Any of these work:
- the full address: `quietotter482@maxxspace.com`
- the part before `@`: `quietotter482`
- a label you gave it with `-l`: `github`

A label wins over a local part: if one inbox is labelled `work` and another is
`work@example.com`, `-i work` picks the labelled one. A local part that matches several inboxes
picks none: tmpmail lists them and exits with 64, so use the full address. A new label can't be
one that another inbox already has, nor another inbox's address or local part. A new or
logged-in address can't have another inbox's label before its `@` either (`new -u work`,
`new work@…`, `login work@…`): that's refused with exit 64, naming the labelled inbox, before
any provider is asked. If a provider hands out such a name by chance (a random address, or
`rotate`), the inbox is kept, a warning says so (the MCP server's `create_inbox` adds it as `note`), and the label still picks its own inbox. Creating or logging in again to an address that is already stored is not refused, and keeps its label.

**Choosing a message** (`read`, `otp`, `links`, `delete`, `save`, `attachments`). Any of these work:
- its number in `tmpmail ls`: `1` is the newest
- its full id: `6aa6c0f1e2...`
- the first few characters of its id, if no other message starts the same way: `6aa6`
- nothing at all, which means the newest message

**Regex options** (`--from`, `--subject`, `--body`, `--match`) take regular expressions and
ignore case. Plain words work: `--from github` matches `GitHub <noreply@github.com>`.

---

## Global options

| Option | Short | Explanation |
|---|---|---|
| `--json` | | Print machine-readable JSON instead of text (format in [json.md](json.md)). Errors become `{"error":{"kind","message"}}`, on stdout; so do usage errors (an unknown option, a bad value, options that conflict), with kind `usage` and exit 64. |
| `--inbox ADDR` | `-i` | Act on this inbox instead of the active one. Example: `tmpmail -i github ls`. Also settable with `TMPMAIL_INBOX`. |
| `--quiet` | `-q` | Print only the essential result, without hints or status lines. Example: `addr=$(tmpmail new -q)`. |
| `--no-color` | | No bold or dim text. Colors are also off when output is piped or `NO_COLOR` is set. In a Windows console, tmpmail turns on its escape-code processing (off in the classic `cmd.exe` window) and prints without colors if it can't. |
| `--browser CMD` | `-b` | Program used by `read --html` and `links --open`, e.g. `-b firefox` or `-b w3m`. Default: `$BROWSER`, then the system opener: `xdg-open` (Linux), `open` (macOS), `wslview` (WSL), or Explorer's default handler on Windows (started without tmpmail's own handles, so a file its output is redirected to stays free). |
| `--clipboard-cmd CMD` | | Program that receives the address on stdin for `copy` and `--copy`. Default: auto-detects `wl-copy` (Wayland), `xclip`/`xsel` (X11), `pbcopy` (macOS), `clip.exe` (Windows, WSL), `termux-clipboard-set` (Termux). |
| `--help` | `-h` | Help for tmpmail or for one command (`tmpmail wait -h`). |
| `--version` | `-V` | Print the version. |

---

## Commands

### `new`: create an address

Creates an address, saves it and makes it active. Alias: `generate`. Old flag: `-g`.

| Option | Short | Explanation |
|---|---|---|
| `ADDRESS` | | Full address you want, e.g. `tmpmail new otter42@maxxspace.com`. Same as `-u otter42 -d maxxspace.com`. |
| `--user NAME` | `-u` | The part before `@`. Random and readable by default (`quietotter482`). Only providers that allow choosing it are used: mail.tm, mail.gw, Guerrilla, Maildrop. Refused (exit 64, before anything is created) if it is another inbox's label. |
| `--domain DOMAIN` | `-d` | The part after `@`. See `tmpmail domains` for choices. tmpmail asks every provider for its domains at once and goes straight to one that has it, so `-d` works without `-p`. If none that answered has it, that's exit 64; if some couldn't be reached, exit 3 names them. Maildrop's `maildrop.cc` is the exception: Maildrop is left out of the fallback (its inboxes are public), so `-d maildrop.cc` or `new NAME@maildrop.cc` asks you to add `-p maildrop` (exit 64; the MCP server's `create_inbox` and the apps ask for that provider too). An empty username or domain (`new foo@`) is a usage error too. |
| `--provider ID` | `-p` | Use only this provider (no fallback): `mailtm`, `mailgw`, `dropmail`, `guerrilla`, `tempmail_lol`, `maildrop`. |
| `--label NAME` | `-l` | A nickname for the inbox, used later with `-i NAME`. Example: `-l github`. Refused (exit 64, before anything is created) if another inbox already has it, or has it as its address or local part. |
| `--password PW` | | Password for the mail.tm/mail.gw account, so you can log in later on the mail.tm website. Random by default. Also `TMPMAIL_PASSWORD`. |
| `--copy` | `-c` | Also copy the new address to the clipboard. If that fails (no clipboard tool, or it exits with an error), the address is still saved and printed, a warning goes to stderr, and the exit code stays 0. |

```sh
tmpmail new                        # random address, first working provider
tmpmail new -u myname -l shop -c   # chosen name, labelled "shop", copied
tmpmail new -p guerrilla           # force Guerrilla Mail
```

### `ls`: list messages

Lists messages, newest first. Alias: `inbox`. Running `tmpmail` with no arguments does the same.
Each line shows the list number, a `*` if the message is unread, its age, the sender and
the subject. `[att]` means the message has attachments. A message counts as read once
`read`, the MCP server's `read_message`, the TUI (once it has loaded after you moved to it or into its preview) or
the desktop app has opened it (tmpmail remembers it in its state file, with any provider), or
when the provider marks it read itself.

| Option | Short | Explanation |
|---|---|---|
| `--all-inboxes` | `-a` | List the messages of **every** saved inbox, one section per address. An inbox whose provider is down shows `unavailable: ...` instead of stopping the others. If no inbox could be read, the exit code is the first one's error (3 when its provider is unreachable). |

### `read`: show a message

Prints the headers and body as text. Alias: `show`. Old flags: `-r` (newest), `tmpmail ID`.
The hints it prints (`tmpmail save ID`, `tmpmail read ID --html`) include `-i ADDRESS` when
the inbox isn't the active one (picked with `-i`, `TMPMAIL_INBOX`, or `wait -a`).

| Option | Short | Explanation |
|---|---|---|
| `ID` | | Which message (number, id or id prefix). Newest if omitted. |
| `--recent` | `-r` | Explicitly the newest message. |
| `--html` | | Open the HTML version in your browser. It is sanitized first: no scripts, forms or frames, links can't track you back, no relative, `//host` or `\\host` links or images (from a `file:` page they would reach another machine: on Windows an SMB connection), `data:` only as an image's raster picture, inline styles keep only look-and-layout properties (colors, fonts, spacing, borders, sizes, table and flex layout; not `position`, `z-index`, `transform`, `scale`, `opacity`..., and no negative margins; a declaration with a CSS escape goes), and without `--images` a style that could load anything (`url(`, `image-set(`, `@import`, also spelled with escapes) goes; with `--images` it may load only web and inline raster images. In-page `#` links stay. Inline images (`cid:`) are put in the copy as `data:` URLs: the attachment the provider lists under that name, else, where the provider has the raw message (mail.tm, mail.gw, DropMail, Maildrop), the part with that `Content-ID` in it (raster images only, up to 10 MiB each, at most 64 `cid:` names per message and 25 MiB of images in the copy; an attachment that several names point at is fetched once). The copy is a private file (`0600`, in a `0700` folder) under `view/` in the state folder, deleted after a day. A browser installed as a snap (Firefox on Ubuntu and Kubuntu) can't read hidden folders in your home, so when the browser that opens it is a snap, the copy goes to `~/snap/NAME/common/tmpmail-view/` instead (the snap may always read its own folder there). The browser is `--browser` or `$BROWSER` (a command in `/snap/bin` or `/var/lib/snapd/snap/bin`, a link to one, or a script that `exec`s one), or for `xdg-open` and the other desktop openers the default for HTML files (`xdg-mime query default text/html`). The snap can write in that folder and swap any folder below `~/snap/NAME` for a link, so tmpmail opens `~/snap`, then `NAME`, `common` and `tmpmail-view` one inside the other, refusing a link, and writes, lists and deletes only inside the folder it opened. If one of them is a link (or the snap never ran, so `common` is missing, or the copy can't be written there), the copy goes to the state folder instead, with a warning. Copies are named `tmpmail-ID.html`; in a snap's folder only such files are deleted, and only in the folder of the browser being used, so after switching to another browser the old one's copies stay until it is used again. |
| `--images` | | With `--html`: also load remote images. Off by default because they are often tracking pixels that tell the sender you opened the mail. As with any remote content, the sender's servers then see your IP address and when you opened the mail. Guerrilla Mail points images at its own proxy, which only works inside a Guerrilla session; tmpmail points them back at each image's own URL so they load too. |
| `--raw` | | Print the full raw email source (headers + MIME). Useful for debugging or `> mail.eml`. Supported by mail.tm, mail.gw, DropMail and Maildrop; Guerrilla and tempmail.lol don't offer it (exit 64). `providers` shows it as `raw`. |
| `--no-pager` | | Print a long message straight to the terminal. By default a message taller than the terminal opens in a pager: `pager` from the config, else `$PAGER`, else `less`. On Windows, if neither is set and `less` isn't installed, the message is printed instead. |

mail.tm and mail.gw cut a long message's text (and HTML) at 100 KiB, mid-line and without a
word. When a body is that long, `read` (and everything else that reads a message: `wait`,
`otp`, `links`, the TUI, the MCP server, the desktop app) checks the raw source and uses its
whole text instead. If the source can't be read (or has no longer text it can decode), the
text ends with a note that it may be cut and that `read --raw` has all of it.

### `wait`: wait for a message

Blocks until a **new** message that matches arrives, prints it and exits 0. If nothing
arrives in time it exits with code 2. Built for scripts and CI.

| Option | Short | Explanation |
|---|---|---|
| `--all-inboxes` | `-a` | Wait on **every** saved inbox at once. The first matching message in any of them wins, and tmpmail tells you which address received it. Can't be combined with `-i`. |
| `--from RE` | | Only messages whose sender matches, e.g. `--from github`. |
| `--subject RE` | | Only messages whose subject matches, e.g. `--subject 'verify\|confirm'`. |
| `--body RE` | | Only messages whose body matches. |
| `--timeout SECONDS` | `-t` | Give up after this long. Default 180 (or `timeout` in config / `TMPMAIL_TIMEOUT`). |
| `--interval SECONDS` | | Time between inbox checks. Default 5, minimum 2. With push (see below) checks are rarer, since the provider announces new mail. |
| `--include-existing` | | Also accept a message that was already in the inbox before you started waiting. Among the messages there when the wait starts, the **newest** match wins (in each inbox, with `-a`), so a code from an earlier try doesn't beat a resent one that has arrived; mail that arrives during the wait is taken oldest first. |
| `--otp` | | Print only the verification code, and keep waiting until a message that contains one arrives. |
| `--otp-regex RE` | | With `--otp`: your own pattern; the first `( )` group is the code. Example: `--otp-regex 'PIN: (\d+)'`. |
| `--link` | | Print only the best link (verification links first). |
| `--delete` | | Delete the message after printing it. Refused before waiting (exit 64) when the inbox's provider can't delete messages (DropMail, tempmail.lol); with `-a`, those inboxes are skipped with a note instead. If the delete itself fails, the message stays printed (one JSON document with `--json`), the failure is told on stderr, and the exit code is the failure's (e.g. 3). |

```sh
code=$(tmpmail wait --otp --from github -t 120)
tmpmail wait -a                     # until any of my inboxes gets mail
tmpmail wait -a --otp -t 600        # a code arriving at any of my inboxes
```

**Push.** For mail.tm and DropMail inboxes, `wait` and `watch` also keep a connection open
over which the provider announces new mail (mail.tm's Mercure server-sent events, DropMail's
`sessionMailReceived` subscription). New mail then shows up within a second or so, instead of
at the next check. While the connection is up, the inbox is still checked once a minute in case
something was missed. A mail.tm connection stays open for as long as the provider keeps it
(75 s without even a keep-alive counts as dead), and is renewed every 30 minutes with the
inbox's current token. If a connection that had been up for a minute or more drops, tmpmail reconnects within a few seconds and the inbox still counts as live meanwhile;
if connections keep failing, it goes back to checking every `--interval` and reconnects with a
growing delay. Turn push off with `push = false` in the
config or `TMPMAIL_PUSH=0`.

### `otp`: extract a verification code

Finds the most likely one-time code (e.g. `482913`, `123-456`, `A7K2QX`) in a message.
Years, prices, phone numbers and order numbers are ignored. With `--json` it also lists
the other candidates with their scores.

| Option | Short | Explanation |
|---|---|---|
| `ID` | | Which message. Newest if omitted. |
| `--recent` | `-r` | The newest message. |
| `--regex RE` | | Your own pattern instead of the built-in guess. The first `( )` group is the code. |
| `--wait` | `-w` | Don't read an existing message; wait for a new one that contains a code (same as `wait --otp`). |
| `--all-inboxes`/`-a`, `--from`, `--subject`, `--body`, `--timeout`/`-t`, `--interval`, `--include-existing` | | Same as in `wait`, used with `--wait`. |

### `links`: extract links

Lists every link in a message. Verify, confirm, activate, magic-login and password-reset links
come first. Unsubscribe, social media and image links come last. An address the HTML only
loads (an image's `src`, `srcset` or `background`, a CSS `url(...)`) isn't a link a reader
would open, so it is left out, also when the text part repeats it, unless the mail links to
it (`href`). A link is read as a browser reads it (character references such as `&#38;`
decoded, line breaks dropped, spaces and invisible characters inside escaped as `%20` and
the like), and is then only a whole absolute `http://` or `https://` URL with a host: a
string that only starts with `http` (`http\..\setup.exe`, `http/../file.html`) or has a
backslash in it is left out, since an opener would take it for a file on disk. So a link
never holds a line break, which a shell would run as a second command when the link is
pasted (still quote a pasted link: `;` or `|` in it mean something to a shell). `--open`
(and `O` in the TUI) refuse anything else whatever the list holds. The same list feeds
`wait --link`, `L`/`O` in the TUI, the MCP server and the desktop app.

| Option | Short | Explanation |
|---|---|---|
| `ID` | | Which message. Newest if omitted. |
| `--recent` | `-r` | The newest message. |
| `--match RE` | `-m` | Keep only links matching this, e.g. `-m confirm`. |
| `--first` | | Print only the best link. |
| `--open` | | Open the best link in the browser. |

### `watch`: live view

Keeps running and prints each new message as it arrives (Ctrl+C to stop). Messages already
there are shown dimmed at start. With `--json` it prints one JSON object per line.

| Option | Short | Explanation |
|---|---|---|
| `--all-inboxes` | `-a` | Watch **every** saved inbox; each line starts with the address that received it. |
| `--notify` | `-n` | Also show a desktop notification for each new message, through `notify-send` (Linux), `osascript` (macOS) or a PowerShell toast (Windows). Nothing is shown if none is available. On Linux, `<`, `>` and `&` in the subject are escaped when the notification server reads markup (its `body-markup` capability, asked through `gdbus` or `dbus-send`; assumed when neither can tell), so a sender can't add bold text, links or images to it. |
| `--interval SECONDS` | | Time between checks. Default 5. Push applies as in `wait`. |
| `--rotate PERIOD` | | Switch to a fresh address this often, like `rotate` does: `90s`, `30m`, `2h`, `1h30m` (at least `1m`). The new address is printed (and notified with `-n`), becomes active, and is watched from then on. Can't be combined with `-a`. |
| `--keep` | | With `--rotate`: keep the old inboxes instead of dropping them; the label moves to each new inbox, as with `rotate --keep`. |
| `--copy` | `-c` | With `--rotate`: copy each new address to the clipboard. |

```sh
tmpmail watch --rotate 30m -c    # a new address every half hour, copied each time
```

### `tui`: full-screen browser

```sh
tmpmail tui          # or: tmpmail -i LABEL tui, to start on that inbox
```

Your inboxes on the left, the selected inbox's mail at the top right and the selected message
below it. Every inbox is checked in the background on the same schedule as `watch` (push on
mail.tm and DropMail, polling elsewhere), so new mail appears on its own; the header shows
`● live` when the selected inbox has a push connection. When a message contains a
verification code, the preview shows it highlighted at the top and `y` copies it.

| Key | Action |
|---|---|
| `Tab` (`Shift+Tab` back), `←` `→` (`h` `l`), `Enter` | Move between panes |
| `↑` `↓` (`j` `k`), `g` `G` (`Home` `End`) | Move in a list, or scroll the message (`G` stops at its last page) |
| `PgUp` `PgDn`, `Space` | Scroll the message |
| `n` | Create an inbox (with the same fallback as `new`) |
| `N` | Pick a provider from a list and create an inbox there (no fallback, like `new -p`). Maildrop is marked `public` |
| `a` | Make the selected inbox the active one |
| `c` | Copy the selected inbox's address |
| `D` | Delete the selected inbox (asks first) |
| `y` | Copy the verification code |
| `L` / `O` | Copy / open the message's first link (verification links first) |
| `o` | Open the message's sanitized HTML in the browser, like `read --html` |
| `d` | Delete the message (asks first) |
| `r` | Check the selected inbox now |
| `?` | Show the keys. In a terminal too short for all of them, `↑` `↓` (`j` `k`), `PgUp` `PgDn` (`Space`) and `g` `G` (`Home` `End`) scroll the list, and its bottom line says which rows are shown (`1-5 of 18`); any other key (`Esc`, `?`) closes it. When every key shows, any key closes it |
| `q`, `Esc`, `Ctrl+C` | Quit (`Esc` steps back a pane first) |

The bottom line shows the keys for the focused pane, then any status message on the right. A
hint that doesn't fit next to the message is left out whole rather than cut; `? help` is the last to go.

### `attachments`: list attachments

Shows each attachment's id, file name, type and size. Takes `ID` or `--recent`/`-r` like `read`.
Supported by mail.tm, mail.gw, DropMail and Guerrilla.

### `save`: download attachments

| Option | Short | Explanation |
|---|---|---|
| `ID` | | Which message (required). |
| `ATTACHMENTS...` | | Attachment ids or file names to save. All of them if omitted. |
| `--output DIR` | `-o` | Folder to save into. Default: current folder. Existing files are never overwritten; a `(1)` is added instead. Each file is created new (`O_EXCL`, and `O_NOFOLLOW` on Unix), so a link someone put in the folder under the attachment's name is never written through. Names are cleaned: only the last path component, no control or invisible characters, no `: * ? " < > \|`, no leading `.` or `-` (a leading `-` becomes `_`, so `rm *` can't read it as an option), no trailing dots or spaces, Windows device names (`CON`, `NUL`, `COM1.txt`...) get a `_` in front, and at most 240 bytes, keeping the extension. |

An attachment the provider can't serve doesn't stop the others (mail.tm, for one, refuses one
whose name contains `/`): the saved ones are printed, each failure is told on stderr (`failed`
in `--json`), and the exit code is the first failure's. A provider or disk failure stops the
rest; before anything is saved it is reported as a plain error.

```sh
tmpmail save 1 -o ~/Downloads
```

### `delete`: delete messages

| Option | Explanation |
|---|---|
| `ID` | The message to delete. |
| `--all` | Delete every message in the inbox. |

Supported by mail.tm, mail.gw, Guerrilla and Maildrop.

### `whoami`: show the address

Prints the active address. `--json` adds provider, label, creation and expiry time.

### `copy`: copy the address

Copies the active address to the clipboard. Old flag: `-c`.

### `qr`: QR code

Draws the active address as a QR code in the terminal, so you can scan it with your phone.
With `--json` it prints `{"address": ...}` instead of the drawing.

### `domains`: list domains

Lists the domains you can use with `new -d`. Old flag: `-d` (as the first argument).

| Option | Short | Explanation |
|---|---|---|
| `--provider ID` | `-p` | Which provider to ask. Default: the first one in your fallback order (mail.tm). |

### `inboxes`: list your saved inboxes

`*` marks the active one. Shows the label, provider and when it expires.

| Option | Explanation |
|---|---|
| `--prune` | Forget inboxes that have already expired. |

### `use`: switch the active inbox

`tmpmail use github` makes that inbox the default for later commands. `ADDRESS` can be the
address, the local part or the label.

### `login`: attach an existing account

Adds an account you already have, e.g. one created on the mail.tm website or on another machine.

| Option | Short | Explanation |
|---|---|---|
| `ADDRESS` | | The full address. |
| `--password PW` | | Its password (required for mail.tm/mail.gw). Also `TMPMAIL_PASSWORD`. |
| `--provider ID` | `-p` | `mailtm` (default), `mailgw`, or `maildrop` (chosen automatically for `@maildrop.cc`; no password needed, and the same warning as `new` that its inboxes are public). |
| `--label NAME` | `-l` | A nickname, as in `new`. Without it, logging in again to a stored address keeps its label. An address whose part before `@` is another inbox's label is refused (exit 64), as in `new -u`. |

### `drop`: delete an inbox

Deletes the inbox at the provider (where possible) and removes it from tmpmail.

| Option | Explanation |
|---|---|
| `ADDRESS` | Which inbox. Default: the active one. |
| `--local` | Only forget it here; leave the account at the provider. |

### `rotate`: get a fresh address

Creates a new address (same provider if it is up), keeps the old label, makes it active,
prints it, and only then drops the old inbox. With `--keep`, the label moves to the new inbox and the old one keeps
none, so the label still picks one inbox.

| Option | Short | Explanation |
|---|---|---|
| `--keep` | | Keep the old inbox instead of dropping it. |
| `--copy` | `-c` | Copy the new address to the clipboard. A failed copy only warns, as in `new`. |

### `providers`: provider overview

Lists every provider with its position in the fallback order (`#1`… or `--` if not used
by `new`). It also shows what each one supports:
- `PUBLIC`: anyone can read the inbox
- `custom-user`: you can choose the part before `@` (`new -u`)
- `domain`: you can choose the domain (`new -d`)
- `attachments`
- `raw`: `read --raw` can print the message's source
- `delete`: messages can be deleted
- `login`: an existing inbox can be attached with `login`
- how long addresses live

| Option | Explanation |
|---|---|
| `--check` | Contact each provider now and show `ok` with the response time, or `DOWN` with the reason. |

### `mcp`: server for AI agents

Runs a [Model Context Protocol](https://modelcontextprotocol.io) server on stdin/stdout, so
an AI agent can create inboxes and wait for verification mail. Register it with your client,
for example Claude Code:

```sh
claude mcp add tmpmail -- tmpmail mcp
```

or, in a client's JSON config:

```json
{ "mcpServers": { "tmpmail": { "command": "tmpmail", "args": ["mcp"] } } }
```

| Tool | What it does |
|---|---|
| `create_inbox` | New address, with provider fallback. Optional `provider`, `user`, `domain`, `label` (refused, as in `new -l`, if another inbox has it; a `user` that is another inbox's label is refused too, as in `new -u`). Does not change your active inbox if you have one. |
| `list_inboxes` | Your saved inboxes, as in `inboxes --json`. |
| `list_messages` | Messages in an inbox, newest first. |
| `read_message` | One message as text, with ranked links and attachment names (`include_html` adds the HTML). |
| `wait_for_message` | Blocks until a message matching `from` / `subject` / `body` (regexes) arrives. |
| `get_verification_code` | The OTP in message `id`, or waits for a message that has one. Optional `regex`. |
| `get_links` | Links in message `id`, or waits for a message with a link matching `match`. |
| `delete_message`, `delete_inbox` | Clean up. |

Every tool takes `inbox` (address, local part or label; default: the active inbox). The waiting
tools take `timeout` (default 120 s, at most 600 s, since clients give up on long calls) and
`include_existing`, which is **true** by default here, unlike the CLI: the agent usually
triggers the mail before it calls the tool, and must not miss it. As in the CLI, the newest
matching message already there wins. Pass `false` to wait for new mail only.

An argument a tool doesn't list in its schema (`tools/list`, where each schema has
`"additionalProperties": false`) is refused with a tool error (`isError`, kind `usage`) that
names it and lists the ones the tool takes, so a misspelled `username` for `user` isn't
quietly dropped. Omitted or `null` `arguments` mean none, a `null` argument is the same as leaving it
out, and `arguments` that aren't an object (a list) are refused.

Mail comes from anyone who knows the address. The server tells the client to treat message
contents as untrusted data, but an agent that acts on mail can still be steered by it.

Each tool call runs on its own thread, so a long wait doesn't hold up other calls, and a
client's `notifications/cancelled` stops a running wait within a quarter of a second.

### `man`: manual pages

```sh
tmpmail man | man -l -                     # read it now
tmpmail man -o ~/.local/share/man/man1     # install tmpmail.1 and tmpmail-COMMAND.1
```

| Option | Short | Explanation |
|---|---|---|
| `--output DIR` | `-o` | Write one page per command into this folder instead of printing the main page. If the folder can't be created or written, the error names it (exit 1). |

### `completions`: shell completion

```sh
tmpmail completions bash > ~/.local/share/bash-completion/completions/tmpmail
tmpmail completions zsh  > ~/.zfunc/_tmpmail
tmpmail completions fish > ~/.config/fish/completions/tmpmail.fish
```

They complete commands and options, and the provider ids after `-p` (`new`, `domains`,
`login`, which offers only the providers it can log in to). An unknown id after `-p` is
refused with the list of known ones (exit 64).

---

## Short flags at a glance

The same letter can mean different things in different commands.

| Flag | Where | Meaning |
|---|---|---|
| `-i` | everywhere | `--inbox`: which inbox |
| `-q` | everywhere | `--quiet` |
| `-b` | everywhere | `--browser` |
| `-h` / `-V` | everywhere | help / version |
| `-a` | `ls`, `wait`, `otp`, `watch` | `--all-inboxes`: every saved inbox, not just the active one |
| `-u` | `new` | `--user`: username |
| `-d` | `new` | `--domain` |
| `-p` | `new`, `domains`, `login` | `--provider` |
| `-l` | `new`, `login` | `--label` |
| `-c` | `new`, `rotate`, `watch` | `--copy` |
| `-r` | `read`, `otp`, `links`, `attachments` | `--recent`: newest message |
| `-t` | `wait`, `otp` | `--timeout` in seconds |
| `-w` | `otp` | `--wait` |
| `-m` | `links` | `--match` |
| `-n` | `watch` | `--notify` |
| `-o` | `save`, `man` | `--output` folder |

## Original tmpmail flags

These still work when used as the **first** argument, so old scripts keep running:

| Old | Same as |
|---|---|
| `tmpmail` | `tmpmail ls` |
| `tmpmail -g` / `--generate` | `tmpmail new` |
| `tmpmail -g name@domain` | `tmpmail new name@domain` |
| `tmpmail -r` / `--recent` | `tmpmail read --recent` |
| `tmpmail ID` | `tmpmail read ID` |
| `tmpmail -t ID` / `--text` | `tmpmail read ID` (text is already the default) |
| `tmpmail -d` / `--domains` | `tmpmail domains` |
| `tmpmail -c` / `--copy` | `tmpmail copy` |
| `tmpmail -b w3m -r` | `tmpmail -b w3m read --recent` |

## Environment variables

| Variable | Explanation |
|---|---|
| `TMPMAIL_PROVIDER` | Fallback order for `new`, comma-separated, e.g. `guerrilla,mailtm`. |
| `TMPMAIL_INBOX` | Default inbox, like `-i`. |
| `TMPMAIL_TIMEOUT` | Default `wait` timeout in seconds. |
| `TMPMAIL_POLL_INTERVAL` | Seconds between checks in `wait`/`watch`. |
| `TMPMAIL_PASSWORD` | Password for `new --password` / `login`. Keeps it out of your shell history. |
| `TMPMAIL_STATE_DIR` | Where inboxes are stored. Default `~/.local/state/tmpmail`. |
| `TMPMAIL_CONFIG` | Config file path. Default `~/.config/tmpmail/config.toml`. Unknown keys in it are ignored with a warning on stderr (not with `-q` or `--json`); a known key with a bad value is an error. tmpmail 0.2.0 and earlier refuse unknown config keys; 0.3.0 and later warn. |
| `TMPMAIL_DROPMAIL_TOKEN` | Your own DropMail API token, if DropMail asks for a captcha. |
| `TMPMAIL_PUSH` | `0` turns off push in `wait`/`watch` (see [`wait`](#wait-wait-for-a-message)). Default `1`. |
| `PAGER` | Pager for long messages in `read`, unless `pager` is set in the config. Default `less` (on Windows, printing directly when `less` isn't installed). |
| `TMPMAIL_HTTP_TIMEOUT` | Seconds before a single request to a provider gives up. Default 20, or `http_timeout` in the config file (new in 0.3.0). Like `http_timeout`, it must be a whole number from 1 to 86400; anything else is a usage error (exit 64). |
| `NO_COLOR` | Disable colors. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Other error (e.g. cannot write a file) |
| 2 | `wait` / `otp --wait` timed out |
| 3 | Provider problem: unreachable, rate limited, login failed, bad response |
| 4 | Nothing found: no inbox yet, no such message, no code or link in the message |
| 64 | Wrong usage: bad option, or the provider doesn't support it |
| 141 | Output pipe closed early (e.g. `| head`); normal Unix behaviour |
