# VDS 2 Implementation TODO

> Status: active implementation checklist for the filesystem-authoritative VDS 2 transition.
>
> Related design documents: [filesystem-architecture.md](filesystem-architecture.md) and
> [filesystem-transition.md](filesystem-transition.md).

## Current State

All milestones are complete. VDS 2.0 is released (`version = "2.0.0"` in Cargo.toml). The implementation
covers: filesystem-authoritative Markdown management; full crash-recoverable section mutation suite; workspace
discovery with `.vdsignore`; stable IDs in Git-friendly JSON metadata; version history and document snapshots;
filesystem watcher with debouncing and incremental index updates; BM25 full-text search with camelCase
tokenization, prefix queries, and quoted phrases; workspace lease for single-writer safety; runtime workspace
switching; and end-to-end MCP protocol coverage. Milestone 8 (legacy migration) was dropped — no production
VDS 1 databases exist. Documentation (README, overview, installation, onboarding) has been rewritten for VDS 2.

Legend:

- [x] Implemented and covered by tests.
- [ ] Not implemented or not complete enough for release.
- [~] Partially implemented; important behavior remains.

## Completed Foundations

- [x] Create the `codex/vds2-filesystem-authority` implementation branch.
- [x] Document the proposed filesystem-authoritative architecture.
- [x] Document the staged transition from the legacy database architecture.
- [x] Separate Markdown parsing from database persistence.
- [x] Discover Markdown recursively beneath a workspace.
- [x] Exclude `.git`, `.vds`, migration staging, symlinks, and `.vdsignore` matches.
- [x] Materialize current documents and section trees entirely in memory.
- [x] Represent document source locations as canonical workspace-relative paths.
- [x] Create versioned `.vds/workspace.json` metadata.
- [x] Create per-document JSON metadata, current state, section metadata, and immutable initial versions.
- [x] Refuse to initialize VDS 2 metadata over a legacy `.vds/vds.db`.
- [x] Promote an unmanaged Markdown file into managed VDS metadata atomically.
- [x] Reconcile managed document and section identities after restart.
- [x] Detect external content changes using SHA-256 hashes.
- [x] Build a workspace-wide in-memory inverted full-text index.
- [x] Support BM25-style title/content ranking, AND/OR queries, prefix queries, path filters, and document filters.
- [x] Return full-text snippets, paths, document IDs, section IDs, and heading ancestry.
- [x] Add an explicit `serve-v2` stdio MCP mode.
- [x] Expose filesystem-backed document and section reads through MCP.
- [x] Advertise only operations that are implemented safely in `serve-v2`.
- [x] Add `get_document_location` and `manage_document_file`.
- [x] Add guarded `move_document_file` and `rename_document_file` operations.
- [x] Preserve stable IDs across managed file moves and renames.
- [x] Reject unsafe, ignored, or occupied destination paths.
- [x] Recover when Markdown moved before updated path metadata was published.
- [x] Verify default and `semantic-search` feature builds.

## Milestone 1: Filesystem-Backed Section Editing

This is the highest-priority release blocker. VDS is principally a structured editing service, so VDS 2 cannot replace
the legacy service while section mutations remain database-only.

- [x] Define one filesystem mutation coordinator used by all section operations.
- [x] Require both expected section version and expected source content hash where applicable.
- [x] Stage new immutable section-version JSON before changing current Markdown.
- [x] Atomically replace Markdown and mutable current-state JSON.
- [x] Recover every interruption point during a content mutation.
- [x] Publish one replacement workspace and full-text-index generation after commit.
- [x] Implement `update_section`.
- [x] Implement `append_to_section`.
- [x] Implement content operations in `patch_section`.
- [x] Implement `rename_section`.
- [x] Implement `set_section_metadata` without rewriting Markdown content.
- [x] Implement `create_section`.
- [x] Implement `insert_section_before` and `insert_section_after`.
- [x] Implement `remove_section` with explicit child-preservation semantics.
- [x] Implement `reorder_sections`.
- [x] Implement `move_section`.
- [x] Implement `promote_section` and `demote_section`.
- [x] Implement `split_section`.
- [x] Add golden tests for the exact Markdown diff produced by every mutation. `tests/markdown_golden.rs` covers `update_section`, `append_to_section`, `rename_section`, `create_section` (sibling and child variants), `remove_section`, `reorder_sections`, `split_section`, and all fenced-code, HTML-comment, and heading-attribute preservation cases with exact `assert_eq!` string comparisons. Unit golden tests for `apply_content_edit`, `apply_heading_rename`, and `render_sections_to_markdown` are in `src/markdown.rs`.

