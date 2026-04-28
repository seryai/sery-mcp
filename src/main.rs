//! `sery-mcp` binary entrypoint.
//!
//! Wires the [`SeryMcpServer`] to rmcp's stdio transport. Tracing
//! goes to **stderr** — never stdout, since stdout is the MCP
//! protocol channel (JSON-RPC frames).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{transport::stdio, ServiceExt};
use sery_mcp::SeryMcpServer;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "sery-mcp",
    version,
    about = "Local-files MCP server. Pure Rust. Read-only by design.",
    long_about = "sery-mcp exposes local files (CSV, Parquet, XLSX, DOCX, PDF, HTML, …) \
                  as MCP tools to any Model Context Protocol client (Claude Desktop, \
                  Cursor, Zed, Continue, …). Stdio transport only; the LLM client spawns \
                  this binary, asks structured tool questions, and receives structured \
                  answers. Files never leave the machine."
)]
struct Cli {
    /// Folder to expose as the MCP root. Defaults to the current
    /// working directory. Tool arguments that take a `path` are
    /// validated to fall under this root — no `..` escape, no
    /// absolute paths.
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // CRITICAL: never log to stdout — stdout is the MCP transport
    // channel (JSON-RPC frames). Anything we print there breaks the
    // protocol and disconnects the client. Stderr is safe.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .init();

    let cli = Cli::parse();
    let raw_root = cli
        .root
        .unwrap_or_else(|| std::env::current_dir().expect("CWD must be readable"));
    let root = raw_root
        .canonicalize()
        .with_context(|| format!("failed to resolve --root {}", raw_root.display()))?;

    tracing::info!(
        version = sery_mcp::VERSION,
        root = %root.display(),
        "starting sery-mcp"
    );

    let service = SeryMcpServer::new(root)
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!(error = ?e, "rmcp serve failed to start"))?;

    service.waiting().await?;
    Ok(())
}
