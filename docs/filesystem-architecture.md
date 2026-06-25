# Proposed Filesystem-Authoritative Architecture

> Status: design proposal for VDS 2.0. This document describes intended behavior, not the current implementation.

## Summary

VDS currently treats Markdown as an import and export format and treats a `redb` database as the authoritative document
store. That arrangement makes the database difficult to inspect or version with Git and leaves a live database file
locked inside directories managed by tools such as Dropbox.

VDS 2.0 will invert that relationship:

- Project Markdown files are authoritative for current visible document content and heading structure.
- Plain JSON files under `.vds` are authoritative for VDS-specific identity, metadata, versions, and snapshots.
- Runtime memory is a derived materialized view used for navigation, mutation, and search.
- An optional `redb` cache outside the project workspace may accelerate startup and preserve expensive derived data. It
  is disposable and never authoritative.

This remains a versioned document service rather than becoming a general-purpose repository indexer. Documents are
still exposed as section trees through the existing MCP-oriented domain model.

## Are There Three Copies of Every Document?

There are up to three persistent representations, but they are not three equivalent copies:

1. The Markdown file contains the current human-readable document.
2. `.vds` JSON contains durable VDS metadata and historical states that Markdown cannot represent safely.
3. An optional external `redb` file contains a disposable materialized cache.

There is also a fourth, non-persistent representation while VDS is running: the in-memory section model and search
indexes.

The representations intentionally overlap, but should not duplicate everything:

- Markdown is not duplicated into current-state JSON unless recovery requirements justify it.
- Immutable section versions may contain historical title and content because history cannot be reconstructed from the
  current Markdown file alone.
- `redb` should cache the current materialized model, fingerprints, search acceleration data, and embeddings. It does
  not need another complete copy of every historical version.
- Memory should normally contain current sections and current search indexes. Historical versions should be loaded on
  demand rather than retained in the normal working set.

Consequently, the system has multiple representations but only two durable authorities, each for a different class of
data. The cache can be deleted without losing a document or its VDS history.

## Authority Model

| Data | Authoritative source | Derived representations |
| --- | --- | --- |
| Current Markdown content | Project Markdown file | Parsed sections in memory and optional cache |
| Current heading hierarchy | Project Markdown file | Section tree in memory and optional cache |
| Document path | Workspace-relative filesystem path | Manifest and cache lookup keys |
| Document and section identity | `.vds` metadata, when managed | Embedded invisible markers and cache indexes |
| Tags, summaries, locks, and VDS metadata | `.vds` JSON | Memory and optional cache |
| Section versions | Immutable `.vds` JSON version objects | Loaded history and optional cache entries |
| Named snapshots | `.vds` JSON snapshot objects | Loaded snapshot views |
| Full-text index | Runtime memory | Optional external cache |
| Semantic embeddings | Recomputable derived data | Prefer external cache; optionally durable metadata by policy |
| Pending mutation | In-memory transaction until durable write | Recovery record when required |

This split is important. Calling both Markdown and JSON authoritative without assigning responsibility by data type would
create ambiguous conflict resolution.

## Filesystem Structure

The filesystem is divided into four areas with different lifetimes:

1. Project Markdown, which users edit and render.
2. Durable `.vds` JSON, which Git and synchronization tools may track.
3. Short-lived recovery files inside `.vds`, which permit interrupted writes to be diagnosed or completed.
4. Optional disposable cache files outside the project and its synchronization root.

### Complete Project Tree

