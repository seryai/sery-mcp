//! # sery-mcp — local-files MCP server (library surface)
//!
//! `sery-mcp` is primarily a **binary** — most users `cargo install
//! sery-mcp` and configure their MCP client to spawn it. The library
//! surface exists for the rare downstream that wants to embed
//! sery-mcp's tool implementations into their own MCP server (e.g.
//! Sery Link's desktop app spawning the same logic in-process
//! instead of as a subprocess).
//!
//! ## Status
//!
//! Pre-0.1 bootstrap. The MCP handshake + tool implementations land
//! in v0.1.0. The current library surface is intentionally empty so
//! consumers don't pin against an API that hasn't stabilised yet.
//!
//! ## Architecture
//!
//! See the README's architecture diagram. The short version:
//!
//! - [`rmcp`](https://crates.io/crates/rmcp) handles MCP protocol +
//!   stdio transport.
//! - The Sery kit family ([`scankit`], [`tabkit`], [`mdkit`]) does
//!   the file-extraction work.
//! - This crate is glue: register tools with rmcp, dispatch tool
//!   calls to the appropriate kit, return results.
//!
//! ## Privacy
//!
//! `sery-mcp` opens no sockets and makes no outbound network calls.
//! The only things crossing the stdio pipe are the MCP handshake,
//! tool calls, and tool results.

#![doc(html_root_url = "https://docs.rs/sery-mcp")]
#![cfg_attr(docsrs, feature(doc_cfg))]

// Public surface lands in v0.1.0 alongside the first working tool.
// For now, we expose a build-time version constant so downstream
// callers can sanity-check what they linked against.

/// The crate version as reported by Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
