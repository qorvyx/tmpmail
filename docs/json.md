# JSON output

Every command accepts `--json`. Output goes to stdout. Timestamps are RFC 3339 in UTC,
and absent values are `null`. Fields may be added later but won't be renamed or removed
within a major version.

Strings are written so that the output is plain to a terminal: every control character
(C0, DEL and the C1 range U+0080–U+009F), bidi control (U+202A–U+202E, U+2066–U+2069,
U+200E/F, U+061C), zero-width and other invisible format character, the line and paragraph
separators U+2028/U+2029 and the Unicode tag characters is written as a `\uXXXX` escape
(a surrogate pair past U+FFFF), whether stdout is a terminal or not. A JSON parser reads the
same values as before: `"\u202e"` is the character U+202E. The MCP server writes its
replies the same way.

## Errors (any command)

```json
{"error": {"kind": "unreachable", "message": "mailtm: unreachable (HTTP 502)"}}
```

Usage errors that come from the command line itself (an unknown option, a bad value,
conflicting options) are reported the same way when `--json` is on it, with kind `usage`.
`--help` and `--version` stay plain text.

`kind` is one of `unreachable`, `rate_limited`, `auth`, `bad_response`, `not_found`,
`unsupported`, `address_taken`, `expired`, `no_inbox`, `timeout`, `usage`, `error`.
The exit code matches (see the README).

## Inbox: `new`, `whoami`, `use`, `login`, `rotate`; `inboxes` returns a list of these

```json
{
  "address": "quietotter482@maxxspace.com",
  "provider": "mailtm",
  "label": "github",
  "created_at": "2026-10-05T10:00:00Z",
  "expires_at": null,
  "expired": false,
  "active": true,
  "private": true
}
```

Credentials are never printed.

## Message summary: `ls` and `watch` lines

`ls` returns `{"address": "...", "created_at": "RFC 3339", "messages": [Summary, ...]}`, newest first.

`ls --all-inboxes` returns `{"inboxes": [{"address", "created_at", "messages": [...], "error"?}, ...]}`.
`error` (`{"kind", "message"}`) is present only for an inbox that couldn't be read. When none
could, the document is printed the same way and the exit code is the first inbox's error's
(3 for an unreachable provider).

`from` and `subject` (and, in a full message, `to` and attachment file names) are decoded
the same way with every provider: RFC 2047 encoded-words such as `=?utf-8?q?Zo=C3=AB?=`
come out as text. `intro` has no line break at its end.

`seen` is true once the message was opened with `read`, in the TUI or in the desktop app, or
when the provider marks it read itself.

`watch --json` prints one Summary per line as messages arrive, plus an `"inbox"` field
with the receiving address. With `--rotate`, each switch to a new address prints a line
`{"rotated": {"from": "old@...", "to": "new@..."}}`; tell the two apart by the `rotated` key.

```json
{
  "id": "6aa6c0...",
  "from": "GitHub <noreply@github.com>",
  "subject": "Your verification code",
  "intro": "Use 482913 to ...",
  "date": "2026-10-05T10:01:12Z",
  "seen": false,
  "has_attachments": false
}
```

## Message: `read`, and `wait` without `--otp` or `--link`

`wait` adds an `"inbox"` field with the receiving address (useful with `--all-inboxes`).
It prints exactly one document: `wait --delete` on a provider that can't delete messages is
refused with an `unsupported` error before waiting, and when a delete fails after the message
was printed, the failure goes to stderr (no `error` document follows) and sets the exit code.

```json
{
  "id": "6aa6c0...",
  "from": "GitHub <noreply@github.com>",
  "to": ["quietotter482@maxxspace.com"],
  "subject": "Your verification code",
  "date": "2026-10-05T10:01:12Z",
  "text": "Use 482913 to sign in.",
  "html": "<p>Use <b>482913</b> ...</p>",
  "attachments": [
    {"id": "ATTACH000001", "filename": "invoice.pdf", "content_type": "application/pdf", "size": 48213}
  ]
}
```

Attachment `size` is in bytes, or `null` when the provider doesn't say. mail.tm and mail.gw
only report whole kilobytes, so their sizes are rounded up to a multiple of 1024.

mail.tm and mail.gw cut `text` and `html` at 100 KiB (102 400 bytes). tmpmail then reads the
message's raw source, which is whole, and gives its text and HTML instead. If the source
can't be read (or has no longer body it can decode), `text` ends with a line saying it may
be cut and that `read --raw` has all of it (`html` with a paragraph saying so).

## Other commands

| Command | Output |
|---|---|
| `otp` | `{"code", "candidates": [{"code", "score"}], "inbox", "message_id", "from", "subject"}` |
| `wait --otp` | `{"code", "candidates": [{"code", "score"}], "inbox", "message_id", "from", "subject"}` |
| `wait --link` | `{"link", "inbox", "message_id"}` |
| `links` | `{"links": [...], "message_id"}` |
| `attachments` | `{"message_id", "attachments": [...]}` |
| `save` | `{"saved": ["path", ...]}`, plus `"failed": [{"id", "filename", "error": {"kind", "message"}}]` when some attachments couldn't be saved and the others were tried (`filename` is the name it would have been saved as) (the exit code is then the first failure's, and no separate `error` document follows) |
| `delete` | `{"deleted": N}` |
| `drop` | `{"dropped": "address", "active": "address or null"}` |
| `copy` | `{"copied": "address"}` |
| `qr` | `{"address": "address"}` (no drawing) |
| `domains` | `{"provider", "domains": [...]}` |
| `providers` | `[{"id", "name", "website", "in_fallback_order", "capabilities": {...}, "status": {"ok", "ms"} \| {"ok": false, "error"} \| null}]` |

`candidates` lists the heuristic matches; it is empty when `--regex` / `--otp-regex` picked the code.
