# Transition from Database Authority to Filesystem Authority

> Status: migration proposal for VDS 2.0. The phases and compatibility rules in this document are not yet implemented.

## Purpose

This document describes how VDS can move from its current authoritative `redb` model to the filesystem-authoritative
architecture described in [filesystem-architecture.md](filesystem-architecture.md) without attempting a risky single
rewrite.

The transition changes persistence, startup, search, mutation durability, identity, and several CLI semantics. It
should therefore be released as VDS 2.0 even if the MCP surface remains substantially compatible.

## System A: Current Architecture

The current data flow is:

```text
Markdown file
  -> explicit import
  -> parsed document and section records
  -> authoritative redb database in or near the workspace
  -> MCP reads and mutations
  -> explicit Markdown rendering or export
```

Characteristics of System A:

- Markdown is a boundary format.
- The database owns current content, identity, metadata, versions, and snapshots.
- Project files can diverge from database state after import.
- Search scans sections from the database for each query.
- The semantic index is assembled at query time.
- An open database in `.vds/vds.db` may interfere with Git and filesystem synchronization.
- A database file is opaque to Git review and merge.

## System B: Target Architecture

The target data flow is:

```text
Project Markdown + .vds JSON metadata/history
  -> discovery, parsing, and reconciliation
  -> current workspace generation in memory
  -> full-text and semantic indexes
  -> MCP reads and durable filesystem mutations
  -> optional disposable external redb cache
```

Characteristics of System B:

- Current Markdown remains directly readable and editable by humans.
- `.vds` stores Git-friendly VDS metadata and history.
- Runtime state is rebuilt from authoritative files.
- Full-text search spans the workspace through an inverted index.
- External edits are detected with content hashes and filesystem events.
- The optional binary cache resides outside Git- and Dropbox-managed workspaces.
- Deleting the cache cannot delete a document or its history.

## Compatibility Principles

The migration should preserve concepts and tool intent wherever practical:

- A document is still exposed as a section tree.
- Section reads and mutations continue to use document and section identifiers.
- Version checks, versions, snapshots, and diffs remain meaningful for managed documents.
- MCP transport behavior remains unchanged.
- Search result shapes may be extended with relative path and document information.
- Tools whose old semantics depended on database ownership receive explicit filesystem semantics.

Compatibility must not hide meaningful changes. In particular, an unmanaged document's ephemeral ID must not be
presented as durable, and a filesystem conflict must not be reported as an ordinary section-version conflict.

## Tools Whose Meaning Changes

| Operation | System A | System B |
| --- | --- | --- |
| `create_document` | Creates database records | Creates a Markdown file and managed metadata |
| `import_document` | Copies a file's meaning into the database | Discovers, adopts, or copies a file into the workspace |
| `export_document` | Renders database state to a file | Copies or renders an authoritative project document |
| `rename_document` | Changes a stored name | Renames metadata, title, path, or some explicit combination |
| `get_document_location` | Not available | Returns path, folder, filename, management state, and hash |
| `manage_document_file` | Implicit database import | Promotes discovered Markdown into durable `.vds` management |
| `move_document_file` | Not available | Moves Markdown and updates durable path metadata transactionally |
| `rename_document_file` | Not available | Performs a filename-only path move |
| `remove_document_file` | Database record deletion | Soft-deletes Markdown and retains recoverable history |
| `unmanage_document_file` | Not available | Leaves Markdown while ending active VDS management |
| `delete_document` | Deletes database records | Deletes or unmanages a project file according to explicit policy |
| `set_workspace` | Opens another database | Flushes state, stops watchers, discovers the new workspace, and publishes a new generation |
| `set_database` | Selects authoritative storage | Selects or disables an external disposable cache |
| `search_sections` | Searches one document by scanning | Performs structured or lexical search over indexed current sections |

Some operations need new parameters rather than overloaded ambiguity. For example, renaming a display title and moving
a file are distinct filesystem operations.

## Transition Strategy

The safest approach is incremental replacement behind explicit boundaries. Permanent dual authority should be avoided.

### Phase 0: Record the Architecture Decision

Before implementation:

- Approve the authority matrix.
- Decide the managed/unmanaged lifecycle.
- Version the `.vds` JSON formats.
- Define path normalization and case-sensitivity behavior.
- Define default mutation durability.
- Define which compatibility changes require new MCP request fields.

Deliverables:

- Accepted architecture document.
- JSON schema drafts or equivalent Rust types.
- Representative workspace fixtures, including Git and Dropbox-like rename scenarios.

