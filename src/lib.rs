//! # sery-mcp — local-files MCP server.
//!
//! `sery-mcp` is primarily a **binary** — most users run
//! `cargo install sery-mcp` and configure their MCP client to spawn
//! it. The library surface exists for the rare downstream that wants
//! to embed the same tool implementations into their own MCP server
//! (e.g. Sery Link's desktop app spawning the logic in-process
//! instead of as a subprocess).
//!
//! ## v0.3.0 surface
//!
//! - [`SeryMcpServer`] — the configured MCP server. Construct via
//!   [`SeryMcpServer::new`] with a single `--root` path; serve with
//!   [`rmcp::ServiceExt::serve`] and your transport of choice.
//! - **Six tools**, all read-only:
//!   - `list_folder` — enumerate files (scankit)
//!   - `search_files` — filename + extension search with scoring (scankit)
//!   - `get_schema` — column names + types + row count (tabkit)
//!   - `sample_rows` — N rows of sampled data, header-keyed (tabkit)
//!   - `read_document` — DOCX/PDF/PPTX/HTML/IPYNB → markdown (mdkit)
//!   - `query_sql` — read-only SQL queries against a CSV / Parquet
//!     file (`DataFusion`). The file is registered as table `data`
//!     for the duration of the call.
//!
//! ## Privacy + threat model
//!
//! `sery-mcp` opens no sockets and makes no outbound network calls.
//! All file reads are bounded by `--root`: any tool argument that
//! tries to escape via `..` or absolute paths is rejected before the
//! filesystem call. Tools are read-only by design — no `write_file`,
//! no `delete`, no `execute`.

#![doc(html_root_url = "https://docs.rs/sery-mcp/0.3.0")]
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};

/// The crate version as reported by Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 50 MB cap on document extraction. Mirrors Sery Link's scanner
/// default — beyond this, mdkit's pandoc / libpdfium backends start
/// to trip on memory limits and the LLM context window can't hold
/// the result anyway. Configurable via tool argument in a future
/// version; for v0.2 it's a hard cap.
const MAX_DOCUMENT_BYTES: u64 = 50 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Lazy backends
// ---------------------------------------------------------------------------

fn mdkit_engine() -> &'static mdkit::Engine {
    static ENGINE: OnceLock<mdkit::Engine> = OnceLock::new();
    ENGINE.get_or_init(mdkit::Engine::with_defaults)
}

fn tabkit_engine() -> &'static tabkit::Engine {
    static ENGINE: OnceLock<tabkit::Engine> = OnceLock::new();
    ENGINE.get_or_init(tabkit::Engine::with_defaults)
}

// ---------------------------------------------------------------------------
// Tool input schemas
// ---------------------------------------------------------------------------

/// Input for the `list_folder` tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListFolderRequest {
    /// Subdirectory under `--root`. Defaults to the root.
    #[serde(default)]
    #[schemars(
        description = "Subdirectory under the configured --root. Must be relative — no '..' segments, no absolute paths. Defaults to the root."
    )]
    pub path: Option<String>,
    /// Cap on the number of returned entries. Defaults to 1000.
    #[serde(default)]
    #[schemars(description = "Maximum entries to return. Defaults to 1000.")]
    pub limit: Option<usize>,
}

/// Input for the `search_files` tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchFilesRequest {
    /// The query string (case-insensitive substring of basename).
    #[schemars(
        description = "Search term (case-insensitive). Matched against file basenames; whole-path matches score lower."
    )]
    pub query: String,
    /// Optional extension filter. Only files matching one of these
    /// extensions (lowercase, no leading dot) are considered.
    #[serde(default)]
    #[schemars(
        description = "Restrict to files whose extension matches one of these (lowercase, no leading dot, e.g. ['csv','parquet'])."
    )]
    pub extensions: Option<Vec<String>>,
    /// Cap on results. Defaults to 50.
    #[serde(default)]
    #[schemars(description = "Maximum results to return. Defaults to 50.")]
    pub limit: Option<usize>,
}

/// Input for the `get_schema` tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetSchemaRequest {
    /// Path to a tabular file under `--root`.
    #[schemars(
        description = "Relative path to a tabular file (CSV / TSV / Parquet / XLSX / XLS / XLSB / XLSM / ODS) under --root."
    )]
    pub path: String,
    /// Optional sheet name for multi-sheet workbooks.
    #[serde(default)]
    #[schemars(
        description = "For multi-sheet XLSX / ODS files: which sheet to inspect. Defaults to the first non-empty sheet."
    )]
    pub sheet: Option<String>,
}

/// Input for the `sample_rows` tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SampleRowsRequest {
    /// Path to a tabular file under `--root`.
    #[schemars(description = "Relative path to a tabular file under --root.")]
    pub path: String,
    /// How many rows to return. Defaults to 5; capped at 100.
    #[serde(default)]
    #[schemars(description = "Sample-row count. Defaults to 5; capped at 100.")]
    pub limit: Option<usize>,
    /// Optional sheet name for multi-sheet workbooks.
    #[serde(default)]
    #[schemars(description = "For multi-sheet XLSX / ODS files: which sheet to sample.")]
    pub sheet: Option<String>,
}

/// Input for the `read_document` tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadDocumentRequest {
    /// Path to a document under `--root`.
    #[schemars(
        description = "Relative path to a document file (DOCX / PDF / PPTX / HTML / IPYNB / EPUB / RTF / ODT) under --root. 50 MB cap."
    )]
    pub path: String,
}

