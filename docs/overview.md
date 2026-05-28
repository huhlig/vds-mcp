# Versioned Document Service Overview

Versioned Document Service (VDS) is an MCP-oriented document service for long-form Markdown documents. Its purpose is to
let agents and tools work with large documents through stable, structured operations instead of repeatedly rewriting
whole files as raw text.

Large Markdown files are easy to corrupt when an agent has to regenerate or patch them from context. VDS addresses that
by giving every document and section a durable identity, storing the document as a tree, and exposing read, search,
version, import, and export operations through a service boundary.

## What It Does

VDS imports Markdown into a persistent internal model, stores that model in a local `redb` database, and renders
documents or sections back to Markdown when needed.

The current implementation supports:

- Starting an MCP server over stdio.
- Starting an MCP server over streamable HTTP.
- Creating and importing Markdown documents.
- Listing, reading, and renaming documents.
- Building a table of contents from the stored section tree.
- Reading individual sections or section subtrees.
- Rendering one section or a whole document back to Markdown.
- Exporting a stored document to a Markdown file.
- Creating, inserting, updating, patching, moving, reordering, promoting, demoting, splitting, and removing sections.
- Switching and diffing section versions.
- Creating, restoring, and diffing document snapshots.
- Deleting documents and their associated stored records.
- Listing section versions and document snapshots that exist in storage.
- Searching section titles and content with simple text matching.
- Finding sections by title or tag.
- Listing recent section-version and snapshot changes.
- Cooperative section locking and unlocking.
- Checking optimistic concurrency conflicts against a section version.
- Basic validation, normalization, and repair endpoints that currently return no changes.

## How It Works

Markdown is treated as a boundary format, not as the primary storage model. On import, VDS parses Markdown headings with
`pulldown-cmark` and turns the document into a tree of sections:

```text
Document
  root Section
    Section
      Section
    Section
```

Each document has a stable `DocumentId`. Each section has a stable `SectionId` that survives title changes, moves, and
content edits. Each section also points at a current `VersionId`, allowing callers to detect stale edits and eventually
restore or diff historical versions.

The importer creates a synthetic root section for each document. Content before the first Markdown heading is stored on
that root. Normal headings become child sections whose parentage is inferred from heading level, so an `h3` becomes a
descendant of the nearest preceding lower-level heading. Headings inside fenced code blocks are ignored because parsing
is delegated to `pulldown-cmark`.

The exporter walks the stored section tree in sibling ordinal order and renders the structure back to Markdown.
Whole-document export omits the synthetic root heading, while section rendering includes the requested section heading
and can optionally include descendants.

## Basic Architecture

The project is a Rust crate with one library and one CLI binary.

```text
src/
  bin.rs        CLI entry point and command dispatch
  lib.rs        Public module wiring and high-level crate documentation
  document.rs   Core document, section, version, snapshot, patch, and metadata types
  markdown.rs   Markdown import, parsing, section-tree construction, and export
  storage.rs    redb-backed persistence facade and secondary indexes
  mcp.rs        Transport-neutral MCP request/response types and tool documentation
  service.rs    Runtime MCP server adapter and storage-backed command handlers
```

### CLI Layer

`src/bin.rs` defines the `vds` command using `clap`. It supports server startup and basic document operations:

```text
vds serve
vds server --bind 127.0.0.1:8001 --path /mcp
vds list
vds import <path>
vds export <document_id>
```

The CLI defaults to `.vds/vds.db` for local storage. Text output goes to stdout unless `--output` is provided.

### MCP Surface

`src/mcp.rs` defines the transport-neutral API. The central trait is `VdsMcpSurface`, which describes all document
lifecycle, navigation, editing, versioning, search, validation, repair, locking, and conflict-check commands.

This module also defines the request and response structs used by those commands. Keeping these types separate from the
runtime server lets the project document and evolve the command contract independently from a specific transport.

### Service Adapter

`src/service.rs` implements `VdsMcpSurface` for `VdsServer`. It adapts incoming MCP tool calls to typed command
handlers, serializes structured results, maps service errors to MCP errors, and exposes tools through the `rmcp` server
interface.

The same `VdsServer` can be served over:

- stdio, for local MCP clients that launch the process directly.
- streamable HTTP, for clients that connect to an HTTP endpoint.

### Storage Layer

`src/storage.rs` wraps `redb` behind `DocumentStore`. Records are stored as JSON payloads in redb tables, with secondary
index tables for query patterns the service needs:

- document lookup and listing
- document-name lookup
- all sections in a document
- parent-to-child traversal in ordinal order
- section-version history
- document-snapshot history

Most write paths use transactions so related records and indexes are updated together.

### Markdown Boundary

`src/markdown.rs` owns import and export. It converts Markdown into the internal tree model and converts stored trees
back to Markdown strings or files.

This is the key architectural choice in VDS: Markdown remains the format humans read and write at the edges, but agents
operate on stable document and section IDs inside the service.

## Data Flow

Typical import and export flow:

```text
Markdown file
  -> pulldown-cmark parser
  -> Document + Section tree + initial SectionVersion records
  -> DocumentStore / redb
  -> MCP tools or CLI commands
  -> rendered Markdown string or file
```

Typical agent read flow:

```text
list_documents
  -> table_of_contents
  -> get_section or get_section_tree
  -> render_section_markdown when human-readable Markdown is needed
```

Typical safe edit flow, as the full editing surface is implemented:

```text
get_section
  -> inspect current_version
  -> update or patch with expected_version
  -> store a new SectionVersion
  -> use check_conflicts when stale context is possible
```

## Design Goals

- Stable addressability: callers edit sections by ID, not by fragile heading text or byte offsets in a whole file.
- Incremental context: agents can request just the table of contents, one section, or a subtree.
- Version awareness: section versions and document snapshots provide a foundation for history, rollback, and conflict
  detection.
- Markdown compatibility: documents can still enter and leave the system as regular Markdown.
- Transport flexibility: the same service can run through stdio or streamable HTTP MCP transports.

## Current Status

VDS currently has the foundation in place: data types, MCP command definitions, local persistence, Markdown
import/export, basic search, basic conflict checks, and server transports.

The next major implementation work is the mutation and history surface: creating, updating, patching, moving, deleting,
diffing, snapshotting, restoring, and locking sections with full version persistence.
