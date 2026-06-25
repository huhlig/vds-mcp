# Changelog

| Date       | Version | Change                                                                           |
|------------|---------|----------------------------------------------------------------------------------|
| 2026-06-24 | 2.0.0   | VDS 2: filesystem-authoritative mode (`serve-v2`). Full section mutation suite, crash-recoverable transactions, filesystem watcher, incremental full-text index with BM25/phrase/camelCase support, workspace switching, and metadata integrity validation. |
| 2026-06-01 | 1.1.0   | Added support for MCP setting of workspace. (Claude Compatibility)               |
| 2026-05-29 | 1.0.0   | Initial Release                                                                  |

## VDS 2.0 — Compatibility Notes

VDS 2 (`serve-v2`) is a new server mode alongside the existing VDS 1 (`serve`). Both binaries are provided in the same `vds-mcp` executable.

**VDS 2 differences from VDS 1:**
- Markdown files are the source of truth; no binary database file is required.
- `.vds/` metadata is plain JSON, safe to commit to Git or sync with Dropbox.
- The tool catalog is different: VDS 2 exposes `full_text_search`, `manage_document_file`, `get_document_location`, `set_workspace`, and all section mutation tools; it does not expose VDS 1-only tools like `search_sections`, `find_by_title`, `find_by_tag`, `lock_section`, or `recent_changes`.
- `get_workspace` returns `"database": "filesystem"` in VDS 2 mode.
- `get_database` returns `"filesystem"` in VDS 2 mode.

**Migration:** VDS 2 is a greenfield deployment. Existing VDS 1 databases can continue to use `serve` mode. No automatic migration tool is provided.