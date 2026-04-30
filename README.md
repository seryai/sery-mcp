# sery-mcp

**The local-files MCP server. Pure Rust. Free your Claude Desktop / Cursor / Zed / Continue from the upload-to-cloud dance.**

> **Status:** v0.4 — `query_sql` with multi-file JOINs, glob patterns, and smart CSV sniffing. Library-first packaging so other Rust crates can embed the same tool surface.

`sery-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server that exposes the data-heavy files on your machine — CSVs,
Parquet, Excel, DOCX, PDF, HTML — to any MCP-aware LLM client. Files
stay where they live; only what your AI explicitly asks about ever
crosses the stdio pipe.

## Why this exists

Every MCP server in the official registry today is TypeScript or
Python. Both are fine for a single-user demo, neither is what you
want sitting in your `mcp.json` quietly indexing 50 GB of CSVs and
documents while you work.

`sery-mcp` is the canonical pure-Rust local-files MCP server,
extracted from [Sery Link](https://sery.ai)'s production scanner. It
composes three open-source crates we already publish:

- [`scankit`](https://crates.io/crates/scankit) — walk + watch + filter directory trees
- [`tabkit`](https://crates.io/crates/tabkit) — tabular files → schema + sample rows + row count
- [`mdkit`](https://crates.io/crates/mdkit) — documents → markdown (PDF, DOCX, PPTX, HTML, IPYNB)

Plus ~500 lines of MCP glue on top of [`rmcp`](https://crates.io/crates/rmcp), the official Anthropic
Rust SDK.

## Quick start

```bash
cargo install sery-mcp
```

Then add it to your MCP client's config. Pick yours below.

### Claude Desktop

`~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)
or `%APPDATA%\Claude\claude_desktop_config.json` (Windows):

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

### Warp ([went open-source 2026-04-28](https://www.warp.dev/blog/warp-is-now-open-source))

In Warp: **Settings → AI → MCP Servers → + Add** and paste:

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

