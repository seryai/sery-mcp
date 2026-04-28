# Changelog

All notable changes to `sery-mcp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and `sery-mcp`
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`sery-mcp` is pre-1.0 — the public CLI surface, MCP tool surface, and
configuration shape may evolve until 1.0 lands. We will document
breaking changes here and bump the minor version (0.x → 0.y) when we
do.

## [Unreleased]

## [0.4.1] — 2026-04-28

### Changed — docs polish

Docs / CHANGELOG / lib.rs prose now reference the SQL backend by
function (embedded SQL engine, smart CSV sniffing, multi-file
JOINs, glob reads) rather than by product name. The dependency
graph on crates.io is unchanged; this is an editing pass only.

No code or API changes — straight 0.4.0 → 0.4.1 patch bump.

## [0.4.0] — 2026-04-28

### Changed — `query_sql` switched to a richer SQL backend

The previous backend covered the simple "one file, one query" case
but couldn't handle the use cases that real desktop apps need:
multi-file JOINs (e.g. customer × order tables), glob patterns over
folders (`2024/*.csv`), smart CSV sniffing on messy real-world
exports, window functions for trends, native XLSX.

Switched to a feature-richer embedded SQL engine that ships with the
binary. Same dialect across local stdio MCP and any cloud transport
that proxies through the same code path.

### Added — multi-file `query_sql`

- **`tables: { name: path }`** request shape lets the LLM register
  multiple files as named SQL tables and JOIN across them. Each name
  must be a valid SQL identifier; cap of 16 tables per call.
- **Glob patterns** (`*`, `**`, `?`, `[...]`) supported in both
  single-file `path` mode and multi-file `tables` mode — the engine
  expands them at read time. Patterns stay bounded by `--root` —
  the path validator still rejects `..` and absolute paths.
- **HUGEINT / Decimal128 handling** for `SUM` / `AVG` results.
  Scale-0 values that fit in `i64` emit as JSON numbers; larger
  values fall through to string preserving precision.

### Added — read-only safety net

Even with `--root` sandboxing, paranoid SQL validation:

- Query must start with `SELECT` or `WITH` — anything else rejected.
- Token-based blacklist for: `INSERT`, `UPDATE`, `DELETE`, `CREATE`,
  `DROP`, `ALTER`, `ATTACH`, `DETACH`, `COPY`, `PRAGMA`, `INSTALL`,
  `LOAD`, `EXPORT`, `IMPORT`, `CHECKPOINT`, `VACUUM`, `ANALYZE`,
  `TRUNCATE`, `GRANT`, `REVOKE`, `BEGIN`, `COMMIT`, `ROLLBACK`,
  `SAVEPOINT`. False positives only when those keywords appear as
  literal string values (`WHERE name = 'INSERT'`) — LLM can reword.
- Path strings are escaped before interpolation into
  `read_csv_auto('...')` / `read_parquet('...')` calls (single-quotes
  doubled per the engine's literal escape rules).

### Tests

- 5 new tests, 23 total. `query_sql_csv_happy_path`,
  `query_sql_truncates_at_limit`, `query_sql_rejects_unsupported_format`,
  `query_sql_surfaces_sql_parse_errors`, `query_sql_blocks_ddl`,
  `query_sql_multi_file_join`, `query_sql_glob_pattern`,
  `query_sql_rejects_both_path_and_tables`,
  `query_sql_rejects_invalid_table_name`.

### Migration notes

Single-file `query_sql` callers from v0.3 keep working — the `path`
argument is still accepted, the file is still registered as table
`data`. Only change: response field renamed `relative_path` → `input`
(now describes either a single path or a multi-table mapping).
Standalone v0.3 binary users won't notice; LLMs see a slightly
different field name in tool results.

`query_sql` is now **synchronous** internally (the new SQL backend's
API is sync; async wasn't buying us anything in the
stdio-one-client-at-a-time model). The tool method dropped its
`async` keyword. Callers that embed `SeryMcpServer::query_sql`
directly need to drop their `.await`.

## [0.3.0] — 2026-04-28

### Added — `query_sql` (the v0.3 headline tool)

- **`query_sql`** — read-only SQL queries against a CSV / TSV /
  Parquet file. The file is registered as table `data` for the
  duration of the call; the LLM writes
  `SELECT name FROM data WHERE amount > 100`, not a path-templated
  query. Backed by [DataFusion](https://crates.io/crates/datafusion)
  53 (pure Rust, Apache 2.0).
- **Default row cap of 100, max 1000.** Use SQL `LIMIT` for tighter
  caps. The response carries a `truncated: bool` field so the LLM
  can detect when it should refine the query.
- **Format dispatch**: CSV (default delimiter), TSV (tab delimiter),
  Parquet. XLSX/XLS aren't supported by `query_sql` — use
  `get_schema` + `sample_rows` for those, or convert to Parquet
  first via `tabkit`.
- **Read-only by design**: DataFusion's `SessionContext` exposes no
  `CREATE EXTERNAL TABLE` against arbitrary files when no DDL
  statements are submitted; we never call `execute_logical_plan` on
  user input. `INSERT` / `UPDATE` / `DELETE` / `DDL` are rejected
  at SQL parse time.
- **Arrow → JSON** conversion handles all common types: int / uint
  (8/16/32/64), float (32/64), bool, utf8, large utf8, date32 /
  date64 (ISO 8601), timestamp (sec / ms / μs / ns, ISO 8601
  RFC 3339). Decimals / lists / structs / dictionaries fall through
  to Arrow's `ArrayFormatter` so we don't panic on schemas we didn't
  explicitly map.

### Added — supporting infrastructure

- **`datafusion = "53"`** added as a direct dep. Default features
  cover SQL + Parquet + datetime / string / nested expressions +
  compression. ~5 MB compiled.
- **`QuerySqlRequest` + `QueryResponse`** types exported on the
  library surface for downstream embedders that want to share
  shapes with the LLM client.
- **4 new unit tests** (18 total): `query_sql_csv_happy_path`,
  `query_sql_truncates_at_limit` (verifies the +1 lookahead row
  detection), `query_sql_rejects_unsupported_format`,
  `query_sql_surfaces_sql_parse_errors`.

### Changed

- Server `instructions` enumerate all six tools and call out
  `query_sql`'s `data` table convention.
- Module-level docs reorganised around the v0.3 surface
  (six tools, no more "TBD" entries).

### Verified

- `cargo build --release` clean.
- `cargo clippy --all-targets -- -D warnings` clean.
- `cargo fmt --check` clean.
- 18 unit tests pass (4 new for `query_sql`).
- Live smoke test: `tools/list` returns all six tools with full
  JSON schemas.

### v0.3 is feature-complete

All six tools from the original roadmap now ship:
`list_folder`, `search_files`, `get_schema`, `sample_rows`,
`read_document`, `query_sql`. Future minor versions will focus on
hardening (timestamp precision edge cases in DataFusion, better
SQL error messages, optional PII redaction in `sample_rows`) rather
than new tools.

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