### Phase 1: Introduce Persistence Boundaries

The current service calls `DocumentStore` directly throughout its command handlers. Introduce interfaces that separate
current state, history, workspace files, and derived search:

```text
WorkspaceRepository
  discover documents
  read and atomically write Markdown
  manage paths and filesystem hashes

MetadataRepository
  read and write document/section metadata
  append immutable versions and snapshots

WorkspaceIndex
  publish and query current materialized state
  update document generations

CacheStore
  load and save disposable derived records
```

The interfaces should express domain-level atomic operations rather than exposing generic `put_section` calls. This
reduces the chance that a command writes only one part of a multi-file mutation.

System A may initially implement these interfaces as adapters over the existing store. This is a scaffolding step, not
a permanent second operating mode.

Exit criteria:

- Existing tests pass through the new boundaries.
- Service handlers no longer depend directly on `redb` tables.
- Storage errors distinguish authoritative I/O failures from disposable cache failures.

### Phase 2: Add Discovery and Read-Only Materialization

Implement System B as a read-only workspace view:

- Add `.vdsignore` discovery.
- Parse all eligible Markdown documents.
- Materialize current document and section trees in memory.
- Assign ephemeral IDs to unmanaged documents.
- Load stable IDs for managed fixtures.
- Report parse, path, and identity diagnostics.
- Preserve the existing database-backed mutation path temporarily, but do not mix its state into the new view.

During the transition, the read-only filesystem service is launched explicitly so legacy `serve` behavior does not
change silently:

```powershell
vds-mcp --workspace C:\path\to\project serve-v2
```

This mode advertises only implemented read, navigation, rendering, validation, workspace, and indexed section-search
tools. Mutation and history tools remain absent until their filesystem commit paths are durable.

Run the old and new readers in tests against exported fixtures and compare:

- Rendered Markdown.
- Tables of contents.
- Parent/child relationships.
- Section titles and content.
- Searchable fields.

Exit criteria:

- A workspace can restart with no authoritative database and reproduce the same current document content.
- Cache deletion has no effect on reconstructed results.
- Ignored and unsafe paths are tested across supported platforms.

### Phase 3: Implement In-Memory Full-Text Search

Replace the linear content scan with a workspace-wide inverted index:

- Define tokenizer and query syntax behavior.
- Index title and content fields separately.
- Store positions and byte offsets.
- Implement BM25-style scoring and title boosting.
- Add document and path-prefix filters.
- Include document identity, relative path, and heading ancestry in results.
- Reindex one document generation after a filesystem change.

The first implementation can rebuild entirely in memory. Persisting postings in `redb` should be justified by cold
startup benchmarks rather than assumed.

Exit criteria:

- Results are deterministic for a fixed workspace generation.
- Mutation and watcher tests never expose partially updated postings.
- Memory and startup benchmarks cover small, medium, and large documentation repositories.

### Phase 4: Add Git-Friendly Metadata and History

Implement `.vds` repositories for managed documents:

- Workspace manifest.
- Document records and canonical relative paths.
- Section identity and current-version metadata.
- Immutable section version objects.
- Immutable snapshot objects or manifests.
- Format-version validation and migrations.
- Recovery diagnostics for incomplete operations.

Promotion from unmanaged to managed should be explicit in code even when automatically triggered by the first mutation.
Promotion establishes durable document and section identities before writing the first VDS version.

Exit criteria:

- JSON changes are readable and reviewable in Git diffs.
- Independent immutable versions do not overwrite one another during ordinary synchronization.
- Cache deletion preserves versions, snapshots, and current identity.
- Ambiguous external section matching produces diagnostics rather than incorrect history attachment.

### Phase 5: Move Mutations to the Filesystem

Port mutation families in increasing order of structural risk:

1. Update or append section content.
2. Rename a heading.
3. Create and remove leaf sections.
4. Insert and reorder sibling sections.
5. Move, promote, demote, and split subtrees.
6. Restore versions and snapshots.
7. Rename, move, and delete documents.

Each mutation should follow one transaction coordinator that:

- Checks the base content hash and logical version.
- Constructs all outputs before changing authoritative files.
- Writes immutable history first.
- Uses atomic replacement for mutable files.
- Publishes one replacement runtime generation.
- Treats cache write failure as recoverable degradation.

Source-span editing should be used where it can preserve surrounding Markdown. Structural operations need golden tests
that make any canonical reformatting visible.

Exit criteria:

