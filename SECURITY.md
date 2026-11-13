# Security policy

## Supported versions

Security fixes go into the latest release of each part:

| Part | Supported |
|---|---|
| Command-line tool and library (`tmpmail`) | the latest 0.3.x |
| Desktop app | the latest 0.1.x |

## Reporting a vulnerability

Please report it privately, not in a public issue: use **Report a vulnerability** on the
[Security tab](https://github.com/qorvyx/tmpmail/security) of this repository. Include what
you found, how to reproduce it, and what an attacker could do with it.

Once it's fixed and released, the report is published as a security advisory, with credit to
you unless you'd rather stay anonymous.

## What counts

tmpmail treats every mail as hostile: anyone can send mail to a throwaway address. Examples
of what we want to hear about:

- mail content that runs script, reaches the desktop app's commands, opens files or URLs on
  its own, or loads remote content you didn't allow;
- mail content that changes your terminal, or fakes a sender in the list;
- a way to read or change another user's tmpmail files on the same machine;
- a problem in the release process or the published files.

The mail services themselves are run by others; problems on their side should go to them.