```text
project/
├── README.md                              discovered Markdown document
├── AGENTS.md                              discovered Markdown document
├── docs/
│   ├── architecture.md                    discovered Markdown document
│   └── operations/
│       └── recovery.md                    discovered Markdown document
├── generated/
│   └── api.md                             ignored when matched by .vdsignore
├── .vdsignore                            discovery rules
└── .vds/
    ├── workspace.json                     workspace identity and format version
    ├── .gitignore                         ignores transient recovery content
    ├── documents/
    │   ├── <document-id-1>/
    │   │   ├── document.json              stable document metadata and source path
    │   │   ├── current.json               mutable current-state pointer and hashes
    │   │   ├── sections/
    │   │   │   ├── <root-section-id>.json current root section metadata
    │   │   │   ├── <section-id-1>.json     current section metadata
    │   │   │   └── <section-id-2>.json     current section metadata
    │   │   ├── versions/
    │   │   │   ├── <root-section-id>/
    │   │   │   │   └── <version-id>.json   immutable historical root state
    │   │   │   ├── <section-id-1>/
    │   │   │   │   ├── <version-id-1>.json immutable historical section state
    │   │   │   │   └── <version-id-2>.json immutable historical section state
    │   │   │   └── <section-id-2>/
    │   │   │       └── <version-id>.json
    │   │   └── snapshots/
    │   │       └── <snapshot-id>.json      immutable snapshot manifest
    │   └── <document-id-2>/
    │       └── ...
    ├── tombstones/
    │   └── documents/
    │       └── <document-id>.json           optional durable deletion record
    └── recovery/
        └── <transaction-id>/
            ├── intent.json                  operation and expected base hashes
            ├── state.json                   last completed durable step
            └── staged/                      same-filesystem temporary outputs
```

Only managed documents receive a directory under `.vds/documents`. Unmanaged Markdown exists only as a project file
and as runtime materialization.

Document directories are keyed by `DocumentId`, not source path. Renaming or moving a Markdown file therefore updates
`document.json` without moving all of its section history. It also avoids path-escaping problems for filenames that
contain spaces, Unicode, dots, or platform-specific reserved characters.

No central mutable `documents.json` index is required. Startup discovers document directories and constructs path and
ID indexes in memory. This avoids turning one global manifest into a merge-conflict hotspot. Duplicate IDs or duplicate
canonical paths are reported as integrity errors.

### File Responsibilities

| Path | Mutability | Git policy | Responsibility |
| --- | --- | --- | --- |
| `*.md` outside `.vds` | Mutable | Track according to project policy | Current visible content and headings |
| `.vdsignore` | Mutable | Track | Markdown discovery exclusions |
| `.vds/workspace.json` | Rarely mutable | Track | Workspace ID and metadata format version |
| `.vds/documents/<id>/document.json` | Mutable | Track | Stable identity, relative path, document metadata |
| `.vds/documents/<id>/current.json` | Mutable | Track | Current root/version pointers and source hash |
| `.vds/documents/<id>/sections/<id>.json` | Mutable | Track | Current section identity, metadata, and matching hints |
| `.vds/documents/<id>/versions/<section>/<version>.json` | Immutable | Track | Historical title, content, metadata, and edit attribution |
| `.vds/documents/<id>/snapshots/<id>.json` | Immutable | Track | Point-in-time tree and version references |
| `.vds/tombstones/documents/<id>.json` | Immutable | Track when enabled | Prevent ambiguous resurrection after deletion or sync |
| `.vds/recovery/**` | Transient | Ignore | Same-filesystem staging and crash recovery |
| External cache | Mutable and disposable | Never track | Optional startup, search, and embedding acceleration |

### Naming and Serialization Rules

- IDs used as path components use validated VDS-generated typed IDs. No user-supplied string becomes a metadata
  filename.
- Persisted project paths are workspace-relative, use `/`, contain no `.` or `..` segments, and never begin with `/`.
- Absolute paths, canonical host paths, and Dropbox-specific paths are runtime data only.
- JSON is UTF-8, pretty-printed with two-space indentation, and ends with one newline for readable Git diffs.
- JSON object fields have a stable documented order even though readers must not depend on ordering.
- Unknown fields are preserved when practical or rejected according to the format-version policy; they are never
  silently discarded during an unrelated edit.