- A successful default mutation survives immediate process termination after its durable commit point.
- An interrupted operation is recoverable or produces actionable diagnostics.
- External edits cannot be overwritten silently.
- Every mutation produces the intended Markdown diff and VDS history diff.

### Phase 6: Move Semantic Search to the Materialized Model

Semantic search should stop creating a complete vector index for every query:

- Cache embeddings externally by content and model hash.
- Build one vector index per workspace generation.
- Incrementally replace changed document vectors.
- Keep lexical search available while embeddings are incomplete.
- Report semantic index readiness separately from lexical readiness.

Exit criteria:

- Repeated queries do not rebuild unchanged embeddings or the entire graph.
- Deleting the external cache degrades startup performance but not correctness.
- Model changes invalidate only incompatible embeddings.

### Phase 7: Introduce the External Disposable Cache

Only after profiling should VDS persist derived state in `redb`:

- Choose an operating-system cache location by default.
- Key caches by workspace identity and normalized canonical path.
- Store content and metadata hashes with every cached generation.
- Add schema, tokenizer, and embedding-model versions.
- Rebuild automatically after corruption or incompatibility.
- Allow `--no-cache` for a fully in-memory service.

Do not copy every immutable historical version into the cache by default. Cache current materialized state, expensive
embeddings, and any search data shown by benchmarks to improve startup materially.

Exit criteria:

- No open binary database exists inside the project workspace.
- Git and Dropbox operate only on Markdown and closed JSON files.
- A stale or missing cache never changes logical results.

### Phase 8: Migrate Existing Workspaces

Provide an explicit migration command rather than silently reinterpreting an existing `.vds/vds.db`:

```text
vds-mcp migrate --from-database <path> --workspace <path> --dry-run
vds-mcp migrate --from-database <path> --workspace <path>
```

### Migration File Layout

During migration, System A and the staged System B output must remain visibly separate. Given an existing database at
`project/.vds/vds.db`, a migration run uses a sibling staging directory:

```text
project/
├── .vds/
│   └── vds.db                              unchanged System A database
├── .vds2-staging/
│   └── <migration-id>/
│       ├── migration.json                  inputs, options, phase, and tool version
│       ├── report.json                     machine-readable validation report
│       ├── report.md                       human-readable decisions and warnings
│       ├── metadata/                       proposed contents of the new .vds directory
│       │   ├── workspace.json
│       │   ├── .gitignore
│       │   └── documents/
│       │       └── <document-id>/
│       │           ├── document.json
│       │           ├── current.json
│       │           ├── sections/
│       │           ├── versions/
│       │           └── snapshots/
│       └── markdown/                       proposed project-relative Markdown outputs
│           ├── README.md
│           └── docs/
│               └── architecture.md
└── ... existing project files
```

The staging directory is not a valid VDS 2.0 workspace and must never be discovered as project content. VDS excludes
`.vds2-staging/` unconditionally while migration support exists.

`migration.json` records enough information to resume validation but does not become an authority for migrated
documents:

```json
{
  "format_version": 1,
  "migration_id": "019...",
  "source_database": "C:/project/.vds/vds.db",
  "target_workspace": "C:/project",
  "source_database_hash": "sha256:...",
  "tool_version": "2.0.0",
  "phase": "validated",
  "created_at": "2026-06-23T12:00:00Z"
}
```

Publication cannot atomically replace the entire workspace. It proceeds from validated staging with a durable publish
plan:

1. Require the System A database to be closed.
2. Copy or move the legacy database to a user-approved backup outside the synchronized workspace.
3. Publish project Markdown with per-file atomic replacement after collision checks.
4. Atomically rename staged `metadata/` to `.vds` after the old `.vds` directory is no longer present.
5. Rescan the published workspace and compare it with the migration report.
6. Retain reports until the user confirms the migration.

The legacy backup location must be explicit. VDS must not silently place `vds.db` inside the new `.vds` tree, because
that would recreate the Git, Dropbox, and locking problem the migration is intended to remove.

After successful publication, the target tree is:

```text
project/
├── README.md
├── docs/
│   └── architecture.md
├── .vdsignore
└── .vds/
    ├── workspace.json
    ├── .gitignore
    └── documents/
        └── <document-id>/
            ├── document.json
            ├── current.json
            ├── sections/
            ├── versions/
            └── snapshots/

external-backup-directory/
└── <workspace-id>/
    └── vds-v1.redb                         closed rollback database
```