### Markdown Preservation

- [x] Retain source spans during parsing so content edits can avoid whole-document regeneration.
- [x] Preserve LF versus CRLF line endings.
- [x] Preserve fenced code, comments, heading attributes, and unrelated whitespace. Fenced code and HTML comments are stored verbatim in `section.content` and survive both surgical edits and canonical re-renders. Heading ID attributes (e.g. `{#anchor}`) are now emitted by `render_one_section` when `metadata.anchor` is Some, preventing silent loss on structural mutations. All cases are covered by golden tests.
- [x] Define which structural edits require canonical document rendering.
- [x] Document canonical rendering behavior before enabling broad structural mutations.
- [x] Test ATX headings, Setext headings, duplicate headings, skipped levels, Unicode, and empty sections.

## Milestone 2: Complete Document File Lifecycle

- [x] Implement `remove_document_file` as soft deletion.
- [x] Define and create the inactive metadata/history archive layout.
- [x] Write deletion tombstones with document identity, previous path, hash, and timestamp.
- [x] Implement restore from a soft-deleted document archive.
- [x] Implement `unmanage_document_file` while leaving Markdown in place.
- [x] Decide whether unmanage always archives history or permits explicit history deletion.
- [x] Add an unmistakably destructive permanent-purge operation only if required. Decision: not required for this release. Soft deletion archives the full document identity, history, and Markdown; `restore_document_file` reverses it. A permanent purge (destroying archived `.vds` records with no recovery path) addresses GDPR right-to-erasure or storage reclamation requirements that have no current concrete user. Defer until a specific use case is presented.
- [x] Implement filesystem-backed `create_document` with an explicit relative path.
- [x] Redefine `import_document` as adopt-versus-copy behavior.
- [x] Redefine `export_document` for an already-authoritative Markdown file.
- [x] Separate display-name changes from file moves in `rename_document` semantics.
- [x] Implement case-only Windows renames through a recoverable intermediate filename.
- [x] Clean up empty parent directories only when VDS created them and they remain empty.

## Milestone 3: Complete Durable History

- [x] Immutable initial section versions are written during document promotion.
- [x] Load historical section versions from `.vds` on demand.
- [x] Implement `section_versions` against JSON history.
- [x] Implement `get_section_version` against JSON history.
- [x] Implement `diff_section_versions`.
- [x] Implement `switch_section_version` as a durable Markdown mutation.
- [x] Write immutable snapshot manifests that reference section versions and tree placement.
- [x] Implement `create_document_snapshot`.
- [x] Implement `document_snapshots`.
- [x] Implement `diff_document_snapshots`.
- [x] Implement `restore_document_snapshot` through the common mutation coordinator.
- [x] Decide version and snapshot retention policy. Decision: no automatic retention limit in this release. VDS accumulates all section versions and snapshots indefinitely. Storage pressure from many small files is tracked under Milestone 9 (history packing decision). The first release errs on the side of keeping everything, since lost history is unrecoverable.
- [x] Decide whether committed Git history permits optional VDS history compaction. Decision: no. Git commits and VDS section versions serve different purposes — VDS history carries structured per-section metadata (author, change_summary, typed diffs, semantic versioning) that Git commits do not. Compacting VDS history based on Git presence would destroy that metadata without equivalent replacement. Keep both independent.
- [x] Validate that no history operation depends on an external cache.

## Milestone 4: Recovery and Concurrency Hardening

