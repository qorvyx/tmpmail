tmpmail desktop 0.1.2 fixes what a hands-on test of 0.1.1 on Kubuntu (KDE Plasma) found.
It is built on the same core as the `tmpmail` command-line tool (0.3.2) and shares its
inboxes and `config.toml`.

**This is a preview.** On Linux, 0.1.2's rpm was tested by hand on Fedora with KDE Plasma
(X11 and Wayland) and GNOME (Wayland); the deb and the AppImage come from the same build but
weren't tried by hand. On Windows only 0.1.0 was tried by hand, on a clean Windows 11, and
macOS has only been checked by the automated security probe. Please report what breaks:
<https://github.com/qorvyx/tmpmail/issues>.

## What's new in 0.1.3

- **Names and subjects show decoded.** Mail from tempmail.lol showed encoded sender names
  and subjects such as `=?utf-8?q?Zo=C3=AB?=` in the message list, the message header and
  notifications; they now read `Zoë`, as with the other providers (from CLI 0.3.3's core).
- **A label picks one inbox.** The new inbox dialog accepted a label another inbox already
  had, or a username that was another inbox's label, and then the CLI's `-i LABEL` matched
  neither or both. These are now refused before anything is created, with the reason under
  the label or username field; the dialog stays open so you can change it.
- **A clearer error when a domain isn't offered** while one provider is down: the
  unreachable provider is named first, then what the others answered.
- The TUI crash in short terminals fixed in CLI 0.3.3 doesn't affect the app.

## What's new in 0.1.2

- **Linux: the window shows the tmpmail icon** in the title bar and the task switcher. KDE
  on Wayland showed its generic "W" icon, since the window's app ID (`tmpmail-gui`)
  matched no desktop file; it is now `tmpmail`, like the desktop file.
- **Linux: clicking a notification brings the window to the front** on KDE. The message
  opened, but KWin only flashed the taskbar entry; the app now passes on the activation
  token the notification server sends with the click.
- **Keyboard shortcuts keep working after a click inside the mail.** They stopped until
  you clicked elsewhere in the app. Selected mail text is now copied with right-click →
  Copy, since Ctrl+C goes to the app.
- **Linux: notifications are labeled "tmpmail"**, not "tmpmail-gui", and KDE and GNOME
  list them under the app's notification settings.
- **Linux: in KDE's notification history, use the "View" button** to open a message;
  clicking the entry's text does nothing there (Plasma's behavior). The click action is
  now named "Open" for servers that list actions.
- **No more "New mail" notice for the message a notification click just opened**: the
  in-app notice for it now goes away; notices about other new mail stay.
- **Linux on Wayland: the window fits the screen.** It opened at 1240×780 whatever the
  screen, so on a 1280×800 screen its bottom (Settings' Save button, the toasts) was behind
  KDE's panel: GTK can't read the work area there. It now leaves room for the panels.

## What's new in 0.1.1

- **Windows: clicking a notification opens the message**, also from the notification
  center after the toast is gone. The installed app shows its own toasts, which launch a
  `tmpmail-desktop:` link the installer registers; the app restores its window (or starts)
  and opens that inbox and message. Starting the app a second time now brings back the
  one that runs.
- **Opening a message marks it read**: the unread dot and the sidebar count update at once,
  and the mark is saved with the inbox, so `tmpmail ls` and the TUI agree (with CLI 0.3.2).
- **Small screens:** the window opens within the screen's work area (it was 1240×780
  whatever the screen), and every dialog keeps its title and buttons in view with a
  scrolling body. Settings' Save button was off-screen on a 720-pixel-high screen.
- **"Live" stays on for mail.tm**: the push connection was dropped and redone every 75
  seconds, so "Live" blinked off (after new mail, among other times). It now stays up, and
  a quick reconnect doesn't hide it.
- **Delete confirmations open on "Keep it"**, not on the red button, so a stray Enter or
  Space right after `d` or `D` cancels instead of deleting. Press `y` (as in the TUI) or
  click the red button to delete.
