# Versioned Document Service — Architecture Overview

Versioned Document Service (VDS) is an MCP server for long-form Markdown documents. Its purpose is to let agents and tools work with large documents through stable, structured operations instead of repeatedly rewriting whole files.

Large Markdown files are easy to corrupt when an agent has to regenerate or patch them from context. VDS addresses that by giving every document and section a durable identity, exposing surgical section-level edits with crash recovery, and maintaining a workspace-wide full-text index — all while keeping the Markdown file itself as the source of truth.

## Filesystem-Authoritative Design (VDS 2)

VDS 2 treats the project filesystem as authoritative. The Markdown file is the document; VDS metadata (`.vds/`) stores only what cannot be reconstructed from the file: stable IDs, version history, and snapshots.

This means:

- Humans and agents can edit Markdown side by side. VDS detects external changes using SHA-256 hashes and reloads the affected document.
- The workspace is safe to commit to Git or sync with Dropbox. All `.vds/` files are plain JSON.
- Removing `.vds/` and restarting loses history but not content. The Markdown file is always the ground truth.

## Document Model

VDS parses Markdown headings into a section tree:

```text
Document
  root Section            ← synthetic; holds content before the first heading
    Section (H1)
      Section (H2)
    Section (H1)
```

Each document has a stable `DocumentId`. Each section has a stable `SectionId` that survives title changes, moves, and content edits. Sections reference a `VersionId` that enables optimistic concurrency and historical diffs.

Section content is stored verbatim from the Markdown file. Fenced code blocks, HTML comments, and heading ID attributes (`{#anchor}`) are preserved exactly during both surgical edits (byte-range replacements) and canonical re-renders (structural mutations).

## Mutation Durability

Every write goes through a crash-recoverable transaction:

1. **Intent record** is written first — identifies the mutation type and expected state.
2. **Staged files** are written to a temporary directory.
3. **Atomic rename** moves staged files into place.
4. **Intent record** is deleted to confirm completion.

On restart, `recover_transactions` scans incomplete intents and either completes or rolls back each one. No write is ever partially visible.

## Conflict Detection

Callers supply an `expected_content_hash` with every mutation. If the Markdown file was externally edited between read and write, the hash check fails with `ExternalContentConflict`. This prevents silent overwrites when humans and agents edit concurrently.

## In-Memory Index

On startup, VDS reads all managed Markdown files and builds an in-memory `WorkspaceGeneration`:

- **WorkspaceState** — materialized documents, sections, and managed metadata
- **FullTextIndex** — BM25-scored inverted index over section titles and content

After each mutation, `reload_incremental` updates only the affected document's postings rather than rebuilding the full index. The filesystem watcher triggers a full reload when external changes are detected.

### Full-Text Search

The search index supports:

- **Term queries** — tokenized words scored by BM25
- **Prefix queries** — `term*` expands to all indexed terms with that prefix
- **Phrase queries** — `"exact phrase"` matches contiguous substrings in title or content
- **camelCase / PascalCase / acronym splitting** — `camelCase` indexes as both `camelcase` and `camel` + `case`
- **Path prefix filters** — constrain results to a subtree of the workspace
- **AND / OR modes** — `require_all_terms` controls whether all atoms must match

## Filesystem Watcher

A background thread uses `notify` to watch the workspace for file changes. It debounces burst events (300 ms drain loop) and filters editor backup files and OS noise. When a stable change is detected, it rebuilds the workspace generation atomically and publishes it under the existing `RwLock`.

`watcher_active` and `reload_count` fields in `get_workspace` allow clients to detect stale cached data.

## Workspace Lease

VDS acquires a lock file outside the synchronized project tree to prevent two writable VDS processes from targeting the same workspace simultaneously. Read-only materialization is available without acquiring the lease.

## .vds Metadata Layout

```
.vds/
├── workspace.json                 ← workspace identity (UUID) and format version
├── documents/
│   └── doc-<id>/
│       ├── document.json          ← stable document identity and metadata
│       ├── current.json           ← current content hash, root section ID, version
│       ├── sections/
│       │   └── sec-<id>.json      ← section identity, current version, matching hints
│       ├── versions/
│       │   └── sec-<id>/
│       │       └── ver-<id>.json  ← immutable section version (title, content, author, …)
│       └── snapshots/
│           └── snap-<id>.json     ← point-in-time document tree capture
├── inactive/
│   └── doc-<id>/                  ← soft-deleted documents with full archived history
└── recovery/
    └── <uuid>/                    ← in-progress transaction staging (cleaned on commit)
```

All files use `format_version: 1`. Version mismatches are detected at load time and reject the file before any fields are read.

## Module Map

| Module | Role |
|---|---|
| `document.rs` | Shared data types: Document, Section, SectionVersion, DocumentSnapshot, IDs, ValidationDiagnostic |
| `filesystem_service.rs` | VDS 2 MCP server — routing, mutation, search, workspace management |
| `markdown.rs` | Markdown parser (pulldown-cmark), section-tree construction, renderer |
| `metadata.rs` | .vds JSON reads/writes, recoverable transactions, catalog loading |
| `search.rs` | In-memory BM25 full-text index, camelCase tokenization, phrase queries |
| `workspace.rs` | Workspace discovery, .vdsignore matching, state materialization, watcher |
| `mcp.rs` | MCP parameter and result types, tool documentation strings |
| `service.rs` | VDS 1 legacy MCP server (redb-backed) |
| `storage.rs` | VDS 1 redb storage layer (legacy) |

## Typical Agent Workflows

**Read a section:**
```
get_workspace → list_documents → table_of_contents → get_section
```

**Edit a section safely:**
```
get_section → note current_version and content_hash
update_section { expected_content_hash: "<hash>", content: "…" }
```

**Search across the workspace:**
```
full_text_search { query: "\"exact phrase\"", max_results: 10 }
full_text_search { query: "camelCase tokenization*", require_all_terms: true }
```

**Create a snapshot before a large edit:**
```
create_document_snapshot { document_id: "…", label: "before refactor" }
```

**Restore after a bad edit:**
```
document_snapshots → diff_document_snapshots → restore_document_snapshot
```