(Warp accepts the same `mcpServers` JSON format as Claude Desktop and
Cursor — same snippet works in all three. See the
[Warp MCP docs](https://docs.warp.dev/agent-platform/capabilities/mcp)
for the UI flow.)

Then in any Warp prompt: *"find tax documents from 2024"* or
*"sum the revenue column across every CSV in this folder"* — Oz
agents call `sery-mcp`'s tools to read your local files; nothing
uploads.

### Cursor

`~/.cursor/mcp.json`:

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

### Continue (VS Code / JetBrains)

In `~/.continue/config.json` under `mcpServers`:

```jsonc
{
  "mcpServers": [
    {
      "name": "sery",
      "transport": {
        "type": "stdio",
        "command": "sery-mcp",
        "args": ["--root", "/Users/me/Documents"]
      }
    }
  ]
}
```

Restart your client. The tools below show up in its tool palette.

## Tool surface (v0.3.0 — feature-complete)

| Tool | What it does | Backed by |
|---|---|---|
| `list_folder` | Enumerate files under `--root` (or a sub-path) with size + mtime + extension | `scankit` |
| `search_files` | Case-insensitive filename search with ranking (exact 1.0 → path-substring 0.2). Optional extension filter. | `scankit` + scoring |
| `get_schema` | Column names + inferred types + row count for any tabular file (CSV / TSV / Parquet / XLSX / XLS / XLSB / XLSM / ODS) | `tabkit` |
| `sample_rows` | First N rows of a tabular file as header-keyed JSON objects (default 5, capped at 100) | `tabkit` |
| `read_document` | DOCX / PDF / PPTX / HTML / IPYNB / EPUB / RTF / ODT → markdown. 50 MB cap. | `mdkit` (libpdfium / pandoc / html2md) |
| `query_sql` | Read-only SQL on **one or more** CSV / TSV / Parquet files. Single-file: pass `path`, reference as table `data`. Multi-file: pass `tables: { name → path }`, JOIN them. Glob patterns supported. Window functions, CTEs, smart CSV sniffing. Row cap default 100, max 1000. | embedded SQL engine |

Tools are **read-only** by design. There is no `write_file`, no
`execute_command`, no `delete`. `query_sql` rejects INSERT / UPDATE
/ DELETE / DDL at SQL parse time. The privacy story is that a bug
in the LLM (or in your prompt) cannot lose your data.

## What `sery-mcp` deliberately doesn't do

- **No cloud.** Stdio transport only in v0.1. The cloud-routed variant
  (`mcp.sery.ai`) is a separate product that wraps the existing Sery
  Link tunnel; this binary is local-only.
- **No watching.** Tools are pull-based — the LLM asks, we answer. If
  you need continuous indexing, run [Sery Link](https://sery.ai) and
  let `sery-mcp` query its scan cache instead of re-walking each call.
- **No write tools.** Read-only. Forever.
- **No auth.** Single-user, single-machine, stdio-spawned. If your MCP
  client is malicious you have bigger problems.

## Architecture

```
┌──────────────────────────────────────────────────┐
│  LLM client (Claude Desktop / Cursor / Zed / …) │
└────────────────────┬─────────────────────────────┘
                     │ stdio (MCP / JSON-RPC 2.0)
                     ▼
┌──────────────────────────────────────────────────┐
│  sery-mcp (this binary)                          │
│                                                  │
│  ┌─────────────────────────────────────────┐    │
│  │  rmcp (protocol + dispatch)             │    │
│  └────────────────────┬────────────────────┘    │
│                       │ tool calls               │
│                       ▼                          │
│  ┌─────────────────────────────────────────┐    │
│  │  Tools (v0.3.0 — feature-complete)      │    │
│  │  ├─ list_folder    →  scankit           │    │
│  │  ├─ search_files   →  scankit           │    │
│  │  ├─ get_schema     →  tabkit            │    │
│  │  ├─ sample_rows    →  tabkit            │    │
│  │  ├─ read_document  →  mdkit             │    │
│  │  └─ query_sql      →  embedded SQL      │    │
│  └─────────────────────────────────────────┘    │
└──────────────────────────────────────────────────┘
                     │ filesystem reads
                     ▼
                Your local files
```

## Privacy

`sery-mcp` is pure stdio. It opens no sockets. It makes no outbound
network calls. The only things that cross the stdio pipe are:

- The MCP handshake (capabilities, server info)
- Tool calls coming in (e.g. `{ "tool": "get_schema", "args": { "path": "..." } }`)
- Tool results going out (e.g. `[{"name":"customer_id","type":"INTEGER"}, ...]`)

Whatever your LLM client does with the results — that's between you
and your LLM client. We don't see prompts or completions.

The source of truth is the code: read `src/` for what the tools do,
read `Cargo.toml` for what we depend on. No FFI, no Python, no
network — by design.

## Family

| Crate | Role |
|---|---|
| [`sery-mcp`](https://github.com/seryai/sery-mcp) | This — local-files MCP server |
| [`scankit`](https://github.com/seryai/scankit) | Walk + watch + filter directory trees |
| [`tabkit`](https://github.com/seryai/tabkit) | Tabular files → schema + samples + count |
| [`mdkit`](https://github.com/seryai/mdkit) | Documents → markdown |
| [`sery-link`](https://github.com/seryai/sery-link) | Full desktop app — GUI, file watcher, multi-machine network |

## License

Dual-licensed under [MIT](LICENSE-MIT) OR [Apache 2.0](LICENSE-APACHE)
at your option. SPDX: `MIT OR Apache-2.0`. Same convention as the
rest of the Sery kit family.

## Contributing

Bug reports, format-coverage PRs, MCP spec-compliance fixes, and
downstream-production stories all welcome. Drop a star if you find
this useful — it helps other developers find it.

Issues + PRs: <https://github.com/seryai/sery-mcp/issues>