- **The Windows installer** shows the tmpmail icon, header and sidebar instead of NSIS's.

## What it does

- Inboxes at mail.tm, mail.gw, DropMail, Guerrilla Mail, tempmail.lol and Maildrop: create
  one (the first provider that works, or one you pick, with a username and domain where the
  provider allows it, and a label), copy the address, make it the CLI's active inbox, delete
  it.
- Live mail on the same worker as `tmpmail tui` (polling, plus push on mail.tm and
  DropMail). New mail is marked New.
- The verification code in a banner with a Copy button, and the likeliest verification link
  first, each with Open and Copy.
- A safe HTML preview, in light or dark (mail with its own colors is inverted, with an
  "Original colors" switch). Remote images load only on request, over https only.
- Attachments: Save, or drag them out on Linux and macOS.
- Notifications for new mail while the window is in the background, with the code when
  there is one. A click opens the message.
- An opt-in tray icon (New address, Copy code, Quit) that keeps checking with the window
  closed.
- A settings screen for the provider order, poll interval, push, request timeout and the
  desktop options.
- The TUI's keyboard shortcuts (`?` lists them).

## Download

| System | File |
|---|---|
| Windows 10/11 (x64) | `tmpmail-desktop-0.1.2-x64-setup.exe` |
| macOS 10.15+ (Apple silicon and Intel) | `tmpmail-desktop-0.1.2-universal.dmg` |
| Debian, Ubuntu (x86_64) | `tmpmail-desktop-0.1.2-amd64.deb` |
| Fedora and other RPM-based (x86_64) | `tmpmail-desktop-0.1.2-x86_64.rpm` |
| Other Linux (x86_64) | `tmpmail-desktop-0.1.2-x86_64.AppImage` |

Check a download against `SHA256SUMS`, or its build attestation:

```sh
gh attestation verify tmpmail-desktop-0.1.2-x86_64.AppImage --repo qorvyx/tmpmail
```

## Installing an unsigned app

The app isn't signed with a paid certificate, so each system warns once.

- **macOS:** open the DMG and drag tmpmail to Applications. Open it once: macOS refuses.
  Then go to System Settings → Privacy & Security, click **Open Anyway** next to the
  message about tmpmail, and enter your password. On macOS 15 (Sequoia) and later,
  Control-click → Open no longer skips this. If you trust the download, `xattr -dr
  com.apple.quarantine /Applications/tmpmail.app` does the same from a terminal.
- **Windows:** SmartScreen says "Windows protected your PC": click **More info**, then
  **Run anyway**. The installer installs for your user only, with no admin prompt, and
  fetches WebView2 if it's missing. If **Smart App Control** is on, it blocks the installer
  outright with no way past; it can only be turned off as a whole (Windows Security → App &
  browser control).
- **Linux:** `sudo apt install ./tmpmail-desktop-0.1.2-amd64.deb`, or `sudo dnf install
  ./tmpmail-desktop-0.1.2-x86_64.rpm`; both pull in WebKitGTK 4.1. The AppImage runs
  as is after `chmod +x` (it may need `libfuse2`; without FUSE, run it with
  `--appimage-extract-and-run`). On NVIDIA, if the window stays blank, start it with
  `WEBKIT_DISABLE_DMABUF_RENDERER=1`.

**Use tmpmail CLI 0.3.2 or later alongside the app.** The settings screen can write the
`http_timeout` key to `config.toml`, which tmpmail 0.2.0 refuses, and a CLI older than 0.3.2
drops the app's read marks when it rewrites an inbox.

## Known limits

- No auto-update yet: watch the releases page.
- Windows: no dragging attachments out (a drag can hang the app there); Save works.
- Linux: the tray needs a StatusNotifier host (stock GNOME needs the AppIndicator
  extension), and clicks on the tray icon itself aren't reported, so everything is in its
  menu. Copying on pure Wayland needs a compositor with clipboard data-control support.
- macOS: the notification permission prompt and the tray have not been tried on a real
  Mac. An ad-hoc signed app may be asked for permissions again after each update.
- x86_64 only on Linux and Windows for now.
