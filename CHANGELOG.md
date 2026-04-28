# Changelog

All notable changes to `sery-mcp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and `sery-mcp`
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`sery-mcp` is pre-1.0 — the public CLI surface, MCP tool surface, and
configuration shape may evolve until 1.0 lands. We will document
breaking changes here and bump the minor version (0.x → 0.y) when we
do.

## [Unreleased]

## [0.1.0] — 2026-04-28

### Added — first working MCP server
- **Real MCP handshake** over rmcp's stdio transport. Clients
  (Claude Desktop, Cursor, Zed, Continue, …) get the standard
  initialize → tools/list → tools/call flow.
- **`list_folder` tool** — first end-to-end tool. Walks the
  configured `--root` (or a sub-path) via `scankit::Scanner` and
  returns one JSON object per file with `relative_path`,
  `size_bytes`, `modified` (RFC 3339), and lowercase `extension`.
  Read-only by design.
- **Path-traversal hardening** — `list_folder`'s `path` argument is
  validated to fall under `--root` before any filesystem call.
  Absolute paths, `..` segments, drive prefixes, and root anchors
  are rejected up-front with `invalid_params` errors. We
  deliberately don't `canonicalize()` here — that's TOCTOU-prone on
  symlinks; component checks are simpler and safer.
- **Correct `serverInfo` identity** — manually constructs
  `Implementation` from `CARGO_PKG_*` env vars instead of calling
  rmcp's `Implementation::from_build_env()`, which bakes in rmcp's
  own crate name + version. Clients now see
  `serverInfo.name = "sery-mcp"`, with description and homepage URL
  populated from the manifest.
- **`SeryMcpServer::new(root)`** — public constructor for downstream
  embedding (Sery Link will eventually spawn the same logic
  in-process instead of as a subprocess).
- **`FileEntry`, `ListFolderRequest`** types exported for
  downstream consumers building their own dispatch on top of the
  same tool surface.
- **7 unit tests** covering subpath resolution (defaults, absolute
  rejection, `..` rejection, relative join) and `walk_entries`
  behaviour (entries emitted, limit respected, extension
  lowercased).

### Bootstrap (carried over from pre-0.1)
- Dual-licensed under MIT OR Apache-2.0.
- Cargo manifest pinning `rmcp` 1.5 (the official Anthropic Rust
  SDK), `scankit` 0.3, `tabkit` 0.4, `mdkit` 0.7.
- CI workflow on Ubuntu / macOS / Windows (stable Rust + MSRV 1.88
  + clippy + rustfmt + cargo-audit).

### Quick start

Once installed via `cargo install sery-mcp`, point your MCP client
at it. Claude Desktop's `mcp.json`:

```jsonc
{
  "mcpServers": {
    "sery": {
      "command": "sery-mcp",
      "args": ["--root", "/Users/me/Documents"]
    }
  }
}
```

Restart the client and ask: *"List the files in my Documents
folder."* The LLM will call `list_folder` and stream the result.

### Coming in v0.2.0
- `get_schema` (tabular files → column names + types) via `tabkit`
- `read_document` (DOCX / PDF / HTML / IPYNB → markdown) via `mdkit`
- `search_files` (filename + extension search with ranking)

## [0.0.1] — 2026-04-28

### Added — initial bootstrap
- Repo skeleton: dual MIT/Apache 2.0 license, Cargo manifest
  pinning [`rmcp`](https://crates.io/crates/rmcp) plus the Sery kit
  family ([`scankit`](https://crates.io/crates/scankit),
  [`tabkit`](https://crates.io/crates/tabkit),
  [`mdkit`](https://crates.io/crates/mdkit)).
- Placeholder binary that printed the build banner and exited
  cleanly. Useful only for verifying the dep graph + license files.
- README, CI workflow, this CHANGELOG.
- **Not recommended for use** — superseded by 0.1.0 hours later.
