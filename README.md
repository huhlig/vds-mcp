# Versioned Document Service

Versioned Document Service (VDS) is a Rust MCP server for managing long-form Markdown documents as versioned, addressable section trees. It is designed for agents and tools that need to read, edit, search, and snapshot large documents without rewriting whole files each time.

VDS 2 (`serve-v2`) is the current recommended mode. It treats the project filesystem as authoritative: Markdown lives in your project directory and can be read and edited by humans and agents side by side. VDS metadata (`.vds/`) stores stable IDs, version history, and snapshots as plain JSON files that are safe to commit to Git or sync with Dropbox.

## Features

**VDS 2.0 (filesystem-authoritative)** — Production ready ✅

- Markdown files live in your project directory and are the source of truth.
- Stable document and section IDs survive renames, moves, and external edits.
- Every section edit creates an immutable version record on disk.
- Document snapshots capture a full tree at a named point in time.
- **Full-text search** with BM25 ranking, prefix queries, phrase queries, and camelCase tokenization.
- **Semantic search** (optional `semantic-search` feature) — HNSW vector index with external embedding cache.
- Filesystem watcher reloads changed documents without restarting the server.
- Single-writer workspace lease prevents concurrent write conflicts.
- Crash-recoverable transactions protect every mutation.
- Safe runtime workspace switching via `set_workspace`.
- All metadata stored as Git-friendly JSON files.

**Legacy VDS 1 (`serve`)** — backed by a local `redb` database. Retained for compatibility; VDS 2 is recommended for all new workspaces.

## Installation

From a local checkout:

```powershell
cargo install --locked --path .
```

From a Git repository:

```powershell
cargo install --locked --git <repo-url> vds-mcp
```

Verify the binary is on `PATH`:

```powershell
vds-mcp --help
```

## Quick Start

Run the VDS 2 stdio MCP server for your project:

```powershell
vds-mcp --workspace /path/to/project serve-v2
```

Or start the HTTP server:

```powershell
vds-mcp --workspace /path/to/project server-v2 --bind 127.0.0.1:8001 --path /mcp
```

VDS initializes `.vds/` metadata on first run. Existing Markdown files in the workspace are discovered automatically but not tracked until `manage_document_file` is called or a document is created through VDS. All `.vds/` JSON files (except the binary database lock) are Git-safe.

## MCP Client Configuration

**Claude Desktop / stdio clients** — add to your MCP server configuration:

```json
{
  "mcpServers": {
    "vds": {
      "command": "vds-mcp",
      "args": ["--workspace", "/absolute/path/to/project", "serve-v2"]
    }
  }
}
```

**HTTP clients** — run the server separately and point the client at it:

```powershell
vds-mcp --workspace E:\Projects\myproject server-v2 --bind 127.0.0.1:8001 --path /mcp
```

```text
http://127.0.0.1:8001/mcp
```

**Codex TOML**:

```toml
[mcp_servers.vds]
command = "vds-mcp"
args = ["--workspace", "/absolute/path/to/project", "serve-v2"]
```

## Workspace Layout

```
project/
├── docs/
│   ├── overview.md          ← your Markdown files (VDS 2 is authoritative)
│   └── installation.md
├── .vds/
│   ├── workspace.json       ← workspace identity and format version
│   └── documents/
│       └── doc-<id>/
│           ├── document.json   ← stable document identity and metadata
│           ├── current.json    ← current content hash and root section
│           ├── sections/       ← per-section identity and matching hints
│           ├── versions/       ← immutable section version history (JSON)
│           └── snapshots/      ← document snapshot records (JSON)
└── .vdsignore               ← optional glob patterns to exclude files
```

`.vds/` files are plain JSON and safe to commit. The only binary file (`vds.lock`) is excluded from Git automatically.

## .vdsignore

Create `.vdsignore` in the workspace root to exclude Markdown files from discovery:

```
# Ignore generated output
generated/

# But include the operator guide even inside generated/
!generated/operator-guide.md

# Ignore all drafts
drafts/*.md
```

