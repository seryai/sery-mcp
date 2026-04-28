# Changelog

All notable changes to `sery-mcp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and `sery-mcp`
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`sery-mcp` is pre-1.0 — the public CLI surface, MCP tool surface, and
configuration shape may evolve until 1.0 lands. We will document
breaking changes here and bump the minor version (0.x → 0.y) when we
do.

## [Unreleased]

### Added
- Initial repo skeleton: dual MIT/Apache 2.0 license, Cargo manifest
  pinning [`rmcp`](https://crates.io/crates/rmcp) (the official
  Anthropic Rust SDK for Model Context Protocol) plus the Sery kit
  family ([`scankit`](https://crates.io/crates/scankit),
  [`tabkit`](https://crates.io/crates/tabkit),
  [`mdkit`](https://crates.io/crates/mdkit)) as the file-extraction
  backends.
- Placeholder binary entrypoint that prints the build banner and exits
  cleanly. The MCP handshake + tool implementations land in v0.1.0.
- README explaining what `sery-mcp` is, where it fits in the Sery
  family (`sery-link` desktop app, kit-family infrastructure, this
  MCP bridge), and the planned tool surface.
- CI workflow mirroring the kit-family template (clippy + rustfmt +
  cargo-audit on Ubuntu / macOS / Windows, stable Rust + MSRV).

### Notes
- The first published release will be `0.1.0` once the MCP handshake
  + at least one working tool (`list_folder`) ship end-to-end. The
  bootstrap commits aren't published to crates.io.