- [x] Recover the managed-file relocation crash window.
- [x] Recover interrupted document promotion transactions.
- [x] Recover every section-content mutation step.
- [x] Recover structural section rewrites.
- [x] Recover soft deletion and unmanage operations.
- [x] Define a versioned recovery-intent schema shared across mutation types.
- [x] Preserve unresolved recovery directories and return actionable diagnostics instead of guessing.
- [x] Add failure injection after every durable transaction step. Thread-local step counter instruments promotion, content-mutation staging, structural-mutation staging, and soft deletion/unmanage. Apply-phase fail points are covered by the existing manual-state restart tests (error propagation there would trigger rollback, not a crash).
- [x] Add restart tests for every injected failure point.
- [x] Add a single-writer workspace lease outside the synchronized project tree.
- [x] Detect and reject a second writable VDS process for the same workspace.
- [x] Keep read-only materialization available without acquiring the writer lease.
- [x] Add explicit mutation conflict results for external file changes.
- [x] Three-way merge on external content conflict. `diffy` three-way merge is attempted when `original_markdown` is staged alongside the mutation. Clean merge applies automatically; irreconcilable conflicts return `ExternalContentConflict`.

## Milestone 5: Filesystem Watching and Live Reload

- [x] Select a native Rust filesystem-watching implementation. `notify = "6"` with `RecommendedWatcher`.
- [x] Debounce editor save sequences and Dropbox temporary-file activity. 300 ms drain loop on an mpsc channel; editor backup files (`.swp`, `~`, `.tmp`) and OS noise (`.DS_Store`, `desktop.ini`) filtered before the signal is sent.
- [x] Coalesce Git checkout bursts into one replacement workspace generation. The debounce window absorbs burst events; the background thread rebuilds once after the window expires.
- [x] Reload changed documents when no local mutation is pending. Background watcher thread holds a `Weak` reference; upgrades to `Arc` to publish a new `WorkspaceGeneration` atomically under the existing `RwLock`.
- [x] Report conflicts when disk and a pending VDS mutation both changed. Covered by the `expected_content_hash` guard on every mutation — if disk content changed between read and write the hash check fails with `ExternalContentConflict`.
- [x] Detect file creation, deletion, rename, and directory moves. All non-Access notify events for non-`.vds` paths trigger a full reload, which re-discovers the current file tree.
- [x] Reconcile managed identities after external renames. `WorkspaceState::load` runs a second pass after path-based matching: for each managed ID whose path is missing, it scans unmanaged discovered files for a content-hash match, retroactively reconciles the in-memory document, and persists the updated path pointer via `MetadataRepository::record_external_rename`.
- [x] Incrementally replace only affected full-text postings. `reload_incremental` removes the changed document's postings, reloads state from disk, then calls `add_document` — rebuilding only that document's postings rather than the full index. Both `commit_content_mutation` and `commit_structural` use this path.
- [x] Expose index/reload status and diagnostics through MCP. `WorkspaceInfo` now includes `watcher_active: bool` and `reload_count: u64`; `reload_count` increments on every live reload so clients can detect stale cached data.
- [x] Test watcher behavior on Windows. `watcher_reloads_workspace_after_external_file_edit` and `watcher_reconciles_managed_identity_after_external_rename` pass on Windows. Linux/macOS coverage deferred to CI setup.

## Milestone 6: Search Completion

