//! `sery-mcp` binary entrypoint.
//!
//! Pre-0.1 bootstrap: prints the build banner to stderr and exits
//! cleanly. The actual MCP handshake (rmcp `ServiceExt::serve` over
//! stdio) + tool registrations land in v0.1.0.
//!
//! Why ship a placeholder rather than a fully working v0.1.0 in the
//! first commit: the MCP tool surface (input schemas, output shapes,
//! error semantics) deserves design review before being baked into
//! crates.io. The skeleton lets us verify the dep graph, license
//! files, and CI compile cleanly first.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "sery-mcp",
    version,
    about = "Local-files MCP server. Pure Rust. Read-only by design.",
    long_about = None,
)]
struct Cli {
    /// Folder to expose as the MCP root. Defaults to the current
    /// working directory. Tools that take a `path` argument are
    /// validated to fall under this root — no escaping via `..`.
    #[arg(long, value_name = "DIR")]
    root: Option<std::path::PathBuf>,
}

// Bootstrap binary doesn't yet have any fallible MCP work, but v0.1.0
// will (rmcp's serve loop, file I/O, schema lookups). Keep the Result
// return type now so the signature doesn't churn when real work lands.
#[allow(clippy::unnecessary_wraps)]
fn main() -> anyhow::Result<()> {
    // Subscribe to tracing → stderr. CRITICAL: never log to stdout —
    // stdout is reserved for the MCP transport (JSON-RPC frames).
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let root = cli
        .root
        .unwrap_or_else(|| std::env::current_dir().expect("CWD must be readable"));

    tracing::info!(
        version = sery_mcp::VERSION,
        root = %root.display(),
        "sery-mcp bootstrap (pre-0.1) — MCP handshake + tools land in v0.1.0"
    );

    eprintln!(
        "sery-mcp v{} — pre-0.1 bootstrap. See https://github.com/seryai/sery-mcp",
        sery_mcp::VERSION
    );

    Ok(())
}