- Timestamps use UTC RFC 3339. Hashes name their algorithm, for example `sha256:<hex>`.
- Immutable files are created with create-new semantics. An existing version or snapshot ID is never overwritten.
- Mutable JSON and Markdown are written to a sibling temporary file, synchronized as required by the durability policy,
  and atomically renamed over the destination.

### `.vdsignore`

The root `.vdsignore` uses Gitignore-compatible syntax. A representative file is:

```gitignore
# VDS always excludes .git and .vds, but listing them is harmless.
.git/
.vds/

# Generated and dependency documentation that should not be searchable.
generated/
target/
node_modules/

# Include an otherwise ignored project guide.
!generated/operator-guide.md
```

Nested `.vdsignore` files are not part of the first format. One root policy keeps discovery deterministic. Support for
nested policies can be added later if repository scale requires it.

### Optional External Runtime Area

The baseline VDS 2.0 design does not require an on-disk cache. If profiling later justifies one, the live cache must not
be stored below `.vds` or elsewhere in the synchronized project. On Windows it could use:

```text
%LOCALAPPDATA%/vds/
└── workspaces/
    └── <workspace-key>/
        ├── cache.json                      cache schema and generation metadata
        ├── materialized.redb               optional parsed/current-state cache
        ├── embeddings/                     optional model-specific derived data
        │   └── <model-key>.bin
        └── writer.lock                     runtime-only single-writer coordination
```

Equivalent operating-system cache roots should be used on Linux and macOS. The workspace key incorporates the
normalized canonical workspace path and `workspace_id`. The identifier prevents unrelated workspaces that occupy the
same path at different times from sharing a cache accidentally.

Neither correctness nor recovery may depend on any file in this external directory. Removing the whole
`<workspace-key>` directory while VDS is stopped must be safe.

### Workspace Manifest

`workspace.json` defines the metadata format and provides a stable workspace identity:

```json
{
  "format_version": 1,
  "workspace_id": "019...",
  "created_at": "2026-06-23T12:00:00Z"
}
```

### Document Metadata

`document.json` connects a VDS identity to a project file:

```json
{
  "format_version": 1,
  "document_id": "019...",
  "relative_path": "docs/architecture.md",
  "title": "Architecture",
  "description": null,
  "tags": ["design"],
  "created_at": "2026-06-23T12:00:00Z"
}
```

`relative_path` is canonical. `folder` and `filename` may be returned by APIs or denormalized into indexes, but are
derived from `relative_path` so that the three values cannot disagree. Paths use `/` in persisted data regardless of
the host platform.

### Current State

`current.json` contains VDS state that changes when the current document changes:

```json
{
  "format_version": 1,
  "content_hash": "sha256:...",
  "current_document_version": "019...",
  "root_section_id": "019...",
  "updated_at": "2026-06-23T12:05:00Z"
}
```

Section metadata files contain identity, current version, VDS metadata, and matching information. They should not be a
second mutable copy of current section content unless a recovery design explicitly requires that duplication.

A section metadata file could use the following shape:

```json
{
  "format_version": 1,
  "section_id": "019...",
  "current_version": "019...",
  "metadata": {
    "tags": ["storage"],
    "summary": null,
    "locked": false
  },
  "matching": {
    "embedded_marker": "019...",
    "last_known_title": "Storage",
    "last_known_ancestry": ["Architecture"],
    "last_known_ordinal": 2,
    "content_fingerprint": "sha256:..."
  },
  "updated_at": "2026-06-23T12:05:00Z"
}
```

Parent, child, level, and ordinal data are derived from the current Markdown hierarchy during materialization. Matching
hints are not a second authority for hierarchy; they exist only to reconnect externally edited headings to durable
identities.

### Immutable Versions

A version object contains the historical section state needed for diffing and restoration:

```json
{
  "format_version": 1,
  "version_id": "019...",
  "section_id": "019...",
  "title": "Storage",
  "content": "Historical section content...",
  "metadata": {},
  "created_at": "2026-06-23T12:05:00Z",
  "author": null,
  "change_summary": "Clarify cache authority"
}
```