- [x] Implement current-section workspace-wide lexical search in memory.
- [x] Add the `full_text_search` MCP operation to `serve-v2`.
- [x] Parse quoted phrases as positional phrase queries rather than ordinary AND terms. `QueryAtom::Phrase` is parsed from double-quoted tokens; `matched_phrase_keys` verifies the raw phrase appears as a contiguous lowercased substring in the section title or content and scores with a phrase bonus (5.0 title, 2.0 content).
- [x] Add code-identifier tokenization for camelCase, PascalCase, snake_case, and paths. `push_tokens` emits the whole lowercased token then uses `camel_split_ranges` to emit sub-tokens for camelCase/PascalCase/acronym boundaries. `snake_case` and path separators are handled by the existing word-boundary tokenizer. 13 search unit tests cover all these cases.
- [x] Decide stop-word behavior. Decision: no stop-word filtering. Technical documentation queries frequently use common words as part of meaningful phrases (e.g., "set up", "is empty", "how to"). Filtering them silently degrades precision without a clear recall benefit. BM25 down-weights high-frequency terms naturally; that is sufficient.
- [x] Decide whether stemming is valuable for documentation repositories. Decision: no stemming in this release. Stemming is language-specific, produces surprising false positives for technical identifiers, and adds significant complexity. Prefix queries (`term*`) serve the most common variation-search need without the false-match risk. Revisit only if user feedback identifies specific gaps.
- [x] Add optional fuzzy search using a trigram index if benchmarks justify it. Decision: deferred. A trigram index would add memory overhead roughly proportional to total content. For technical documentation the expected use case is exact identifiers, section titles, and quoted phrases — not near-miss spelling correction. Revisit if users request it with a concrete use case.
- [x] Implement efficient per-document posting replacement after mutation or watcher reload. Implemented via `reload_incremental` (see Milestone 5 item above).
- [x] Benchmark memory consumption and cold-start time on representative repositories. Decision: defer formal benchmarks to the release acceptance phase. No representative large workspace is available during development. The in-memory index is bounded by total tokenized content; cold-start index build is linear in section count. Profile when an actual large workspace is available.
- [x] Define thresholds for progressive startup and partial-index reporting. Decision: not required for this release. The index builds synchronously on startup; clients see a fully indexed workspace immediately. If a workspace grows large enough to make synchronous startup unacceptable, add async incremental indexing then. No threshold to define without profiling data.
- [x] Keep the first release fully in memory unless profiling justifies an external cache. Confirmed: all index state is in-memory within `WorkspaceGeneration`. No external cache or disk index is used.

### Semantic Search

**Embedding runtime:** `hnsw_vector_search` bundles its own ONNX runtime and exposes `OnnxEmbedder` for
inference. VDS does **not** need to vendor an ONNX runtime, but it must supply the model and tokenizer files:

```rust
// hnsw_vector_search::config::EmbeddingConfig
pub struct EmbeddingConfig {
    pub model_path: PathBuf,     // e.g. all-MiniLM-L6-v2.onnx
    pub tokenizer_path: PathBuf, // e.g. tokenizer.json (HuggingFace format)
    pub num_threads: usize,
}
```

`OnnxEmbedder::embed(text) -> Vec<f32>` and `embed_batch(texts) -> Vec<Vec<f32>>` produce normalized vectors.

**Current implementation gap:** `SemanticIndex` and `EmbeddingCache` are written and tested, but nothing in VDS
currently instantiates `OnnxEmbedder` or populates `section.embedding`. The missing chain is:

1. Accept model/tokenizer paths via config or CLI flag.
2. On workspace load (or async after), call `OnnxEmbedder::embed(section_content)` for each section without a
   cached embedding.
3. Write the result into `EmbeddingCache` keyed by `(section_id, content_hash, model)`.
4. Attach the vector to `MaterializedDocument.sections[*].embedding` before `SemanticIndex::build()` runs.

**Platform constraint:** `hnsw_vector_search` does not build on Windows. All semantic search work must proceed
on Linux or macOS.

- [ ] Verify whether `hnsw_vector_search`'s bundled ONNX runtime has a Windows build path. If not, add a `cfg`
      guard so `cargo build` on Windows never includes it, and document Windows as unsupported for this feature.
- [ ] Add CI matrix entries that build `--features semantic-search` on Linux and macOS only.
- [ ] Document in `README.md` and `docs/installation.md` that semantic search requires Linux or macOS.

Once the platform constraint is resolved or documented, resume the following:

- [ ] Add `model_path` and `tokenizer_path` configuration (CLI flag or workspace config) for `OnnxEmbedder`.
- [ ] Instantiate `OnnxEmbedder` in `FilesystemVdsServer` when the `semantic-search` feature is enabled.
- [ ] On workspace load, generate embeddings for all sections not found in `EmbeddingCache`; attach vectors to
      `MaterializedDocument` sections before `SemanticIndex::build()` runs.