/// Input for the `query_sql` tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct QuerySqlRequest {
    /// Path to a CSV or Parquet file under `--root`.
    #[schemars(
        description = "Relative path to a CSV / TSV / Parquet file under --root. The file is registered as table `data` for the duration of this query."
    )]
    pub path: String,
    /// The SQL query. Reference the registered file as `data`,
    /// e.g. `SELECT * FROM data WHERE amount > 100`.
    #[schemars(
        description = "SQL query (DataFusion dialect, ANSI-compatible). Reference the registered file as table `data`. Read-only — no INSERT/UPDATE/DELETE/DDL support."
    )]
    pub sql: String,
    /// Cap on returned rows. Defaults to 100; capped at 1000.
    #[serde(default)]
    #[schemars(
        description = "Maximum rows to return. Defaults to 100, capped at 1000. Use SQL LIMIT for tighter caps."
    )]
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Tool output shapes
// ---------------------------------------------------------------------------

/// One entry in a `list_folder` response.
#[derive(Debug, serde::Serialize)]
pub struct FileEntry {
    /// Path relative to the configured `--root`.
    pub relative_path: String,
    /// File size in bytes at walk time.
    pub size_bytes: u64,
    /// Last-modified timestamp as RFC 3339, when the filesystem reports one.
    pub modified: Option<String>,
    /// Lowercase, dot-less extension. Empty when the file has none.
    pub extension: String,
}

/// One result in a `search_files` response.
#[derive(Debug, serde::Serialize)]
pub struct SearchHit {
    /// Path relative to the configured `--root`.
    pub relative_path: String,
    /// File size in bytes at walk time.
    pub size_bytes: u64,
    /// Lowercase, dot-less extension.
    pub extension: String,
    /// Match score in `[0.0, 1.0]`. See `search_files` doc for the rubric.
    pub score: f64,
    /// Short human-readable explanation of the match category.
    pub why_matched: &'static str,
}

/// One column in a `get_schema` response.
#[derive(Debug, serde::Serialize)]
pub struct ColumnInfo {
    /// Column header. Falls back to `column_<idx>` when the source has none.
    pub name: String,
    /// Inferred type as a stable lowercase string (`"integer"`, `"text"`, …).
    #[serde(rename = "type")]
    pub data_type: &'static str,
    /// `true` when any sample row had a null/empty cell in this position.
    pub nullable: bool,
}

/// `get_schema` response.
#[derive(Debug, serde::Serialize)]
pub struct SchemaResponse {
    /// The path the caller passed in, echoed back for tool-call audit.
    pub relative_path: String,
    /// Lowercase extension (`"csv"`, `"parquet"`, …).
    pub format: String,
    /// Columns in source order.
    pub columns: Vec<ColumnInfo>,
    /// Total row count when known. `None` when the backend skipped a
    /// full scan.
    pub row_count: Option<u64>,
    /// Backend metadata — for XLSX this carries `"sheet"`, for CSV
    /// it can carry `"delimiter"`. Stable keys are documented in
    /// tabkit's per-backend docs.
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub metadata: std::collections::HashMap<String, String>,
}

/// `sample_rows` response.
#[derive(Debug, serde::Serialize)]
pub struct SamplesResponse {
    /// The path the caller passed in.
    pub relative_path: String,
    /// Lowercase extension (`"csv"`, `"parquet"`, …).
    pub format: String,
    /// Column headers in source order.
    pub columns: Vec<String>,
    /// Sample rows as JSON objects keyed by column header.
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Total row count when known.
    pub row_count: Option<u64>,
}

/// `read_document` response.
#[derive(Debug, serde::Serialize)]
pub struct DocumentResponse {
    /// The path the caller passed in.
    pub relative_path: String,
    /// Lowercase extension (`"pdf"`, `"docx"`, …).
    pub format: String,
    /// Extracted markdown text — the whole document.
    pub markdown: String,
    /// Document title when the backend could derive one.
    pub title: Option<String>,
    /// Extracted markdown character count.
    pub char_count: usize,
    /// Source file size in bytes.
    pub size_bytes: u64,
}