Version files are immutable once published. Immutability produces understandable Git diffs, avoids write contention on
one growing history file, and makes interrupted mutations easier to recover. It may create many small files, so scale
and retention behavior must be measured before the format is frozen.

### Snapshot Manifests

A snapshot should reference immutable versions rather than copying every version body again:

```json
{
  "format_version": 1,
  "snapshot_id": "019...",
  "document_id": "019...",
  "label": "Before storage redesign",
  "change_summary": null,
  "created_at": "2026-06-23T12:10:00Z",
  "root_section_id": "019...",
  "sections": [
    {
      "section_id": "019...",
      "version_id": "019...",
      "parent_id": null,
      "ordinal": 0,
      "level": 0
    }
  ]
}
```

The snapshot records tree placement because immutable section versions contain section state, not necessarily the
document-wide hierarchy at the time of the snapshot.

### Tombstones

Document deletion normally appears naturally in Git as deleted Markdown and metadata. An optional tombstone is useful
when synchronized replicas may replay an old metadata directory or when deletion audit information must survive:

```json
{
  "format_version": 1,
  "document_id": "019...",
  "last_relative_path": "docs/obsolete.md",
  "deleted_at": "2026-06-23T12:15:00Z",
  "deleted_version": "019...",
  "reason": null
}
```

Tombstone retention is a policy decision. Keeping them forever prevents ID reuse but grows metadata; omitting them
relies on Git history and current filesystem state.

### Recovery Transactions

Recovery directories exist only while an authoritative multi-file mutation is being committed. `intent.json` records
the affected document, expected Markdown and metadata hashes, target paths, and planned immutable objects. `state.json`
records the last completed step. `staged/` holds closed temporary files on the same filesystem as `.vds`.

The root `.vds/.gitignore` should contain:

```gitignore
/recovery/
```

Dropbox may still observe short-lived recovery files, but it never encounters an open database there. Recovery cleanup
runs after a successful operation and during startup. A leftover directory is evidence of an interrupted operation and
must be inspected before its target document is made writable.

## Managed and Unmanaged Documents

Discovery should not force durable VDS metadata onto every Markdown file immediately.

- An unmanaged document is discovered, parsed, and searchable, but receives ephemeral runtime identities.
- A managed document has `.vds` metadata and can provide stable identity, VDS history, snapshots, and optimistic
  concurrency across restarts.
- The first durable VDS mutation promotes an unmanaged document to managed status.
- An explicit management command may promote a document before its first mutation.

This keeps ordinary repositories clean while preserving full VDS behavior where it is used. APIs must disclose whether
an identity is ephemeral or managed so clients do not retain ephemeral identifiers across service restarts.

## Invisible Metadata in Markdown

Stable section matching is difficult when users rename, move, duplicate, or reorder headings outside VDS. Invisible
markers may be added when a document becomes managed:

```markdown
<!-- vds:section id="019..." -->
## Storage Architecture
```

Markers are not rendered by normal Markdown renderers, but external formatters may remove or relocate them. Startup
reconciliation should therefore match sections in this order:

1. Valid embedded section identity.
2. Existing metadata plus a source fingerprint.
3. Heading ancestry, occurrence, anchors, and content similarity.
4. A new identity when no match is sufficiently reliable.

An uncertain match must not silently attach old history to the wrong section. VDS should emit a diagnostic and create a
new section identity when confidence is below a defined threshold.

## Discovery and `.vdsignore`

VDS discovers Markdown recursively beneath the workspace. `.vdsignore` uses Gitignore-compatible patterns and controls
which files enter the materialized model.

Default exclusions should include:

- `.git/`
- `.vds/`
- the configured external cache directory
- recognized dependency and generated-output directories
- files outside configured size and count limits

VDS should not follow a symlink that resolves outside the canonical workspace. Unreadable, oversized, or malformed
files produce diagnostics without preventing other documents from loading.