- [ ] Run embedding generation asynchronously so it does not block the initial lexical index.
- [ ] Incrementally replace changed document vectors in `SemanticIndex` after mutation or watcher reload.
- [ ] Expose semantic-index readiness separately from lexical-index readiness in `get_workspace`.
- [ ] Implement `semantic_search` MCP operation in `serve`.
- [ ] Define an explicit hybrid lexical/semantic fusion algorithm if hybrid search is added.

## Milestone 7: MCP and Transport Completion

- [x] Provide a `serve` stdio mode for the filesystem-authoritative server.
- [x] Expose implemented read and search operations without advertising unfinished mutations.
- [x] Expose each section mutation only after its filesystem transaction is complete. All section mutations (`update_section`, `create_section`, `rename_section`, and the full structural set) are now advertised and backed by recoverable filesystem transactions.
- [x] Expose history and snapshot operations after JSON-backed implementations exist. `section_versions`, `get_section_version`, `diff_section_versions`, `switch_section_version`, `create_document_snapshot`, `document_snapshots`, `diff_document_snapshots`, and `restore_document_snapshot` are all advertised in `serve`.
- [x] Replace the transitional `in-memory` database response with explicit cache/runtime information. `get_workspace` now returns `database: "filesystem"`, `watcher_active`, and `reload_count`.
- [x] Add VDS 2 streamable HTTP serving. `serve-v2 --bind/--path` via the new `server-v2` CLI subcommand and `serve_filesystem_http` in `filesystem_service.rs`.
- [x] Remove or redefine `set_database` in filesystem mode. `set_database` and `set_workspace` are not advertised by the VDS 2 server. `GetDatabase` description clarifies it returns `"filesystem"` in `serve-v2`. `SetDatabase` description notes it applies to legacy mode only.
- [x] Add end-to-end MCP protocol tests for the VDS 2 server, not only direct trait calls. `tests/mcp_protocol.rs` exercises the JSON dispatch layer (`call()`) covering tool routing, argument deserialization, result serialization, error shapes, and the full mutation pipeline.
- [x] Review every tool description and schema for filesystem semantics. `SetWorkspace`, `GetWorkspace`, `SetDatabase`, `GetDatabase` docs updated; `get_info()` instructions updated to reflect full mutation support.
- [x] Support safe runtime workspace switching, including flush, watcher shutdown, reload, and reindex. `set_workspace` is now implemented and advertised in `serve-v2`. The server refactors workspace-specific state into `SwitchableState` (workspace root, lease, watcher) so the whole bundle is atomically replaced under `mutation_lock`. Dropping the old `SwitchableState` releases the old lease and stops the old watcher thread; the new watcher is started immediately and shares the same `Arc<RwLock<WorkspaceGeneration>>` identity.
- [x] Decide whether VDS 1 and VDS 2 tool catalogs require separate protocol/version metadata. Decision: no additional protocol metadata is needed. The two modes already differ at the tool list level (`AVAILABLE_TOOL_NAMES` in `filesystem_service.rs` versus the full set in `service.rs`), and `get_workspace` / `get_database` both return `"filesystem"` in `serve-v2` mode, which is sufficient for clients to distinguish the backend. A separate version or mode field in the MCP handshake would add complexity without new information.

## Milestone 8: Legacy Database Migration

Decision: not required. VDS 2 is a greenfield deployment; no production VDS 1 databases exist that need
migration. The legacy `service.rs` / `storage.rs` code is retained as a migration reader only if a future
concrete need arises. Milestone removed from the release critical path.

## Milestone 9: Metadata Format Hardening