Patterns follow a subset of Gitignore syntax: glob matching, `!` negation, and trailing `/` for directory matching. `.gitignore` is not read automatically.

## Key MCP Operations

| Category | Tools |
|---|---|
| Documents | `list_documents`, `create_document`, `get_document`, `get_document_location`, `manage_document_file`, `rename_document`, `rename_document_file`, `move_document_file`, `import_document`, `export_document`, `remove_document_file`, `restore_document_file`, `unmanage_document_file` |
| Sections | `get_section`, `get_section_tree`, `get_sections`, `table_of_contents`, `create_section`, `update_section`, `append_to_section`, `rename_section`, `insert_section_before`, `insert_section_after`, `move_section`, `reorder_sections`, `promote_section`, `demote_section`, `remove_section`, `split_section`, `patch_section`, `set_section_metadata`, `render_section_markdown`, `render_document_markdown` |
| History | `section_versions`, `get_section_version`, `diff_section_versions`, `switch_section_version` |
| Snapshots | `create_document_snapshot`, `document_snapshots`, `diff_document_snapshots`, `restore_document_snapshot` |
| Search | `full_text_search`, `semantic_search_sections`* (feature-gated), `find_by_title`, `find_by_tag` |
| Workspace | `get_workspace`, `set_workspace`, `validate_document` |

**\* Semantic search requires pre-computed embeddings** — VDS stores and indexes embeddings but does not generate them. Clients must provide embedding vectors via the `query_embedding` parameter.

## Semantic Search (Optional Feature)

Build with semantic search support:

```bash
cargo build --features semantic-search
```

**Platform support:** Linux and macOS only. The `hnsw_vector_search` dependency does not build on Windows.

**How it works:**
- VDS maintains an external embedding cache at `{cache_dir}/vds/workspaces/{workspace_id}/embeddings.zst`
- Embeddings are keyed by `(section_id, content_hash, model)` for automatic invalidation on edits
- Cache uses postcard serialization with CRC checksums and zstd compression (~1.1-1.3x ratio)
- The HNSW vector index is built per workspace generation for fast nearest-neighbor search
- Clients must provide pre-computed embeddings — VDS does not call embedding models

**Cache location by platform:**
- **macOS:** `~/Library/Caches/vds/workspaces/{workspace_id}/embeddings.zst`
- **Linux:** `~/.cache/vds/workspaces/{workspace_id}/embeddings.zst`
- **Windows:** `%LOCALAPPDATA%\vds\workspaces\{workspace_id}\embeddings.zst` (build not supported)

## Documentation

- [docs/overview.md](docs/overview.md) — architecture, data flow, and design decisions
- [docs/installation.md](docs/installation.md) — detailed installation and client setup

## Project Structure

```
src/
├── bin.rs                 ← CLI entry point
├── document.rs            ← shared document model (Document, Section, SectionVersion, TextEmbedding)
├── embedding_cache.rs     ← external zstd+postcard embedding cache (feature: semantic-search)
├── filesystem_service.rs  ← VDS 2 MCP server (filesystem-authoritative)
├── markdown.rs            ← Markdown parser and renderer
├── mcp.rs                 ← MCP tool parameter and result types
├── metadata.rs            ← .vds JSON metadata and recoverable transactions
├── search.rs              ← in-memory BM25 full-text index
├── semantic.rs            ← HNSW vector index for semantic search (feature: semantic-search)
├── service.rs             ← VDS 1 MCP server (legacy redb-backed)
├── storage.rs             ← VDS 1 redb storage layer (legacy)
└── workspace.rs           ← workspace discovery and materialization
tests/
├── markdown_golden.rs     ← exact-byte Markdown mutation golden tests
├── mcp_protocol.rs        ← end-to-end VDS 2 MCP protocol tests
├── mcp_smoke.rs           ← VDS 1 smoke tests
└── overview.rs            ← VDS 1 integration tests
```