Whether `.gitignore` is also honored should be configurable. A tracked generated Markdown file can be relevant even
when some of its supporting artifacts are ignored, so `.gitignore` and `.vdsignore` do not express exactly the same
policy.

## Startup Materialization

Startup constructs one coherent generation of the workspace model:

1. Resolve and validate the workspace.
2. Load `.vdsignore` and discover eligible Markdown files.
3. Load the workspace manifest and managed-document metadata.
4. Consult the optional external cache using content and metadata hashes.
5. Parse new or changed Markdown files.
6. Reconcile managed section identities.
7. Build current document and section trees.
8. Build the in-memory full-text index.
9. Load or schedule semantic embeddings.
10. Publish the completed workspace generation to readers.

For small and medium workspaces, VDS should finish this process before advertising a ready service. Progressive startup
can be added for large workspaces, but search results must then report index completeness.

Modification time and file size are useful hints but are not sufficient cache keys. Git operations, Dropbox, and file
copy tools can preserve or manipulate timestamps. Content and metadata hashes are the reliable validation mechanism.

## Runtime Memory Model

Runtime memory contains the active state required by MCP operations:

```text
WorkspaceState
  documents by ID and relative path
  current sections by ID
  parent/child and document/section indexes
  metadata and current-version pointers
  full-text index
  semantic index, when enabled
  filesystem fingerprints
  diagnostics
```

Historical version bodies and snapshot bodies should normally remain cold in `.vds` and load on demand. Keeping every
historical object in memory would make memory consumption grow with repository history rather than current workspace
size.

A complete replacement generation can be built off-lock and atomically swapped into service. A file watcher can use the
same mechanism at document granularity: parse and index the replacement document off-lock, then replace its previous
generation in one short critical section.

## Full-Text Search

The initial full-text index may be entirely in memory. It should be a real inverted index rather than the current linear
substring scan.

```text
term dictionary
  term -> posting list

posting
  document ID
  section ID
  title frequency
  content frequency
  token positions and byte offsets

forward index
  section ID -> indexed terms

corpus statistics
  current section count
  document frequencies
  average title and content lengths
```

The forward index enables efficient removal when a section changes. Positions support phrase queries and useful
snippets. A `BTreeMap` term dictionary supports prefix ranges naturally; a `HashMap` favors exact lookup. The concrete
choice should be benchmarked against representative documentation repositories.

The first implementation should provide:

- Workspace-wide search with optional document and path filters.
- Separate title and content fields with title boosting.
- BM25-style ranking.
- Quoted phrase queries.
- Prefix queries.
- Result snippets and heading ancestry.

The MCP surface exposes this as `full_text_search`, which is distinct from the existing document-scoped
`search_sections`. Its request accepts a query plus optional `document_id` and `path_prefix` filters, AND/OR term
semantics, and a result limit. Results include both document identity and canonical relative path.

Stemming, fuzzy search, and language-specific analyzers can follow later. Tokenization must account for ordinary prose,
Unicode, code identifiers, underscores, hyphens, and filesystem paths.

The optional external cache may persist postings to improve cold startup, but persisted postings remain disposable. A
valid cache is an optimization; rebuilding the same logical index from authoritative files must always be possible.

## Semantic Search

Semantic search uses the same current section generation as lexical search. Unlike tokenization, embedding generation
can be expensive, so embeddings are strong candidates for the external cache. Cache keys must include:

- Content hash.
- Title and other embedded fields.
- Model and tokenizer identity.
- Embedding configuration version.

The vector index should be built once per workspace generation and incrementally updated. Rebuilding an HNSW graph for
every query does not scale.

Lexical, semantic, and structured section search remain distinct operations initially. Hybrid ranking can be introduced
later with an explicit fusion algorithm rather than by directly comparing incompatible BM25 and vector scores.

## Mutation and Durability

### Document File Lifecycle MCP Operations