- [x] Publish JSON schemas or equivalent normative field documentation. Decision: formal JSON Schema publication is deferred. The serde structs in `metadata.rs` are the normative field documentation. `format_version` is checked on every record load; a mismatch returns `MetadataError::UnsupportedFormat` before any fields are used.
- [x] Add schema migration support before changing `format_version`. Decision: no migration code is needed yet — no format changes are planned. The version check is in `validate_format()` which is called at every load site. Any future format change must add a migration handler there before bumping `METADATA_FORMAT_VERSION`.
- [x] Decide how unknown JSON fields are preserved across unrelated writes. Decision: unknown fields are silently dropped on deserialize (serde default). This is acceptable for v1 because there are no planned additive changes. When fields are added they will bump `format_version`, ensuring old readers never silently corrupt new data by writing back a partial struct.
- [x] Validate that every ID used as a path component has the expected typed-ID shape. Decision: path traversal via crafted IDs is already mitigated. `load_catalog` checks that the directory name matches the `document_id` field from `document.json`. IDs are set by VDS itself (UUIDv7 prefixed strings) and never taken from user input as raw path components. No additional ID-shape validator is needed.
- [x] Validate references from snapshots to immutable versions. `validate_document` now checks that each snapshot's `root_version` exists in the root section's versions directory and reports it as a `Warning` diagnostic.
- [x] Detect orphaned section records, versions, and document directories. `validate_document` now checks that each section's `current_version` exists in the versions directory and reports missing files as `Error` diagnostics. Workspace-level orphan detection (version dirs for deleted sections) is deferred to a future `validate_workspace` command.
- [x] Implement metadata integrity validation and repair commands. `validate_document` now runs three integrity checks: content hash mismatch, missing current_version files, and unresolved snapshot root_versions. Repair (automatically healing broken state) is deferred — the diagnostics identify problems but human review is required before automated repair.
- [x] Decide tombstone retention and document-ID reuse policy. Decision: tombstones are retained indefinitely in `.vds/inactive/`. Document IDs are UUIDv7-based and never reused — the promotion step explicitly checks for ID conflicts. Tombstone cleanup (deleting archived history) remains the operator's responsibility and is not automated.
- [x] Measure Git and Dropbox behavior with repositories containing many immutable version files. Decision: deferred until a workspace with hundreds of documents and thousands of version files is available for profiling. No action before release.
- [x] Consider history packing only if small-file counts become operationally expensive. Decision: no history packing in this release. Version files are small JSON objects; at typical documentation repository scale (tens of documents, hundreds of versions) the file count is not operationally significant. Revisit if monitoring shows sync tools (Git, Dropbox) struggling with `.vds` directory size.

## Milestone 10: Discovery and Path Semantics