If the user elects not to publish rendered Markdown over existing project files, migration may adopt files whose
content hashes match the rendered database state. Any mismatch is a decision point: use the database render, use the
existing file and begin new VDS history, or resolve the difference manually.

Migration should:

1. Open the old database read-only if supported by the storage engine.
2. Inventory documents, sections, versions, and snapshots.
3. Resolve each document's destination workspace-relative path.
4. Detect missing source paths, path traversal, and destination collisions.
5. Render current Markdown into a staging directory.
6. Render `.vds` metadata and immutable history into staging.
7. Validate counts, identifiers, tree integrity, and rendered content hashes.
8. Present a migration report.
9. Atomically publish staged files where possible.
10. Leave the old database untouched as rollback material.

Documents with ambiguous paths require user resolution. A document's historical `source_path` must not be trusted as a
safe write destination without canonical workspace validation.

The migration report should include:

- Documents migrated, skipped, or requiring decisions.
- Current sections and versions written.
- Snapshots written.
- Destination paths and collisions.
- Content differences caused by canonical rendering.
- Metadata that could not be represented.

Exit criteria:

- A migrated workspace produces equivalent current renders, version lists, snapshot lists, and diffs.
- Re-running migration is idempotent or safely refused.
- The old database remains usable by the previous release until the user explicitly archives or deletes it.

### Phase 9: Cut Over and Remove Database Authority

After parity and migration testing:

- Make filesystem-authoritative mode the only writable mode.
- Retain legacy database support only in the migration reader.
- Remove the default `.vds/vds.db` path.
- Reinterpret cache configuration explicitly as disposable.
- Update onboarding, installation, and MCP documentation.
- Release as VDS 2.0.

Avoid indefinite dual-write behavior. If Markdown, JSON, and the old database are all writable authorities, recovery and
conflict rules become more complex than either architecture independently.

## Testing Strategy

### Golden Markdown Tests

For each mutation, record input Markdown, requested operation, expected output Markdown, and expected metadata changes.
Include:

- ATX and Setext headings.
- Heading attributes and invisible VDS markers.
- Duplicate headings.
- Skipped heading levels.
- Fenced code containing heading-like text.
- HTML blocks and comments.
- CRLF and LF line endings.
- Unicode titles and content.
- Empty documents and content before the first heading.

### Recovery Tests

Inject failures after each durable mutation step and restart VDS. Verify that startup either completes the operation,
rolls it back safely, or emits a diagnostic without presenting inconsistent current state.

### Synchronization Tests

Simulate:

- External edits between read and write.
- File rename while VDS is running.
- Dropbox-style temporary files and atomic replacements.
- Git checkout replacing many files quickly.
- Two VDS writers targeting one workspace.
- Case-only renames on case-insensitive filesystems.

### Search Tests

Verify ranking, phrases, prefixes, snippets, path filters, Unicode offsets, update removal, and generation consistency.
Compare a rebuilt index with an incrementally updated index for logical equivalence.

### Migration Tests

Construct System A databases containing duplicate names, missing source paths, deep trees, versions, and snapshots.
Migrate them and compare domain-level behavior rather than raw serialized bytes.

## Rollback

Before VDS 2.0 cutover, rollback means running the previous release against the preserved old database. VDS 2.0 should
never delete that database automatically.

After new filesystem mutations occur, the old database is stale. Returning to System A then requires importing or
reconstructing the newer Markdown, and may lose new `.vds` history semantics. The migration tool should state this
boundary clearly and optionally record the first System B mutation time.

## Principal Risks

- Stable section identity after arbitrary external edits is probabilistic without embedded IDs.
- Filesystem operations do not provide a transaction spanning Markdown and metadata files.
- One-version-per-file history may create excessive small-file churn.
- Source rewriting may produce noisy Markdown diffs.
- Loading every historical version into memory would make resource usage unbounded.
- An external cache can accidentally regain authority if code begins relying on data absent from Markdown and `.vds`.
- Permanent dual-mode support would multiply behavior and testing complexity.
- The meaning of import, export, rename, and delete can become surprising unless APIs are made explicit.

## Completion Definition

The transition is complete when:

- Current content can be reconstructed from project Markdown.
- Managed identity, metadata, versions, and snapshots can be reconstructed from `.vds` JSON.
- All navigation and search indexes can be rebuilt without the cache.
- No live binary database is required inside the workspace.
- Git and synchronization tools see ordinary Markdown and closed JSON files.
- Existing workspaces have a validated migration path.
- Cache loss affects performance only.
- The old authoritative database implementation is no longer used for normal reads or writes.
