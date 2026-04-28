//! # sery-mcp — local-files MCP server.
//!
//! `sery-mcp` is primarily a **binary** — most users run
//! `cargo install sery-mcp` and configure their MCP client to spawn
//! it. The library surface exists for the rare downstream that wants
//! to embed the same tool implementations into their own MCP server
//! (e.g. Sery Link's desktop app spawning the logic in-process
//! instead of as a subprocess).
//!
//! ## v0.1.0 surface
//!
//! - [`SeryMcpServer`] — the configured MCP server. Construct via
//!   [`SeryMcpServer::new`] with a single `--root` path; serve with
//!   [`rmcp::ServiceExt::serve`] and your transport of choice.
//! - One tool: `list_folder`. More land in subsequent minor versions
//!   (see the README's planned tool surface).
//!
//! ## Privacy + threat model
//!
//! `sery-mcp` opens no sockets and makes no outbound network calls.
//! All file reads are bounded by `--root`: any tool argument that
//! tries to escape via `..` or absolute paths is rejected before the
//! filesystem call. Tools are read-only by design — no `write_file`,
//! no `delete`, no `execute`.

#![doc(html_root_url = "https://docs.rs/sery-mcp/0.1.0")]
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::path::{Component, Path, PathBuf};

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};

/// The crate version as reported by Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Tool input schemas
// ---------------------------------------------------------------------------

/// Input for the `list_folder` tool.
///
/// `path` is interpreted **relative to the configured `--root`**. Any
/// absolute path or path containing `..` is rejected. `limit` caps the
/// number of returned entries; the default keeps tool responses small
/// enough to fit in an LLM context window.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListFolderRequest {
    /// Optional sub-path under the configured root. Defaults to the
    /// root itself when omitted, empty, or `"."`.
    #[serde(default)]
    #[schemars(
        description = "Subdirectory under the configured --root. Must be relative — no '..' segments, no absolute paths. Defaults to the root."
    )]
    pub path: Option<String>,

    /// Optional cap on the number of entries to return. Defaults to
    /// 1000.
    #[serde(default)]
    #[schemars(description = "Maximum entries to return. Defaults to 1000.")]
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Tool output shapes
// ---------------------------------------------------------------------------

/// One entry in a `list_folder` response. Lowercase, dot-less
/// `extension` is pre-computed by `scankit`.
#[derive(Debug, serde::Serialize)]
pub struct FileEntry {
    /// Path relative to the configured `--root`.
    pub relative_path: String,
    /// File size in bytes at the time of the walk.
    pub size_bytes: u64,
    /// Last-modified timestamp as RFC 3339, when the filesystem
    /// reports one. `None` on filesystems that don't track it.
    pub modified: Option<String>,
    /// Lowercase extension without the leading dot. Empty string for
    /// extensionless files.
    pub extension: String,
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
    ///
    /// `root` should already be canonicalised by the caller (the
    /// binary does this in `main`). All tool calls are bounded by
    /// this path: arguments that escape it via `..` or absolute paths
    /// are rejected.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tool_router: Self::tool_router(),
        }
    }

    /// Returns the canonical root this server is exposing. Useful
    /// for tests + downstream integration.
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
        let json = serde_json::to_string_pretty(&entries)
            .map_err(|e| McpError::internal_error(format!("serialize entries: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // ── Internals ─────────────────────────────────────────────────

    /// Resolve a tool-supplied sub-path against `self.root`. Rejects
    /// absolute paths and `..` traversal up-front (no canonicalize
    /// roundtrip — that would TOCTOU on symlinks).
    fn resolve_subpath(&self, sub: Option<&str>) -> Result<PathBuf, McpError> {
        let raw = match sub {
            None => return Ok(self.root.clone()),
            Some(s) if s.is_empty() || s == "." => return Ok(self.root.clone()),
            Some(s) => s,
        };
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
        Ok(self.root.join(p))
    }

    /// Walk `target` via [`scankit::Scanner`], capping output at
    /// `limit` entries. Errors from individual `scankit::walk` items
    /// (permission denied, transient I/O) are silently dropped — the
    /// LLM gets a partial-but-coherent listing rather than a hard
    /// fail on one bad file.
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
        // captures **rmcp's** crate name + version at rmcp's compile
        // time — so the LLM client would see `serverInfo.name = "rmcp"`,
        // not `"sery-mcp"`. CARGO_PKG_* macros expand against the
        // crate currently being compiled, which gives us the right
        // identity here.
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
                 (no .. escape, no absolute paths). v0.1.0 ships one tool: list_folder. \
                 Subsequent versions add get_schema, search_files, read_document, query_sql, \
                 and sample_rows — see https://github.com/seryai/sery-mcp."
                    .to_string(),
            )
    }
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
        SeryMcpServer::new(
            root.canonicalize()
                .expect("temp dir must canonicalise cleanly"),
        )
    }

    #[test]
    fn resolve_subpath_defaults_to_root() {
        let dir = TempDir::new().unwrap();
        let server = make_server(dir.path());
        for input in [None, Some(""), Some("."), Some(".")] {
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
    fn resolve_subpath_joins_relative() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let server = make_server(dir.path());
        let resolved = server.resolve_subpath(Some("sub")).unwrap();
        assert_eq!(resolved, server.root.join("sub"));
    }

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
}