VDS file operations are document-scoped and may address only discovered Markdown beneath the active workspace. VDS does
not expose a general-purpose filesystem API.

The lifecycle surface is:

| Tool | Purpose | Initial availability |
| --- | --- | --- |
| `get_document_location` | Return canonical path components, management state, and current content hash | Read mode |
| `manage_document_file` | Atomically promote discovered Markdown into `.vds` metadata and history | Read-transition mode |
| `move_document_file` | Move a managed document to another workspace-relative path | Filesystem mode |
| `rename_document_file` | Rename only the filename in its current folder | Filesystem mode |
| `remove_document_file` | Soft-delete Markdown while retaining history and a tombstone | After archive design |
| `unmanage_document_file` | Leave Markdown while removing active VDS management | After archive design |

Move and rename require the content hash returned by `get_document_location`, reject existing destinations, preserve
document and section IDs, and update metadata before publishing a replacement runtime generation. Rename is a
convenience operation over the same path-move primitive. The initial implementation rejects case-only renames on
Windows until an intermediate same-directory rename is implemented and recovery-tested.

Removal is soft by default: the active Markdown disappears, durable metadata and history move to an inactive archive,
and a tombstone records deletion. Permanent history purge, if added, must be a distinct and unmistakably destructive
operation.

The default mutation path should favor durability:

1. Read the current Markdown hash and compare it with the mutation's base state.
2. Reject or reconcile an external edit before overwriting it.
3. Construct the replacement Markdown and VDS metadata off-lock.
4. Write immutable version objects.
5. Atomically replace the Markdown file.
6. Atomically update mutable `.vds` pointers and metadata.
7. Replace the affected runtime document generation.
8. Update the optional cache asynchronously.

VDS cannot atomically commit several independent filesystem files as one transaction. Operation ordering, content
hashes, and recovery records must make each interrupted state detectable and repairable at startup.

Periodic write-behind may be offered as an explicit performance mode, but a successful default mutation should mean
that the Markdown and required history records are durable. Graceful shutdown flushing does not protect against process
termination or power loss.

Writing Markdown also introduces a round-trip concern. The current renderer normalizes Markdown and may produce broad
formatting changes after a small edit. Content mutations should use source spans where possible. Structural operations
may require a canonical rewrite, which must be documented and tested against comments, heading attributes, code blocks,
and line-ending conventions.

## External Changes and Concurrency

Each materialized document records the content hash from which it was parsed. Before a VDS write, the current disk hash
must match that base hash. A mismatch indicates an external editor, Git operation, Dropbox update, or another VDS
process changed the file.

The response may be:

- Automatic reload when VDS has no pending mutation.
- A conflict error when VDS and disk both changed.
- A three-way merge when a suitable merge engine is implemented.

Section version checks alone do not detect external filesystem changes. Both the logical section version and physical
content hash participate in optimistic concurrency.

Only one VDS writer should manage a workspace at a time. Any writer lease should be a small coordination artifact, not
an open binary database inside the synchronized workspace. Read-only processes may build independent in-memory views.

## Cache Policy

The external `redb` cache is optional and disposable:

- It is never committed to Git.
- It is never required to recover documents or VDS history.
- Its lock never resides in a Dropbox-managed project.
- It is invalidated by workspace, schema, tokenizer, model, content, and metadata versions.
- Corruption causes deletion or rebuild, not document loss.
- It may omit cold historical content.

This keeps the performance advantages of `redb` without assigning it authority. An entirely in-memory configuration can
disable the cache and rebuild on every startup.

## Open Design Decisions

- Retention policy for immutable VDS versions.
- Whether embedded invisible IDs are mandatory for managed documents.
- Exact recovery-record format and mutation ordering.
- Whether `.gitignore` augments `.vdsignore` by default.
- Startup threshold for progressive indexing.
- Whether the first release persists FTS postings or rebuilds them in memory.
- How managed document renames are detected when both path and content change.
- Whether Git integration eventually compacts history already protected by commits.
