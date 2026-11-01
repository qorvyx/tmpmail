//! tmpmail's core: providers, the stored inboxes, polling and mail
//! sanitizing. The `tmpmail` binary (feature `cli`, on by default) adds the
//! command line, the TUI and the MCP server; the desktop app uses the core
//! without them.

pub mod app;
#[cfg(feature = "cli")]
pub mod cli;
pub mod config;
pub mod error;
pub mod extract;
mod http;
mod lines;
#[cfg(feature = "cli")]
mod mcp;
pub mod provider;
pub mod push;
pub mod render;
pub mod store;
#[cfg(feature = "cli")]
mod system;
#[cfg(feature = "cli")]
mod tui;
pub mod watcher;