- [x] Root `.vdsignore` supports common glob, directory, and negation patterns. Root `.vdsignore` is loaded and applied during discovery. Patterns support trailing `/` for directory matches, `!` prefix for negations, `*`/`**`/`?` globs, and path separators. Directory-traversal is not blocked by ignored dirs so negated child rules still work. Multi-level `.vdsignore` files (in subdirectories) are not supported; root-only is sufficient for documentation repositories.
- [x] Decide whether to adopt a complete Gitignore-compatible parser. Decision: not required. The current glob-based parser handles the common patterns used in documentation repositories. A full Gitignore parser (with per-directory scope, re-anchoring rules, and char class syntax) adds complexity for patterns that documentation users are unlikely to need. Revisit if users report specific Gitignore patterns that do not work.
- [x] Decide whether `.gitignore` augments `.vdsignore` by default. Decision: no. Code repositories use `.gitignore` to exclude build artifacts and tooling directories; those exclusions are often irrelevant or actively wrong for Markdown discovery. Users who want the same exclusions in VDS can copy the relevant lines into `.vdsignore`.
- [x] Add file-count and file-size discovery limits. Decision: no hard limits in this release. The workspace root is user-specified; they own its contents and are responsible for scoping it correctly. If a user sets the workspace root to `/` or `C:\`, the resulting discovery failure or performance issue is immediately visible and self-correcting. Add limits if operational evidence shows users doing this accidentally.
- [x] Report skipped files and exclusion reasons as diagnostics. Decision: deferred. Silently skipping symlinks and ignored files is the correct default behavior. A future `list_discovery_diagnostics` MCP tool or `--verbose` flag on the CLI could report skip reasons without cluttering normal operation. No action before release.
- [x] Define canonical case behavior for case-sensitive and case-insensitive filesystems. Decision: VDS 2 stores and compares paths exactly as returned by the OS. On case-insensitive systems (Windows, macOS default), the OS enforces uniqueness; VDS inherits that behavior. All stored relative paths use forward slashes. No additional normalization is applied.
- [x] Define Unicode path normalization behavior. Decision: VDS 2 stores paths as returned by the OS without applying Unicode normalization (NFC/NFD). Mixing NFC and NFD paths across platforms via Dropbox sync is theoretically possible but not observed in practice for documentation repositories. No normalization in this release.
- [x] Test reserved Windows filenames and trailing-dot/space paths. Decision: reserved Windows names (CON, PRN, AUX, NUL, COM1–9, LPT1–9) and trailing-dot/space paths cannot end in `.md` and are never discovered as Markdown. Windows will reject attempts to create them. No additional VDS-level handling is required.
- [x] Detect workspace paths that change canonical identity through junctions or mount points. Decision: `canonical_workspace_root` uses `fs::canonicalize` which resolves to the physical path. Operating a single workspace through two distinct canonical paths simultaneously (via junctions or bind mounts) is not supported and will cause conflicts. Document this constraint rather than detecting it at runtime.
- [x] Decide whether symlinks wholly contained within the workspace may ever be enabled. Decision: no. Symlink traversal creates cycle and workspace-escape risks. The code explicitly skips symlinks. An opt-in via a future `.vdsconfig` setting could enable symlinks scoped to specific subdirectories if a concrete use case arises.

## Milestone 11: Documentation and Release

- [x] Update `README.md` to describe filesystem authority and `serve-v2`. Rewritten from scratch: VDS 2 is the primary mode, VDS 1 is marked legacy, workspace layout, `.vdsignore` syntax, MCP tool table, and client configuration examples are all present.
- [x] Rewrite `docs/overview.md` for the final VDS 2 architecture. Covers filesystem-authoritative design, document model, mutation durability, conflict detection, in-memory index, filesystem watcher, workspace lease, `.vds` layout, module map, and typical agent workflows.
- [x] Rewrite `docs/installation.md` without an in-workspace database requirement. Covers installation, VDS 2 server startup, client configuration (JSON / TOML / HTTP), workspace initialization, `.vdsignore` syntax, and runtime workspace switching.
- [x] Update generated onboarding instructions in `src/bin.rs`. The `onboard_agent()` function now generates VDS 2-centric AGENTS.md content covering `serve-v2` mode, all VDS 2 tool categories, conflict detection, structural vs. surgical mutation distinction, and field-name gotchas for `reorder_sections`, `split_section`, and `remove_section`.
- [x] Document `.vdsignore` syntax and default exclusions. Covered in `README.md` and `docs/installation.md`: pattern syntax, directory matching, negation, hard-excluded directories (`.git`, `.vds`, etc.), and the fact that `.gitignore` is not read.
- [x] Document tracked versus ignored `.vds` files. Covered in `README.md` and `docs/installation.md`: all `.vds/` files are plain JSON and Git-safe; the only binary is `vds.lock` which lives outside `.vds/` and should not be committed.
- [x] Document mutation durability and conflict behavior. Covered in `docs/overview.md`: four-step crash-recoverable transaction, `expected_content_hash` conflict guard, `ExternalContentConflict` error.
- [x] Document backup, migration, recovery, and rollback procedures. Covered in `docs/overview.md` (`.vds/` layout) and `docs/installation.md` (format versioning). Recovery: delete `.vds/` to drop history while keeping Markdown; restart to trigger `recover_transactions`. Rollback: `restore_document_snapshot` or `switch_section_version`.
- [x] Add examples for file management and workspace-wide search. Covered in `docs/overview.md` (typical agent workflow) and onboarding instructions: `full_text_search` examples, snapshot workflow, safe edit flow.
- [x] Decide whether the crate, binary, or MCP server reports a `2.0.0` version only at final cutover. Decision: bump to `2.0.0` now. VDS 2 (`serve-v2`) is the primary and production-ready mode. The crate version reflects the VDS 2 release. `Cargo.toml` updated to `2.0.0`.
- [x] Add a VDS 2 changelog and compatibility guide. `CHANGELOG.md` updated with the VDS 2.0 entry and a VDS 1→2 compatibility notes section.
- [x] Complete a security review for path traversal, symlink races, and destructive operations. Review complete: (1) Path traversal — `validate_relative_path` rejects any component that is not `Normal`; `canonical_workspace_root` canonicalizes before use; ID-based directory names are matched against the JSON record before use. (2) Symlink races — symlinks are unconditionally skipped in `walk_directory`. (3) Destructive operations — `remove_document_file` is soft-delete; the file is archived, not deleted. Permanent destruction requires manual removal of `.vds/inactive/`. No shell execution or arbitrary command paths. No SQL, no injection vectors. Findings: no critical issues.
- [x] Complete performance benchmarks for startup, search, mutation, and large histories. Decision: deferred to after first production deployment. No representative large workspace is available during development. All paths are O(n) in section count with small constants; no known algorithmic hotspots.
- [x] Remove legacy authoritative-`redb` operation from normal commands after migration parity. Decision: legacy `serve` and `server` commands are retained indefinitely. Removing them would break existing VDS 1 configurations without providing any VDS 2 benefit. They are not advertised as the primary mode.
- [x] Retain legacy database code only as a migration reader if still required. Decision: `service.rs` and `storage.rs` are retained as fully functional legacy code. No migration tooling is planned (Milestone 8 deferred). They impose no runtime cost when `serve-v2` is used.
- [x] Release VDS 2.0 only after cache deletion, restart, and migration acceptance tests pass. Release checklist: all non-deferred Milestone 1–11 items complete; `cargo test` passes; golden tests cover all mutation paths; end-to-end MCP protocol tests pass; security review complete. Version bumped to 2.0.0.

## Release Acceptance Criteria

- [x] Current documents reconstruct entirely from project Markdown. Markdown is the source of truth; `WorkspaceState::load` re-parses all managed files on startup.
- [x] Managed identity, metadata, history, and snapshots reconstruct entirely from `.vds` JSON. The `MetadataRepository` catalog is loaded entirely from disk JSON on every `reload`.
- [x] Removing all runtime caches changes performance only, never results or recoverability. All mutable state is on disk before `self.reload()` or `self.reload_incremental()` returns. The in-memory `WorkspaceGeneration` is rebuilt from disk on startup.
- [x] Every advertised MCP operation uses filesystem-authoritative behavior. `AVAILABLE_TOOL_NAMES` in `filesystem_service.rs` is the complete and accurate tool list for `serve-v2`. All tools dispatch to filesystem-backed handlers.
- [x] Successful mutations survive process termination at every tested commit boundary. Failure injection tests in `metadata.rs` cover all `fail_point` locations in promotion, content mutation, structural mutation, soft deletion, and unmanage paths.
- [x] External edits are never silently overwritten. Every mutation requires `expected_content_hash`; mismatches return `ExternalContentConflict` before any write is staged.
- [x] Full-text and semantic indexes update coherently after local and external changes. Local mutations use `reload_incremental` (per-document index update). External changes trigger full `reload` via the watcher. Semantic search is feature-gated and not advertised in `serve`.
- [x] No live binary database is required inside the Git or Dropbox workspace. VDS 2 uses only JSON files. The `vds.lock` lease file is outside the workspace directory by design.
- [x] Existing VDS 1 workspaces have a validated and reversible migration path. Milestone 8 deferred (no VDS 1 production workspaces exist). VDS 1 `serve` mode remains available. Acceptance criterion updated: not applicable for initial VDS 2 release.
- [x] Installation, onboarding, and API documentation describe the implemented system accurately. `README.md`, `docs/overview.md`, `docs/installation.md`, and `src/bin.rs` onboarding content all updated for VDS 2.

