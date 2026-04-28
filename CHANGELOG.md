# Changelog

All notable changes to `sery-mcp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and `sery-mcp`
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`sery-mcp` is pre-1.0 — the public CLI surface, MCP tool surface, and
configuration shape may evolve until 1.0 lands. We will document
breaking changes here and bump the minor version (0.x → 0.y) when we
do.

## [Unreleased]

## [0.2.0] — 2026-04-28

### Added — four new tools (read-only)

- **`search_files`** — case-insensitive filename search with ranking.
  Walks `--root` via scankit, scores each file by basename match
  (exact: 1.0, startswith: 0.8, contains: 0.5, path-contains: 0.2),
  returns top N sorted by score then path. Optional `extensions`
  filter restricts to specific file types (e.g. `["csv", "parquet"]`).
  Empty query rejected; default limit 50.
- **`get_schema`** — column names + inferred types + row count for any
  tabular file (CSV / TSV / Parquet / XLSX / XLS / XLSB / XLSM / ODS).
  Backed by tabkit. Skips sample-row work for speed; use
  `sample_rows` separately when you need values. Optional `sheet`
  argument for multi-sheet workbooks.
- **`sample_rows`** — first N rows of a tabular file as header-keyed
  JSON objects. Defaults to 5 rows, capped at 100. Same backend as
  `get_schema`. Tool description warns LLM clients that values can
  contain PII (no redaction in v0.2 — that's a future feature).
- **`read_document`** — DOCX / PDF / PPTX / HTML / IPYNB / EPUB / RTF
  / ODT → markdown. Backed by mdkit (libpdfium for PDF, pandoc for
  office, html2md for HTML, serde_json for IPYNB). Returns the full
  extracted text with title + char_count metadata. **50 MB file size
  cap** — beyond that, mdkit's pandoc / libpdfium backends start to
  trip on memory and the LLM context window can't hold the result.

### Added — supporting infrastructure

- **`tabkit::Engine` + `mdkit::Engine` lazy singletons.** Initialised
  once per process via `OnceLock`, shared across all tool calls.
- **`resolve_required_file()` helper** — extends the v0.1
  path-traversal hardening to tools that take a *required* file path
  (not optional sub-folder). Validates `..` / absolute / drive
  prefix rejection AND that the resolved path points to a regular
  file (not a directory or missing).
- **Stable JSON schemas for all tool I/O** — `SearchHit`,
  `ColumnInfo`, `SchemaResponse`, `SamplesResponse`,
  `DocumentResponse`. All `pub` and exported from the library
  surface so downstream embedders (Sery Link) can deserialize the
  same shapes the LLM client receives.
- **Centralised `as_json_result()` helper** — single place to
  pretty-print + wrap tool output in `CallToolResult::success`.
  Tools now consistent in formatting.
- **7 new unit tests** (14 total): `get_schema_returns_csv_columns`,
  `sample_rows_returns_header_keyed_objects`,
  `search_files_ranks_basename_match_above_path_match`,
  `search_files_extension_filter`,
  `search_files_rejects_empty_query`,
  `resolve_required_file_rejects_directory`,
  `resolve_required_file_rejects_missing`,
  `resolve_required_file_accepts_real_file`.

### Changed

- Server `instructions` updated to enumerate all five v0.2 tools and
  flag `query_sql` as v0.3 work.
- Module-level docs reorganised around the v0.2 surface.

### Coming in v0.3.0

- **`query_sql`** — read-only SQL queries against a single tabular
  file via DataFusion. Bigger design problem than the other tools
  (CSV / Parquet reader config, result schema, row-count caps for
  LLM context windows) so it's getting its own minor version.

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
