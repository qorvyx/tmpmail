<p align="center"><img src="src/assets/logo.svg" alt="" width="72" height="72"></p>

# tmpmail desktop app (preview)

A Tauri 2 window on top of tmpmail's core library, with the same inboxes as the CLI (same
`state.json` and config). Inboxes, live mail, the verification code one click (or `y`) away,
a safe HTML preview in light or dark, and the TUI's actions and keys. Released as a
preview on `gui-v*` tags; install steps are in the [main README](../README.md#desktop-app-preview).

## Features

- **Inboxes:** create one (provider or "first that works", a username where the provider
  allows it, a domain from the provider's list, a label; a label another inbox has, or a
  username that is another inbox's label, is refused before anything is created, with the
  reason under that field and the dialog left open), copy the address (click it, the
  Copy button or `c`), make it the active inbox (the CLI's default, marked with a star),
  check now, and delete it (asks first; deleted at the provider where it can be, then
  forgotten here). Each inbox shows its provider, expiry or lifetime, `Live` when a push
  stream is up (it stays on through a quick reconnect), and "Public" for Maildrop.
- **Live mail:** the same worker loop as the TUI (`tmpmail::watcher`: polling, push
  streams, state reloads so CLI changes show up within ~5 s). New mail is marked New, and a
  notice names the sender and subject with an Open button. A notice about one message goes
  away once that message is opened (by a notification click, the list or the keyboard; one
  that also names other new mail stays); mail already read when it is listed (a
  notification click opened it first) gets neither the notice nor the mark.
- **Messages:** unread dot (opening a message marks it read, saved in `state.json` so the
  CLI's `ls` and the TUI agree; the sidebar badge counts the rest), sender, subject, time and
  attachment icon; filter by sender or subject (`/`); delete (asks first) where the provider supports it.
- **The open message:**
  - The verification code, when `extract` finds one, in a banner at the top with a Copy
    button.
  - Links, the likeliest verification link first, each with Open (system browser, http and
    https only) and Copy.
  - From (copy the sender's address), to and date.
  - Load images (remote images, over **https only**), Original colors in dark mode, and
    Source (the raw message where the provider has it, else the HTML).
  - Attachments with name, type and size, and Save (a native save dialog; never overwrites
    an existing file). On Linux and macOS an attachment can also be dragged out by its grip
    to a folder or another app (see "Dragging attachments out").
- **Notifications:** new mail while the window isn't focused (or is hidden in the tray)
  shows a system notification with the sender and subject, and the code when the subject or
  preview has one ("Code 482913 from GitHub"). Mail arriving within 2 s is grouped into one
  notification ("Code 482913 from GitHub, and 2 more", or "3 new messages"). Clicking it
  brings the window back and opens the message (the one with a code, else the newest).
  Sender and subject are cleaned of control and bidi characters, capped (48 and 100
  characters) and, on Linux servers that read markup, escaped. The first listing of an
  inbox never notifies. Turned off in the settings. Permissions: Linux and Windows ask for
  none; on macOS the system decides whether to ask when the first one is shown (not tested
  yet; System Settings → Notifications turns them on or off), and debug builds started with
  `tauri dev` post as Terminal. Clicks are reported on Linux (servers with the `default` action, e.g.
  KDE, GNOME) and on macOS. On Linux the app talks to the notification server over D-Bus
  itself (not through the plugin), to use the `ActivationToken` the server sends with a
  click (Notifications spec 1.2+, KDE Plasma and GNOME): the token goes to GTK, which hands
  it to the compositor (`xdg_activation_v1` on Wayland; on X11 its `_TIME` part becomes
  the window's user time), so KWin's focus-stealing prevention lets the window come to the
  front instead of only flashing its taskbar entry. The token is taken only from the server
  that showed the notification, only as printable ASCII of at most 512 bytes, and used
  once; without one (an older server) the window is raised as before. The `default`
  action (a click on the notification) is labeled "Open", for servers that list actions
  (such as dunst's menu; not tested). KDE's notification history shows it as a button with
  Plasma's own label, "View" (seen on Plasma 6.6), and a click on a history entry's text
  does nothing there, so use that button; GNOME doesn't show it. The notifications are sent as `tmpmail`, with the `desktop-entry` hint `tmpmail` (the installed
  `tmpmail.desktop`), so KDE and GNOME label them "tmpmail" with its icon and file them
  under the app's own notification settings (0.1.1 sent the binary's name, `tmpmail-gui`;
  the desktop file has `X-GNOME-UsesNotifications=true`, so GNOME lists the app there before
  its first notification). The AppImage installs no desktop file by itself, so there the
  hint only helps once a tool like AppImageLauncher has integrated it. On Windows the installed app shows its own toasts, which launch a
  `tmpmail-desktop://open?inbox=…&message=…` link the installer registers: a click, on
  screen or later in the notification center, brings the running app back (only one runs:
  a second start hands its link over and exits) or starts it, and opens the message. A
  debug build, or one the installer didn't register the link for, has no app id for Windows, so its toasts go through the plugin
  and a click counts only while the toast is on screen. Windows shows the app's name only
  for the installed app.
- **Tray icon (opt-in):** turned on in the settings. Its menu has Show tmpmail, New address
  (the first provider that works; the address is copied and a notification says so), Copy
  code (the newest code of the last 15 minutes, across inboxes; greyed out otherwise) and
  Quit. While it is on, closing the window hides it and mail is still checked; "Start
  hidden in the tray" skips the window at launch. Linux reports no clicks on the icon
  itself, so everything is in the menu (on Windows a click shows the window). Linux needs a
  StatusNotifier host: KDE and most desktops have one, **stock GNOME needs the AppIndicator
  extension**. Without a host the settings say so and closing the window still quits, so
  the app can't vanish. On Linux the icon's title (what KDE shows for it) is `tmpmail`, which
  libappindicator takes from the GTK program name; the tray library offers no tooltip
  there. macOS gets a
  monochrome template icon.
- **Settings** (gear in the sidebar footer, or `Ctrl+,` / `Cmd+,`): theme, provider order
  (turn providers on and off, move them up and down; the fallback order is spelled out),
  poll interval (2–3600 s), live push, request timeout (1–600 s), notifications, tray and
  start hidden. Rust checks every field again (known providers, no duplicates, at least
  one on, the bounds). Saved settings apply at once: the worker gets a new poll interval,
  push setting and timeout without a restart, and the tray appears or goes. Environment
  variables that win over a field (`TMPMAIL_PROVIDER`, `TMPMAIL_POLL_INTERVAL`,
  `TMPMAIL_PUSH`, `TMPMAIL_HTTP_TIMEOUT`) are named under it. "Show in folder" reveals the
  config file. Saving only restarts polling (and the push streams) when `config.toml`
  changed. `http_timeout` is new in tmpmail 0.3.0. tmpmail 0.2.0 and earlier refuse unknown
  config keys; 0.3.0 and later warn. So it is written only once the timeout is changed
  from 20 s, and the screen says so. Unknown keys in `config.toml` show as an info notice
  (`tmpmail: unknown key …, ignored`) at start, and after a save only for keys that are new
  since the last load.
- **Look:** System / Light / Dark switch (remembered), resizable panes (remembered),
  keyboard focus rings, motion only on arrival of mail and dialogs, none with
  `prefers-reduced-motion`. The window opens at 1240×780, or smaller and centered when the
  primary screen's work area can't hold that, title bar included; its minimum size follows
  the screen it is on. Where GTK can't tell the work area (always on Wayland; on X11 when the
  window manager doesn't publish one), 80 px are left for panels on each axis (a 1280×800
  screen gets 1184×672). Wayland has no primary screen either: the window then fits the
  narrowest and shortest screen, and the compositor places it.
  Every dialog fits the window: its body scrolls while the title and buttons stay in view.

What each provider allows decides what's shown: Delete (message) is hidden for DropMail and
tempmail.lol; Attachments for tempmail.lol and Maildrop; the domain picker for tempmail.lol
and Maildrop; the username field for DropMail and tempmail.lol; Source shows the raw message
on mail.tm, mail.gw, DropMail and Maildrop and the HTML elsewhere. Deleting an inbox at a
provider that can't (DropMail, tempmail.lol, Maildrop) only forgets it here.

### Keyboard shortcuts

The same keys as `tmpmail tui`, off while you type in a field or a dialog is open. `?`
shows them in the app. They keep working after a click inside the mail: the mail frame
can't run scripts, so it couldn't pass keys on, and the app takes the keyboard focus back
as soon as a click puts it there. Links in the mail still open on click, the mail still
scrolls with the wheel or its scrollbar, and text can still be selected; to copy selected
mail text, right-click it and choose Copy (Ctrl+C / Cmd+C goes to the app, which says so).
The frame is out of the Tab order (its web links are all in the Links list), so Tab and
Shift+Tab move past it, and Esc takes the focus off the mail. The trade: the mail can't be
scrolled from the keyboard (Space, Page Down; the arrow keys are the app's), only with the
wheel or the scrollbar.

| Key | Action |
|---|---|
| `j` `↓` / `k` `↑` | Next / previous message (opens it) |
| `[` / `]` | Previous / next inbox |
| `n` | New inbox at the first provider that works |
| `N` | New inbox, choose the provider |
| `y` | Copy the verification code |
| `L` / `O` | Copy / open the first link |
| `c` | Copy the address |
| `a` | Make the inbox the active one |
| `r` | Check for mail now |
| `d` / `D` | Delete the message / the inbox (asks first: the dialog opens on "Keep it", so Enter or Space cancels; `y` or a click deletes) |
| `/` | Filter messages |
| `?` | Shortcut help |
| `Ctrl+,` / `Cmd+,` | Settings |

## Where settings are kept

- The provider order (`providers`), `poll_interval`, `push` and `http_timeout` are the CLI's
  own keys, in the same `config.toml` (`~/.config/tmpmail/config.toml`, or
  `$TMPMAIL_CONFIG`), so the CLI and the app agree. The app edits the file with
  `toml_edit`: your comments, the other keys and their order stay; a field still at its
  default isn't added; the file is replaced in one step (temporary file, then rename) with
  its permissions kept, and only if a field changed. A config the CLI couldn't read is
  never written.
- Notifications, the tray and start hidden are the app's own, in `gui.json` next to
  `config.toml`. They aren't a `[gui]` table in `config.toml` because tmpmail 0.2.0 and
  earlier refuse unknown config keys (0.3.0 and later warn), so such a table would stop
  them from starting.
- The theme and pane widths stay in the webview's own storage, as before.

## Dragging attachments out

Best effort, on Linux and macOS. It uses `drag` 2.1.1, the engine of the third-party
`tauri-plugin-drag` (CrabNebula), from Rust: the plugin's own command takes any file path
from the page, and the page never names a path, only "attachment N of the opened message".
When the pointer reaches an attachment's grip, Rust downloads it into a private temporary
directory (`tmpmail-drag-*`, mode 0700, one 0700 subdirectory per attachment, the file
0600 under its sanitized name); dragging the grip then drags that file. Files are deleted
10 minutes later (when another is prepared) and when the app quits. Not on Windows, where
a drag can hang the whole app (drag-rs #94). Save stays the dependable way.

## Dark mail

The theme reaches the mail frame only through its URL (`?theme=dark`), never through a
script, and Rust picks the colors when it builds the document:

- Plain-text mail, and HTML mail that sets no colors of its own: drawn in the dark theme's
  own background and text colors.
- HTML mail with its own colors (`bgcolor`, or `color` / `background` in a `style`,
  detected in the sanitized HTML by `render::has_own_colors`): drawn as mail clients do,
  with `filter: invert(1) hue-rotate(180deg)` on the document and the same filter again on
  `img`, `picture`, `video`, `svg` and anything with a background image, so photos and logos
  keep their colors. The page behind it is set so it inverts to the app's dark surface.
- "Original colors" (per message) shows it as the sender designed it, on white.

## Setup

- Rust 1.90 or newer (the root CLI crate still builds with 1.85).
- Node 24, and pnpm through corepack: `corepack enable --install-directory ~/.local/bin pnpm`
  (user-level), or prefix every command with `corepack`.
- Linux: WebKitGTK 4.1 and friends. Debian/Ubuntu:
  `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`; Fedora:
  `webkit2gtk4.1-devel librsvg2-devel libappindicator-gtk3-devel`. On NVIDIA, a blank window
  is fixed by `WEBKIT_DISABLE_DMABUF_RENDERER=1`. On pure Wayland, copying needs a
  compositor with clipboard data-control support.

```sh
cd gui
pnpm install          # lockfile, exact versions, only releases older than 7 days
pnpm tauri dev        # Vite on :1420 plus the app, reloading on changes
pnpm check            # svelte-check
pnpm lint             # eslint, with svelte/no-at-html-tags as an error
pnpm tauri build --debug --no-bundle   # target/debug/tmpmail-gui with the built frontend
pnpm tauri icon src/assets/logo.svg    # regenerate src-tauri/icons (keep only the desktop ones)
```

`TMPMAIL_STATE_DIR` and `TMPMAIL_CONFIG` work as for the CLI (if either can't be read, the
window says why instead of starting), and `TMPMAIL_GUI_LOG=1` prints every request to the
mail scheme and every navigation.

The interface uses the system UI font for body text and headings (no font is bundled), and
the system monospace font for the verification code. Icons are inline SVG in
`src/lib/Icon.svelte`, drawn after Lucide's (ISC), so no icon package is needed.

## Installers and releases

The app has its own version, independent of the CLI's, in three files that must agree:
`src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json` (then
`cargo metadata` at the root refreshes `Cargo.lock`).

Build the installers for the system you're on (release build, no probe):

```sh
CARGO_BUILD_JOBS=4 pnpm tauri build                     # Linux: .deb, .rpm, AppImage
pnpm tauri build --target universal-apple-darwin        # macOS: universal .app and .dmg (both rust targets installed)
pnpm tauri build --bundles nsis                         # Windows: -setup.exe
```

They land in the workspace's `target/` (`target/release/bundle/{deb,rpm,appimage}`, or
`target/universal-apple-darwin/release/bundle/dmg`), not under `gui/`. Bundle settings are
in `tauri.conf.json`:

- **Linux:** built on Ubuntu 22.04 for an old glibc. The bundler writes the dependencies
  itself: the `.deb` depends on `libwebkit2gtk-4.1-0`, `libgtk-3-0` and the tray library it
  was built against (`libayatana-appindicator3-1` in CI, which `gui.yml` builds against
  too); the `.rpm` requires the same libraries by their `.so` names
  (`libwebkit2gtk-4.1.so.0`, ...), so it isn't tied to Fedora's package names. The AppImage carries WebKitGTK and the tray library, without
  GStreamer (the app plays no media), and takes GTK, Wayland and GL from the system, as
  AppImages do. The desktop file is `tmpmail.desktop` (from
  `src-tauri/packaging/tmpmail.desktop`: Tauri's template with `StartupWMClass` set to the
  product name, so `StartupWMClass=tmpmail`). The app sets its GTK
  program name to `tmpmail` before GTK starts, so the window's X11 class (`tmpmail`,
  `Tmpmail`) and its Wayland app ID are `tmpmail` too, not the binary's name `tmpmail-gui`.
  That way KWin and Mutter match the window to the desktop file on X11 and Wayland, and
  the title bar, task switcher and docks show the tmpmail icon (KWin on Wayland goes by the
  app ID alone, and showed its generic "W" icon in 0.1.1). The window also carries its own
  128×128 icon (`_NET_WM_ICON` on X11; Tauri takes the first PNG of `bundle.icon`, so
  `128x128.png` comes first; 256×256 would be dropped by X servers without BIG-REQUESTS)
  for window managers that don't read desktop files.
- **License:** every package carries the GPL text as a bundle resource (`LICENSE` next to
  the app: `/usr/lib/tmpmail/LICENSE` on Linux and in the AppImage,
  `tmpmail.app/Contents/Resources/LICENSE`, the install directory on Windows). The `.deb`
  also has a DEP-5 `/usr/share/doc/tmpmail/copyright` (`src-tauri/packaging/copyright`),
  and the `.rpm` has License `GPL-3.0-or-later` and `/usr/share/licenses/tmpmail/LICENSE`.
  `bundle.licenseFile` is left out on purpose: the bundler would attach it to the DMG as a
  click-through agreement (shown on every open) and add an "I accept" page to the NSIS
  installer; the GPL asks for neither.
- **macOS:** one universal app (Apple silicon and Intel), macOS 10.15 or later, **ad-hoc
  signed** (`signingIdentity: "-"`): with no signature on the whole bundle, Apple silicon
  reports the app as "damaged" with no way past. It isn't notarized, so users go through
  Open Anyway once.
- **Windows:** an NSIS installer only (no MSI), per user (no admin prompt), with the
  WebView2 bootstrapper that downloads WebView2 if it's missing. Its hooks
  (`src-tauri/packaging/windows/hooks.nsh`) register the `tmpmail-desktop:` link scheme for
  notification clicks under `HKCU\Software\Classes` and remove it on uninstall. The installer
  and uninstaller show the app icon and tmpmail's own header and welcome-page artwork
  (`src-tauri/packaging/windows/nsis-*.bmp`, drawn from `assets/logo.svg` by
  `scripts/nsis-artwork.sh`).
- **Release builds contain no probe**: `probe.rs` and everything that calls it are
  `#[cfg(debug_assertions)]`, and the release workflow fails if the binary contains
  `TMPMAIL_GUI_PROBE`. The caller check and the closed `postMessage` route are in every
  build.

### Releasing

`.github/workflows/gui-release.yml` runs on a `gui-v*` tag (the CLI's `release.yml` runs on
`v*`, which doesn't match it):

1. Set the version in the three files above, update `RELEASE_NOTES.md` (the release's
   text), commit, then tag `gui-vX.Y.Z` and push the tag.
2. A first job stops the run if the three versions differ, if the tag isn't `gui-v` plus
   that version, or if `Cargo.lock` is stale.
3. Builds on `ubuntu-22.04`, `macos-latest` (universal) and `windows-latest`. A failed
   macOS build is retried once (DMG creation is flaky on runners), and the signature of the
   app inside the DMG is checked with `codesign --verify --deep --strict` (the DMG is
   mounted with any license agreement answered, and a warning is printed if it has one).
4. One publish job writes `SHA256SUMS`, attests every file
   (`gh attestation verify FILE --repo qorvyx/tmpmail`), and creates a **pre-release**
   titled "tmpmail desktop X.Y.Z (preview)", never marked latest (the CLI's release stays
   the latest), with `RELEASE_NOTES.md` as its notes.

Running the workflow by hand (`workflow_dispatch`) is a dry run: it builds and uploads the
installers as run artifacts, without a release. Files are named
`tmpmail-desktop-X.Y.Z-{x64-setup.exe,universal.dmg,amd64.deb,x86_64.rpm,x86_64.AppImage}`.

`scripts/homebrew-cask.sh VERSION SHA256SUMS` prints a cask (`tmpmail-desktop`) for the
DMG, with a caveat explaining Open Anyway, for `qorvyx/homebrew-tap`. It isn't run in CI
yet: the cask waits until the app is out of pre-release. No updater yet either: it would
need its own minisign key, backed up before the first release that uses it.

## The preview probe

Debug builds include a probe of the mail preview's defenses:

```sh
TMPMAIL_GUI_PROBE=1 target/debug/tmpmail-gui          # Linux CI: xvfb-run -a target/debug/tmpmail-gui
```

It renders a hostile test mail (script, `onerror`, an attempt to reach `__TAURI_INTERNALS__`
and `invoke`, a remote image, a `cid:` image, `target="_blank"` and `javascript:` links,
inline styles) through the real preview, in light and in dark mode (where the test mail's
own colors make it inverted), and through control frames that each drop one layer (CSP
header only, sandbox only, scripts allowed, nothing). Anything that runs requests a URL on
the mail scheme, which Rust records. It also checks the sanitizer and the document builder
directly (remote images dropped with images off, `http:` images dropped and `https:` kept
with them on, a `style` keeping its colors but losing its `url(...)`, the dark document
inverted), since the frames can't tell the sanitizer's work from the CSP's. The test mail
also attacks the command caller check as a frame that knows Tauri's invoke key would (the
probe hands it the key): through the bridge where the engine gives frames one, a direct
`fetch` to the IPC protocol, and a `postMessage` call with a forged `Origin`. Every call
that reaches the invoke handler from a frame is recorded. Strict checks: the real frames
ran no code, had no bridge and made no call; the unsandboxed control on the mail origin
(`raw_open`) was refused by the caller check on every engine; no forged call was accepted;
`init` ran once (only the app page's calls get that far). What the controls' frames had
(bridge or not, which calls got through to the handler) is reported under
`observations.controls`, since the controls drop layers on purpose. It prints a JSON report
and exits 0 when every check held (21 checks). `TMPMAIL_GUI_PROBE_WAIT=20` keeps the
window open 20 s more so you can click the test links: popups that reach the app are listed
under `popups_reaching_on_new_window` (and are not opened). The `gui` workflow runs the probe
on Linux, macOS and Windows: with `TMPMAIL_GUI_PROBE_REPORT=probe.json` the report goes to
that file instead of stdout (GLib prints some messages to stdout), the app's output goes
to `probe.log`, both are uploaded, and the job fails unless `probe.json` parses and says
`"pass": true`.

## Security model

1. Mail HTML is sanitized only in Rust (ammonia, `render::sanitize_mail_html`) and never
   enters the app's DOM: lint bans `{@html}`. In the app's frame, a `style` attribute with a
   `url(...)` (or an image function, or a CSS escape) loses just those declarations while
   images are off; the CLI's `read --html` output is unchanged.
2. It is served by the `tmpmail-mail` URI scheme with its own CSP header (no scripts, images
   only inline or from the scheme, `https:` only after "Load images"; the sanitizer drops
   plain `http:` images). Inline `cid:` images are served only as raster image types, never
   SVG. The source view goes through the same scheme as escaped text, under a CSP with no
   images at all.
3. It is shown in `<iframe sandbox="allow-popups">`: no scripts, an opaque origin, no IPC.
   The theme and view options are in the frame's URL; no script ever touches the frame.
4. Links get `target="_blank"`; the app denies every new window and opens only http, https
   and mailto links in the system browser, from Rust. The link list's Open does the same
   for http and https.
5. **Commands accept calls only from the app's page** (`src-tauri/src/caller.rs`, run by
   the invoke handler before any command, so none can skip it). WebView2 injects Tauri's
   bridge (`__TAURI_INTERNALS__`, with the invoke key) into **every frame**, and the mail
   scheme's origin counts as app-local, so without this an unsandboxed frame on the mail
   origin could call every command: in CI (2026-10-31, WebView2 155) such a control frame
   called `init` and it ran. WebKitGTK and WKWebView keep the bridge in the main frame.
   The check needs all of:
   - the main window's webview;
   - its top-level URL, read from the engine, is the app's page;
   - the call's `Origin` header is `tauri://localhost` or `http(s)://tauri.localhost`
     (plus the Vite dev server in debug builds). Calls come over Tauri's IPC protocol, where
     the engine sets `Origin` to the calling frame's origin and no script can change it.

   Tauri's other route, the `postMessage` fallback, takes "headers" from the script, so a
   forged `Origin` would pass there: the probe showed that on WebKitGTK any frame, sandboxed
   or not, can post to it (WebKit keeps only the key out of frames). The app removes that
   route at startup on WebKitGTK and WKWebView (wry's `ipc` script message handler) and
   won't start if it can't. On WebView2 only the top-level document can post (frames don't
   reach `CoreWebView2.WebMessageReceived`), and the top-level URL check covers it; web
   messages are also turned off there, but WebView2 applies that from the next document on.
   The cost: if one of the page's IPC `fetch`es ever failed, Tauri's JS would switch to the
   closed route for good and calls would hang until the window reloads.

   Tauri 2.12 has no way to keep its init scripts out of subframes on WebView2 (wry adds
   them to every frame there), so frames on Windows still see the bridge; the check makes it
   useless. Plugin commands don't go through the app's invoke handler: the page may only
   *listen* to events (`core:event:allow-listen`). Unlisten is not allowed, since a frame
   with the bridge could remove the app's listeners (their ids are sequential); events reach
   only the top-level document, so a frame that listens gets nothing.

   Which layer stops what, for mail content: ammonia removes scripts and handlers; the
   scheme's CSP header blocks any that slip through; the sandbox (no `allow-scripts`, an
   opaque origin) blocks them again and makes any IPC call come from `null`; and if all of
   that failed, the caller check refuses every command from a frame.
6. Privileged work happens in Rust commands: the clipboard (`tauri-plugin-clipboard-manager`),
   the save dialog (`tauri-plugin-dialog`, then `std::fs::write` to a new file under a
   sanitized name), the opener, notifications (`tauri-plugin-notification`; on Linux D-Bus, `freedesktop.rs`), the tray and
   dragging files out. The page has **no** plugin permission, only the app's
   own commands, each listed in `capabilities/main.json` and `build.rs`
   (`AppManifest::commands`).
7. Commands check their input: an address must be a stored inbox, a message key one the app
   opened, a link or attachment index one of that message's; usernames, domains, labels and
   ids are checked for shape. Copying takes what to copy (the code, the sender, link N), not
   arbitrary text. Settings are checked field by field in Rust before anything is written.
8. The shell has a strict CSP, `freezePrototype`, no global Tauri object, no asset protocol,
   and one capability.
9. What the sender or a provider writes is cleaned in Rust before it reaches the page, so
   the page can't forget it: the list rows' sender, subject and preview (and so the "New
   mail" notice, the filter and the delete dialog), the open message's header, attachment
   names, the code (and the tray's "Copy code" label), notices and the provider errors in
   the list, the new inbox dialog and Save go through `render::clean_line`, which drops
   terminal escape sequences whole, control characters, bidi controls (U+202A–U+202E,
   U+2066–U+2069, U+200E/F, U+061C) and the other invisible format characters (zero-width
   joiners and emoji flag tags stay); the links list gets each link as the browser would
   (parsed, so an ASCII string: punycode hosts, escaped paths); the domain picker shows
   only names that look like domains; and an address a provider hands out is refused when
   it isn't one (`checked_address`), so inbox addresses are clean too. A right-to-left
   override in a sender can't turn a row around to show another address (campaign round
   2, B1). Codes are still looked for in the sender's own text, as the CLI does. The mail
   body in the frame is shown as sent (sanitized HTML; right-to-left text needs its
   marks).
10. Windows: the `tmpmail-desktop:` scheme a notification click launches can be opened by
   any program, or a web page the user allows to. The app accepts only
   `tmpmail-desktop://open?inbox=ADDRESS[&message=ID]` (no other key, each once, the shapes
   checked) and only for a stored inbox; it can bring the window back and open that
   message, nothing else. Anything else on the command line is ignored.
