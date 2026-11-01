use clap::{CommandFactory, Parser};

use tmpmail::cli::{self, Cli, Command};
use tmpmail::error::Error;
use tmpmail::{app, render};

fn main() {
    // Exit quietly when piped into `head` and the like, instead of panicking
    // on a broken pipe.
    #[cfg(unix)]
    // SAFETY: called once at startup before any threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args = cli::rewrite_legacy(std::env::args().collect());
    let json = cli::wants_json(&args);
    // Clap would exit with 2 on bad usage, which is our "wait timed out" code.
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        // Help and --version are output, not errors.
        Err(e) if !e.use_stderr() => {
            let _ = e.print();
            std::process::exit(0);
        }
        Err(e) if json => {
            report(&Error::Usage(cli::usage_message(&e)), true);
            std::process::exit(64);
        }
        Err(e) => {
            let _ = e.print();
            std::process::exit(64);
        }
    };

    match cli.command {
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "tmpmail",
                &mut std::io::stdout(),
            );
            return;
        }
        Command::Man { output } => {
            if let Err(e) = man(output.as_deref()) {
                report(&Error::Other(e), cli.global.json);
                std::process::exit(1);
            }
            return;
        }
        _ => {}
    }

    let json = cli.global.json;
    let mut app = match app::App::new(cli.global) {
        Ok(app) => app,
        Err(e) => {
            report(&e, json);
            std::process::exit(e.exit_code());
        }
    };
    if let Err(e) = app.run(cli.command) {
        if !app.error_shown() {
            report(&e, json);
        }
        std::process::exit(e.exit_code());
    }
}

/// The man page on stdout, or every page (one per command) written to `dir`.
fn man(dir: Option<&std::path::Path>) -> anyhow::Result<()> {
    use anyhow::Context;
    match dir {
        Some(dir) => {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
            clap_mangen::generate_to(Cli::command(), dir)
                .with_context(|| format!("cannot write the man pages to {}", dir.display()))?;
            eprintln!("wrote man pages to {}", dir.display());
            Ok(())
        }
        None => Ok(clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout())?),
    }
}

fn report(e: &Error, json: bool) {
    let msg = match e {
        // anyhow keeps the cause chain; show it.
        Error::Other(inner) => format!("{inner:#}"),
        e => e.to_string(),
    };
    if json {
        let v = serde_json::json!({ "error": { "kind": e.kind(), "message": msg } });
        println!("{}", render::escape_json_invisible(&v.to_string()));
    } else {
        eprintln!("tmpmail: {}", render::clean_line(&msg));
    }
}