/// `query_sql` response.
#[derive(Debug, serde::Serialize)]
pub struct QueryResponse {
    /// The path the caller passed in.
    pub relative_path: String,
    /// Lowercase extension of the queried file.
    pub format: String,
    /// Result column names in the order they appear in the projection.
    pub columns: Vec<String>,
    /// Result rows as JSON objects keyed by column name.
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Number of rows returned (after the row cap).
    pub row_count: usize,
    /// `true` when the result was capped by the row limit and the
    /// underlying query produced more rows. The LLM should use this
    /// to decide whether to refine the SQL with a tighter `WHERE`
    /// or `LIMIT`.
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// A configured MCP server. Cheap to construct + clone; share a single
/// instance across the rmcp serve loop.
#[derive(Clone)]
pub struct SeryMcpServer {
    root: PathBuf,
    // The router is consumed by the `#[tool_handler]` macro that
    // implements `ServerHandler` below — it dispatches incoming
    // tool/call requests to the right `#[tool]` method via this field.
    // Rust's dead-code analysis can't see that cross-macro usage so
    // we suppress the lint at the field rather than file-wide.
    #[allow(dead_code)]
    tool_router: ToolRouter<SeryMcpServer>,
}

#[tool_router]
impl SeryMcpServer {
    /// Construct a new server with the given filesystem root.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tool_router: Self::tool_router(),
        }
    }

    /// Returns the canonical root this server is exposing.
    pub fn root(&self) -> &Path {
        &self.root
    }

    // ── Tools ─────────────────────────────────────────────────────

    #[tool(
        description = "List files under the configured --root (or a sub-path). Returns one JSON object per file with relative_path, size_bytes, modified (ISO 8601), and extension. Read-only; never returns file contents. Path-traversal rejected."
    )]
    fn list_folder(
        &self,
        Parameters(req): Parameters<ListFolderRequest>,
    ) -> Result<CallToolResult, McpError> {
        let target = self.resolve_subpath(req.path.as_deref())?;
        let limit = req.limit.unwrap_or(1000);
        let entries = self.walk_entries(&target, limit)?;
        as_json_result(&entries)
    }

    #[tool(
        description = "Search files by name. Case-insensitive substring match against the basename, ranked: exact basename match (1.0), basename startswith (0.8), basename contains (0.5), path contains (0.2). Optional `extensions` filter restricts to specific file types. Returns up to `limit` hits sorted by score then path."
    )]
    fn search_files(
        &self,
        Parameters(req): Parameters<SearchFilesRequest>,
    ) -> Result<CallToolResult, McpError> {
        let limit = req.limit.unwrap_or(50);
        let query = req.query.trim().to_lowercase();
        if query.is_empty() {
            return Err(McpError::invalid_params("'query' must not be empty", None));
        }
        let ext_filter: Option<Vec<String>> = req
            .extensions
            .map(|v| v.into_iter().map(|s| s.to_ascii_lowercase()).collect());

        let scanner = scankit::Scanner::new(scankit::ScanConfig::default().follow_symlinks(false))
            .map_err(|e| McpError::internal_error(format!("scankit init: {e}"), None))?;

        let mut hits: Vec<SearchHit> = Vec::new();
        for result in scanner.walk(&self.root) {
            let Ok(entry) = result else { continue };
            if let Some(filter) = ext_filter.as_ref() {
                if !filter.iter().any(|e| e == &entry.extension) {
                    continue;
                }
            }
            let basename = entry
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_lowercase)
                .unwrap_or_default();
            let stem = entry
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_lowercase)
                .unwrap_or_default();
            let relative = entry
                .path
                .strip_prefix(&self.root)
                .unwrap_or(&entry.path)
                .to_string_lossy()
                .to_lowercase();

            let (score, why) = if stem == query || basename == query {
                (1.0, "exact basename match")
            } else if basename.starts_with(&query) {
                (0.8, "basename starts with query")
            } else if basename.contains(&query) {
                (0.5, "basename contains query")
            } else if relative.contains(&query) {
                (0.2, "path contains query")
            } else {
                continue;
            };

            hits.push(SearchHit {
                relative_path: entry
                    .path
                    .strip_prefix(&self.root)
                    .unwrap_or(&entry.path)
                    .to_string_lossy()
                    .into_owned(),
                size_bytes: entry.size_bytes,
                extension: entry.extension,
                score,
                why_matched: why,
            });
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        hits.truncate(limit);
        as_json_result(&hits)
    }

    #[tool(
        description = "Return column names + inferred types + row count for a tabular file (CSV / TSV / Parquet / XLSX / XLS / XLSB / XLSM / ODS). Backed by tabkit. row_count is null for very large files where a full scan was skipped. Specify `sheet` for multi-sheet workbooks."
    )]
    fn get_schema(
        &self,
        Parameters(req): Parameters<GetSchemaRequest>,
    ) -> Result<CallToolResult, McpError> {
        let path = self.resolve_required_file(&req.path)?;
        let mut options = tabkit::ReadOptions::default().max_sample_rows(0);
        if let Some(sheet) = req.sheet {
            options = options.sheet_name(sheet);
        }
        let table = tabkit_engine()
            .read(&path, &options)
            .map_err(|e| McpError::internal_error(format!("tabkit read: {e}"), None))?;
        let response = SchemaResponse {
            relative_path: req.path,
            format: extension_of(&path),
            columns: table
                .columns
                .iter()
                .map(|c| ColumnInfo {
                    name: c.name.clone(),
                    data_type: data_type_str(c.data_type),
                    nullable: c.nullable,
                })
                .collect(),
            row_count: table.row_count,
            metadata: table.metadata,
        };
        as_json_result(&response)
    }

    #[tool(
        description = "Return the first N rows of a tabular file as header-keyed JSON objects. Defaults to 5 rows; capped at 100. Specify `sheet` for multi-sheet workbooks. Use sparingly — sample rows can contain PII; this tool returns raw cell values without redaction."
    )]
    fn sample_rows(
        &self,
        Parameters(req): Parameters<SampleRowsRequest>,
    ) -> Result<CallToolResult, McpError> {
        let path = self.resolve_required_file(&req.path)?;
        let limit = req.limit.unwrap_or(5).min(100);
        let mut options = tabkit::ReadOptions::default().max_sample_rows(limit);
        if let Some(sheet) = req.sheet {
            options = options.sheet_name(sheet);
        }
        let table = tabkit_engine()
            .read(&path, &options)
            .map_err(|e| McpError::internal_error(format!("tabkit read: {e}"), None))?;
        let column_names: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
        let rows = table
            .sample_rows
            .iter()
            .map(|row| {
                let mut obj = serde_json::Map::new();
                for (i, col) in column_names.iter().enumerate() {
                    let v = row.get(i).map_or(serde_json::Value::Null, value_to_json);
                    obj.insert(col.clone(), v);
                }
                obj
            })
            .collect();
        let response = SamplesResponse {
            relative_path: req.path,
            format: extension_of(&path),
            columns: column_names,
            rows,
            row_count: table.row_count,
        };
        as_json_result(&response)
    }

    #[tool(
        description = "Run a read-only SQL query against a CSV or Parquet file. The file is registered as table `data` for the duration of the call — write `SELECT * FROM data WHERE amount > 100` (not the file path). Backed by DataFusion. Returns header-keyed JSON rows; row count capped at 1000 (default 100) — use SQL LIMIT or `limit` arg for tighter caps. Read-only by design — INSERT/UPDATE/DELETE/DDL are rejected."
    )]
    async fn query_sql(
        &self,
        Parameters(req): Parameters<QuerySqlRequest>,
    ) -> Result<CallToolResult, McpError> {
        let path = self.resolve_required_file(&req.path)?;
        let format = extension_of(&path);
        let limit = req.limit.unwrap_or(100).min(1000);

        let path_str = path
            .to_str()
            .ok_or_else(|| McpError::invalid_params("path is not valid UTF-8", None))?;

        let ctx = datafusion::prelude::SessionContext::new();
        match format.as_str() {
            "csv" | "tsv" => {
                let mut options = datafusion::prelude::CsvReadOptions::new();
                if format == "tsv" {
                    options = options.delimiter(b'\t');
                }
                ctx.register_csv("data", path_str, options)
                    .await
                    .map_err(|e| McpError::internal_error(format!("register csv: {e}"), None))?;
            }
            "parquet" => {
                ctx.register_parquet(
                    "data",
                    path_str,
                    datafusion::prelude::ParquetReadOptions::default(),
                )
                .await
                .map_err(|e| McpError::internal_error(format!("register parquet: {e}"), None))?;
            }
            other => {
                return Err(McpError::invalid_params(
                    format!(
                        "query_sql supports csv / tsv / parquet only — got '{other}'. \
                         Use get_schema or sample_rows for XLSX/ODS files."
                    ),
                    None,
                ));
            }
        }

        // Ask DataFusion for one extra row so we can detect truncation
        // without scanning the whole batch list afterwards.
        let dataframe = ctx
            .sql(&req.sql)
            .await
            .map_err(|e| McpError::invalid_params(format!("sql parse / plan: {e}"), None))?
            .limit(0, Some(limit + 1))
            .map_err(|e| McpError::internal_error(format!("apply limit: {e}"), None))?;

        let columns: Vec<String> = dataframe
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();

        let batches = dataframe
            .collect()
            .await
            .map_err(|e| McpError::invalid_params(format!("sql execute: {e}"), None))?;

        let mut rows = Vec::with_capacity(limit);
        let mut truncated = false;
        'outer: for batch in &batches {
            for row_idx in 0..batch.num_rows() {
                if rows.len() == limit {
                    truncated = true;
                    break 'outer;
                }
                let mut obj = serde_json::Map::with_capacity(columns.len());
                for (col_idx, col_name) in columns.iter().enumerate() {
                    let array = batch.column(col_idx);
                    obj.insert(
                        col_name.clone(),
                        arrow_value_to_json(array.as_ref(), row_idx),
                    );
                }
                rows.push(obj);
            }
        }

        let response = QueryResponse {
            relative_path: req.path,
            format,
            row_count: rows.len(),
            columns,
            rows,
            truncated,
        };
        as_json_result(&response)
    }

    #[tool(
        description = "Convert a document file (DOCX / PDF / PPTX / HTML / IPYNB / EPUB / RTF / ODT) to markdown. Backed by mdkit (libpdfium for PDF, pandoc for office formats, html2md for HTML). 50 MB file size cap; larger files return an error. Returns the full extracted text — pair with a chunk-aware caller if your LLM context window can't hold the whole document."
    )]
    fn read_document(
        &self,
        Parameters(req): Parameters<ReadDocumentRequest>,
    ) -> Result<CallToolResult, McpError> {
        let path = self.resolve_required_file(&req.path)?;
        let metadata = std::fs::metadata(&path)
            .map_err(|e| McpError::internal_error(format!("stat: {e}"), None))?;
        if metadata.len() > MAX_DOCUMENT_BYTES {
            return Err(McpError::invalid_params(
                format!(
                    "file is {} bytes; read_document caps at {} bytes (50 MB)",
                    metadata.len(),
                    MAX_DOCUMENT_BYTES
                ),
                None,
            ));
        }
        let document = mdkit_engine()
            .extract(&path)
            .map_err(|e| McpError::internal_error(format!("mdkit extract: {e}"), None))?;
        let format = extension_of(&path);
        let response = DocumentResponse {
            char_count: document.markdown.chars().count(),
            relative_path: req.path,
            format,
            title: document.title,
            markdown: document.markdown,
            size_bytes: metadata.len(),
        };
        as_json_result(&response)
    }

    // ── Internals ─────────────────────────────────────────────────

    /// Resolve a tool-supplied sub-path against `self.root` for the
    /// "may be omitted, defaults to root" case (used by `list_folder`).
    fn resolve_subpath(&self, sub: Option<&str>) -> Result<PathBuf, McpError> {
        let raw = match sub {
            None => return Ok(self.root.clone()),
            Some(s) if s.is_empty() || s == "." => return Ok(self.root.clone()),
            Some(s) => s,
        };
        validate_relative_components(raw)?;
        Ok(self.root.join(raw))
    }

    /// Resolve a tool-supplied path that **must** point to a regular
    /// file under `self.root` (used by `get_schema`, `sample_rows`,
    /// `read_document`).
    fn resolve_required_file(&self, sub: &str) -> Result<PathBuf, McpError> {
        if sub.is_empty() {
            return Err(McpError::invalid_params("'path' must not be empty", None));
        }
        validate_relative_components(sub)?;
        let joined = self.root.join(sub);
        let metadata = std::fs::metadata(&joined)
            .map_err(|e| McpError::invalid_params(format!("path not readable: {e}"), None))?;
        if !metadata.is_file() {
            return Err(McpError::invalid_params(
                "'path' must refer to a regular file (not a directory or symlink)",
                None,
            ));
        }
        Ok(joined)
    }

    /// Walk `target` via [`scankit::Scanner`], capping output at
    /// `limit` entries. Errors from individual `scankit::walk` items
    /// (permission denied, transient I/O) are silently dropped.
    fn walk_entries(&self, target: &Path, limit: usize) -> Result<Vec<FileEntry>, McpError> {
        let scanner = scankit::Scanner::new(scankit::ScanConfig::default().follow_symlinks(false))
            .map_err(|e| McpError::internal_error(format!("scankit init: {e}"), None))?;

        let mut out = Vec::new();
        for result in scanner.walk(target) {
            if out.len() >= limit {
                break;
            }
            let Ok(entry) = result else { continue };
            let relative = entry
                .path
                .strip_prefix(&self.root)
                .unwrap_or(&entry.path)
                .to_string_lossy()
                .into_owned();
            out.push(FileEntry {
                relative_path: relative,
                size_bytes: entry.size_bytes,
                modified: entry
                    .modified
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
                extension: entry.extension,
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ServerHandler — protocol metadata
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for SeryMcpServer {
    fn get_info(&self) -> ServerInfo {
        // We build `Implementation` by hand rather than calling
        // `Implementation::from_build_env()` because the latter
        // captures rmcp's crate name + version at rmcp's compile
        // time — clients would see `serverInfo.name = "rmcp"`. The
        // CARGO_PKG_* macros expand against the crate currently
        // being compiled (sery-mcp), giving the right identity.
        let mut server_info =
            Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        server_info.description = Some(env!("CARGO_PKG_DESCRIPTION").to_string());
        let homepage = env!("CARGO_PKG_HOMEPAGE");
        if !homepage.is_empty() {
            server_info.website_url = Some(homepage.to_string());
        }

        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(server_info)
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "sery-mcp exposes the local files under the configured --root as MCP tools. \
                 All tools are read-only. Path arguments are validated to fall under --root \
                 (no .. escape, no absolute paths). v0.3 ships six tools: list_folder, \
                 search_files, get_schema, sample_rows, read_document (DOCX/PDF/PPTX/HTML/IPYNB \
                 → markdown), and query_sql (DataFusion-backed SQL on CSV/Parquet — file is \
                 registered as table `data` for the duration of the call). \
                 See https://github.com/seryai/sery-mcp."
                    .to_string(),
            )
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Reject absolute paths, `..` segments, drive prefixes, and root
/// anchors. Cheaper + safer than `canonicalize()` (no symlink TOCTOU).
fn validate_relative_components(raw: &str) -> Result<(), McpError> {
    let p = Path::new(raw);
    if p.is_absolute() {
        return Err(McpError::invalid_params(
            "'path' must be relative to --root (no absolute paths)",
            None,
        ));
    }
    for component in p.components() {
        match component {
            Component::ParentDir => {
                return Err(McpError::invalid_params(
                    "'path' must not contain '..' (no escaping the configured --root)",
                    None,
                ));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(McpError::invalid_params(
                    "'path' must be relative (no drive prefixes or root anchors)",
                    None,
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Lowercase, dot-less file extension. Empty string when the file
/// has no extension.
fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// Map a `tabkit::DataType` to a stable, lowercase JSON string.
fn data_type_str(t: tabkit::DataType) -> &'static str {
    match t {
        tabkit::DataType::Bool => "boolean",
        tabkit::DataType::Integer => "integer",
        tabkit::DataType::Float => "float",
        tabkit::DataType::Date => "date",
        tabkit::DataType::DateTime => "datetime",
        tabkit::DataType::Text => "text",
        // Covers `Unknown` plus any future `#[non_exhaustive]`
        // additions tabkit ships in a minor version.
        _ => "unknown",
    }
}

/// Convert a tabkit cell value to a JSON value for sample-row output.
fn value_to_json(v: &tabkit::Value) -> serde_json::Value {
    match v {
        tabkit::Value::Bool(b) => serde_json::Value::Bool(*b),
        tabkit::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        tabkit::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        tabkit::Value::Date(s) | tabkit::Value::DateTime(s) | tabkit::Value::Text(s) => {
            serde_json::Value::String(s.clone())
        }
        // Covers `Null` plus any future `#[non_exhaustive]` additions
        // — all map cleanly to JSON null.
        _ => serde_json::Value::Null,
    }
}

/// Convert one cell of an Arrow array to a JSON value.
///
/// Numeric / boolean types map to native JSON. Date / timestamp
/// types serialise as ISO 8601 strings (`DataFusion`'s default
/// format), since that round-trips cleanly through MCP / JSON / the
/// LLM. Anything we don't recognise downgrades to its `Display`
/// representation as a JSON string — keeps `query_sql` resilient
/// to `DataFusion` adding new types in minor versions.
#[allow(clippy::too_many_lines)] // exhaustive type-match by design — splitting harms readability
fn arrow_value_to_json(
    array: &dyn datafusion::arrow::array::Array,
    row: usize,
) -> serde_json::Value {
    use datafusion::arrow::array::{
        BooleanArray, Date32Array, Date64Array, Float32Array, Float64Array, Int16Array, Int32Array,
        Int64Array, Int8Array, LargeStringArray, StringArray, TimestampMicrosecondArray,
        TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray, UInt16Array,
        UInt32Array, UInt64Array, UInt8Array,
    };
    use datafusion::arrow::datatypes::DataType;

    if array.is_null(row) {
        return serde_json::Value::Null;
    }

    macro_rules! number {
        ($arr:ty) => {{
            let typed = array
                .as_any()
                .downcast_ref::<$arr>()
                .expect("downcast matches the matched DataType");
            serde_json::json!(typed.value(row))
        }};
    }

    match array.data_type() {
        DataType::Boolean => {
            let typed = array
                .as_any()
                .downcast_ref::<BooleanArray>()
                .expect("BooleanArray");
            serde_json::Value::Bool(typed.value(row))
        }
        DataType::Int8 => number!(Int8Array),
        DataType::Int16 => number!(Int16Array),
        DataType::Int32 => number!(Int32Array),
        DataType::Int64 => number!(Int64Array),
        DataType::UInt8 => number!(UInt8Array),
        DataType::UInt16 => number!(UInt16Array),
        DataType::UInt32 => number!(UInt32Array),
        DataType::UInt64 => number!(UInt64Array),
        DataType::Float32 => {
            let typed = array
                .as_any()
                .downcast_ref::<Float32Array>()
                .expect("Float32Array");
            serde_json::Number::from_f64(f64::from(typed.value(row)))
                .map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        DataType::Float64 => {
            let typed = array
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("Float64Array");
            serde_json::Number::from_f64(typed.value(row))
                .map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        DataType::Utf8 => {
            let typed = array
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("StringArray");
            serde_json::Value::String(typed.value(row).to_string())
        }
        DataType::LargeUtf8 => {
            let typed = array
                .as_any()
                .downcast_ref::<LargeStringArray>()
                .expect("LargeStringArray");
            serde_json::Value::String(typed.value(row).to_string())
        }
        DataType::Date32 => {
            let typed = array
                .as_any()
                .downcast_ref::<Date32Array>()
                .expect("Date32Array");
            typed
                .value_as_date(row)
                .map_or(serde_json::Value::Null, |d| {
                    serde_json::Value::String(d.format("%Y-%m-%d").to_string())
                })
        }
        DataType::Date64 => {
            let typed = array
                .as_any()
                .downcast_ref::<Date64Array>()
                .expect("Date64Array");
            typed
                .value_as_date(row)
                .map_or(serde_json::Value::Null, |d| {
                    serde_json::Value::String(d.format("%Y-%m-%d").to_string())
                })
        }
        DataType::Timestamp(_, _) => {
            // Cover all four precision variants by trying the most
            // common first. DataFusion CSV/Parquet readers emit
            // microsecond timestamps by default.
            if let Some(typed) = array.as_any().downcast_ref::<TimestampMicrosecondArray>() {
                return typed
                    .value_as_datetime(row)
                    .map_or(serde_json::Value::Null, |d| {
                        serde_json::Value::String(d.and_utc().to_rfc3339())
                    });
            }
            if let Some(typed) = array.as_any().downcast_ref::<TimestampMillisecondArray>() {
                return typed
                    .value_as_datetime(row)
                    .map_or(serde_json::Value::Null, |d| {
                        serde_json::Value::String(d.and_utc().to_rfc3339())
                    });
            }
            if let Some(typed) = array.as_any().downcast_ref::<TimestampNanosecondArray>() {
                return typed
                    .value_as_datetime(row)
                    .map_or(serde_json::Value::Null, |d| {
                        serde_json::Value::String(d.and_utc().to_rfc3339())
                    });
            }
            if let Some(typed) = array.as_any().downcast_ref::<TimestampSecondArray>() {
                return typed
                    .value_as_datetime(row)
                    .map_or(serde_json::Value::Null, |d| {
                        serde_json::Value::String(d.and_utc().to_rfc3339())
                    });
            }
            serde_json::Value::String(format!("(unsupported timestamp at row {row})"))
        }
        // For decimals, lists, structs, dictionaries, etc. — fall back
        // to DataFusion's `pretty_format_value`-style display rather
        // than panicking. Keeps query_sql resilient to schemas we
        // didn't anticipate.
        _ => {
            use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
            ArrayFormatter::try_new(array, &FormatOptions::default()).map_or_else(
                |_| {
                    serde_json::Value::String(format!("(unrenderable {} value)", array.data_type()))
                },
                |fmt| serde_json::Value::String(fmt.value(row).to_string()),
            )
        }
    }
}

/// Serialize any `Serialize` value to pretty JSON wrapped in a
/// `CallToolResult::success`. Centralised so all tools format the
/// same way.
fn as_json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_server(root: &Path) -> SeryMcpServer {
        SeryMcpServer::new(root.canonicalize().expect("temp dir must canonicalise"))
    }

    // ── path-resolution ──

    #[test]
    fn resolve_subpath_defaults_to_root() {
        let dir = TempDir::new().unwrap();
        let server = make_server(dir.path());
        for input in [None, Some(""), Some(".")] {
            let resolved = server.resolve_subpath(input).unwrap();
            assert_eq!(resolved, server.root);
        }
    }

    #[test]
    fn resolve_subpath_rejects_absolute() {
        let dir = TempDir::new().unwrap();
        let server = make_server(dir.path());
        let err = server.resolve_subpath(Some("/etc/passwd")).unwrap_err();
        assert!(format!("{err:?}").contains("absolute"));
    }

    #[test]
    fn resolve_subpath_rejects_parent_dir() {
        let dir = TempDir::new().unwrap();
        let server = make_server(dir.path());
        let err = server.resolve_subpath(Some("../etc")).unwrap_err();
        assert!(format!("{err:?}").contains(".."));
    }

    #[test]
    fn resolve_required_file_rejects_directory() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let server = make_server(dir.path());
        let err = server.resolve_required_file("sub").unwrap_err();
        assert!(format!("{err:?}").contains("regular file"));
    }

    #[test]
    fn resolve_required_file_rejects_missing() {
        let dir = TempDir::new().unwrap();
        let server = make_server(dir.path());
        let err = server.resolve_required_file("nope.csv").unwrap_err();
        assert!(format!("{err:?}").contains("not readable"));
    }

    #[test]
    fn resolve_required_file_accepts_real_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.csv"), "x,y\n").unwrap();
        let server = make_server(dir.path());
        let resolved = server.resolve_required_file("a.csv").unwrap();
        assert_eq!(resolved, server.root.join("a.csv"));
    }

    // ── walk_entries ──

    #[test]
    fn walk_entries_emits_files_under_root() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.csv"), "x,y\n1,2\n").unwrap();
        fs::write(dir.path().join("b.txt"), "hello").unwrap();
        let server = make_server(dir.path());
        let entries = server.walk_entries(server.root(), 100).unwrap();
        assert_eq!(entries.len(), 2);
        let names: Vec<_> = entries.iter().map(|e| e.relative_path.clone()).collect();
        assert!(names.contains(&"a.csv".to_string()));
        assert!(names.contains(&"b.txt".to_string()));
    }

    #[test]
    fn walk_entries_respects_limit() {
        let dir = TempDir::new().unwrap();
        for i in 0..10 {
            fs::write(dir.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let server = make_server(dir.path());
        let entries = server.walk_entries(server.root(), 3).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn walk_entries_lowercases_extension() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("REPORT.PDF"), "%PDF-").unwrap();
        let server = make_server(dir.path());
        let entries = server.walk_entries(server.root(), 100).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].extension, "pdf");
    }

    // ── get_schema (tabkit) ──

    #[test]
    fn get_schema_returns_csv_columns() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("sales.csv"),
            "id,name,amount\n1,alice,99.5\n2,bob,150.0\n",
        )
        .unwrap();
        let server = make_server(dir.path());
        let result = server
            .get_schema(Parameters(GetSchemaRequest {
                path: "sales.csv".into(),
                sheet: None,
            }))
            .unwrap();
        let payload = result_text(&result);
        let parsed: SchemaResponseDe = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed.format, "csv");
        assert_eq!(parsed.columns.len(), 3);
        let names: Vec<_> = parsed.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["id", "name", "amount"]);
    }

    // ── sample_rows ──

    #[test]
    fn sample_rows_returns_header_keyed_objects() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("sales.csv"),
            "id,name,amount\n1,alice,99.5\n2,bob,150.0\n3,eve,200.0\n",
        )
        .unwrap();
        let server = make_server(dir.path());
        let result = server
            .sample_rows(Parameters(SampleRowsRequest {
                path: "sales.csv".into(),
                limit: Some(2),
                sheet: None,
            }))
            .unwrap();
        let payload = result_text(&result);
        let parsed: SamplesResponseDe = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed.columns, vec!["id", "name", "amount"]);
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].get("name").unwrap().as_str(), Some("alice"));
    }

    // ── search_files ──

    #[test]
    fn search_files_ranks_basename_match_above_path_match() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("data/finance")).unwrap();
        fs::write(dir.path().join("data/finance/sales.csv"), "x").unwrap();
        fs::write(dir.path().join("salesreport.csv"), "x").unwrap();
        fs::write(dir.path().join("revenue.csv"), "x").unwrap();
        let server = make_server(dir.path());
        let result = server
            .search_files(Parameters(SearchFilesRequest {
                query: "sales".into(),
                extensions: None,
                limit: None,
            }))
            .unwrap();
        let payload = result_text(&result);
        let hits: Vec<SearchHitDe> = serde_json::from_str(&payload).unwrap();
        assert_eq!(hits.len(), 2);
        // sales.csv (exact stem) should outrank salesreport.csv (startswith)
        assert_eq!(hits[0].relative_path, "data/finance/sales.csv");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn search_files_extension_filter() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("notes.csv"), "x").unwrap();
        fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let server = make_server(dir.path());
        let result = server
            .search_files(Parameters(SearchFilesRequest {
                query: "notes".into(),
                extensions: Some(vec!["csv".into()]),
                limit: None,
            }))
            .unwrap();
        let hits: Vec<SearchHitDe> = serde_json::from_str(&result_text(&result)).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].extension, "csv");
    }

    // ── query_sql (DataFusion) ──

    #[tokio::test]
    async fn query_sql_csv_happy_path() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("sales.csv"),
            "id,name,amount\n1,alice,100\n2,bob,250\n3,eve,50\n",
        )
        .unwrap();
        let server = make_server(dir.path());
        let result = server
            .query_sql(Parameters(QuerySqlRequest {
                path: "sales.csv".into(),
                sql: "SELECT name, amount FROM data WHERE amount > 75 ORDER BY amount".into(),
                limit: None,
            }))
            .await
            .unwrap();
        let payload = result_text(&result);
        let parsed: QueryResponseDe = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed.format, "csv");
        assert_eq!(parsed.columns, vec!["name", "amount"]);
        assert_eq!(parsed.row_count, 2);
        assert!(!parsed.truncated);
        assert_eq!(parsed.rows[0].get("name").unwrap().as_str(), Some("alice"));
        assert_eq!(parsed.rows[1].get("name").unwrap().as_str(), Some("bob"));
    }

    #[tokio::test]
    async fn query_sql_truncates_at_limit() {
        use std::fmt::Write as _;
        let dir = TempDir::new().unwrap();
        let mut csv = String::from("n\n");
        for i in 0..20 {
            writeln!(csv, "{i}").unwrap();
        }
        fs::write(dir.path().join("nums.csv"), csv).unwrap();
        let server = make_server(dir.path());
        let result = server
            .query_sql(Parameters(QuerySqlRequest {
                path: "nums.csv".into(),
                sql: "SELECT n FROM data".into(),
                limit: Some(5),
            }))
            .await
            .unwrap();
        let parsed: QueryResponseDe = serde_json::from_str(&result_text(&result)).unwrap();
        assert_eq!(parsed.row_count, 5);
        assert!(parsed.truncated);
    }

    #[tokio::test]
    async fn query_sql_rejects_unsupported_format() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("notes.txt"), "hi").unwrap();
        let server = make_server(dir.path());
        let err = server
            .query_sql(Parameters(QuerySqlRequest {
                path: "notes.txt".into(),
                sql: "SELECT 1".into(),
                limit: None,
            }))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("csv / tsv / parquet"));
    }

    #[tokio::test]
    async fn query_sql_surfaces_sql_parse_errors() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.csv"), "x\n1\n").unwrap();
        let server = make_server(dir.path());
        let err = server
            .query_sql(Parameters(QuerySqlRequest {
                path: "a.csv".into(),
                sql: "SELEKT * FROM data".into(),
                limit: None,
            }))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").to_lowercase().contains("sql"));
    }

    #[test]
    fn search_files_rejects_empty_query() {
        let dir = TempDir::new().unwrap();
        let server = make_server(dir.path());
        let err = server
            .search_files(Parameters(SearchFilesRequest {
                query: "   ".into(),
                extensions: None,
                limit: None,
            }))
            .unwrap_err();
        assert!(format!("{err:?}").contains("empty"));
    }

    // ── helpers used only by tests ──

    fn result_text(result: &CallToolResult) -> String {
        let first = result.content.first().expect("at least one content item");
        // CallToolResult.content[i] is a `Content`; downcast to text via
        // serde round-trip is overkill — the SDK exposes the raw text via
        // `as_text()`. Fall back to JSON-serialising if not text.
        if let Some(text) = first.as_text() {
            text.text.clone()
        } else {
            serde_json::to_string(&first).unwrap()
        }
    }

    /// Owned mirror of `SchemaResponse` (the source struct lives behind
    /// `pub use` and serialises field-by-field; we want a deserialiser
    /// for tests).
    #[derive(serde::Deserialize)]
    struct SchemaResponseDe {
        #[allow(dead_code)]
        relative_path: String,
        format: String,
        columns: Vec<ColumnInfoDe>,
        #[allow(dead_code)]
        row_count: Option<u64>,
    }

    #[derive(serde::Deserialize)]
    struct ColumnInfoDe {
        name: String,
        #[serde(rename = "type")]
        #[allow(dead_code)]
        data_type: String,
        #[allow(dead_code)]
        nullable: bool,
    }

    #[derive(serde::Deserialize)]
    struct SamplesResponseDe {
        #[allow(dead_code)]
        relative_path: String,
        #[allow(dead_code)]
        format: String,
        columns: Vec<String>,
        rows: Vec<serde_json::Map<String, serde_json::Value>>,
        #[allow(dead_code)]
        row_count: Option<u64>,
    }

    #[derive(serde::Deserialize)]
    struct SearchHitDe {
        relative_path: String,
        #[allow(dead_code)]
        size_bytes: u64,
        extension: String,
        score: f64,
        #[allow(dead_code)]
        why_matched: String,
    }

    #[derive(serde::Deserialize)]
    struct QueryResponseDe {
        #[allow(dead_code)]
        relative_path: String,
        format: String,
        columns: Vec<String>,
        rows: Vec<serde_json::Map<String, serde_json::Value>>,
        row_count: usize,
        truncated: bool,
    }
}
