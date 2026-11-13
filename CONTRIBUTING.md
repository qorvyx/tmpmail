# Contributing to tmpmail

Thanks for helping. Bug reports, ideas and pull requests are all welcome.

## Reporting a bug

Open an [issue](https://github.com/qorvyx/tmpmail/issues) with:

- what you ran (the command, or what you did in the app) and what happened;
- what you expected instead;
- `tmpmail --version` (or the desktop app's version), and your system;
- the provider, if it's about mail (`tmpmail inboxes` shows it).

Don't paste real addresses, codes or mail you want to keep private. For a security problem,
see [SECURITY.md](SECURITY.md) instead.

## Changing the code

The command-line tool and the shared library are in `src/`; the desktop app (Tauri 2 and
Svelte 5) is in `gui/`. Before you open a pull request, run:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

and, for changes to the desktop app, in `gui/`:

```sh
pnpm install
pnpm check && pnpm lint && pnpm build
```

Please:

- keep each pull request to one change, with a test that fails without it where one can
  reach it;
- update `README.md`, `docs/` or `gui/README.md` in the same pull request when behavior
  changes;
- write commit subjects as one short line saying what changed.

`./scripts/smoke.sh` checks the real providers (it creates throwaway inboxes); CI runs a
provider check daily.

## License

By contributing, you agree that your work is licensed under the project's license,
[GPL-3.0-or-later](LICENSE).
