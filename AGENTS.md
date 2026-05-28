# Agent Instructions

## VDS-MCP (Versioned Document Service)

VDS-MCP is a Model Context Protocol (MCP) server for managing Markdown documents as stable, versioned section trees.

### Key Features

- **Hierarchical Document Structure**: Documents are organized as trees of sections with automatic heading level management
- **Version Control**: Every section edit creates a new version with optional author and change summary metadata
- **Snapshots**: Create named snapshots of entire document states for rollback or comparison
- **Search**: Full-text search across section titles and content, with optional semantic search using embeddings
- **Conflict Detection**: Optimistic concurrency control prevents conflicting edits

### Common MCP Operations

#### Document Management
- `list_documents`: List all documents in storage
- `create_document`: Create a new document with optional initial content
- `import_document`: Import a Markdown file from disk
- `export_document`: Export a document to disk as Markdown
- `get_document`: Get document metadata
- `delete_document`: Delete a document and all its sections
- `rename_document`: Change a document's name

#### Section Operations
- `get_section`: Retrieve a specific section
- `get_section_tree`: Get a section and its descendants
- `create_section`: Add a new section to a document
- `update_section`: Replace section content
- `patch_section`: Apply structured edits (append, rename, set metadata). Parameters require a top-level `patch` object containing `operations`.
- `append_to_section`: Add content to the end of a section
- `rename_section`: Change a section's title
- `insert_section_before`: Insert a section before an existing sibling using `sibling_section_id`
- `insert_section_after`: Insert a section after an existing sibling using `sibling_section_id`
- `move_section`: Relocate a section in the document tree
- `remove_section`: Delete a section (optionally with children)
- `promote_section`: Decrease heading level (move up in hierarchy)
- `demote_section`: Increase heading level (move down in hierarchy)

#### Versioning
- `section_versions`: List all versions of a section
- `get_section_version`: Retrieve a historical version
- `switch_section_version`: Revert to a previous version
- `diff_section_versions`: Compare two versions

#### Snapshots
- `create_document_snapshot`: Save current document state
- `document_snapshots`: List all snapshots
- `restore_document_snapshot`: Revert to a snapshot
- `diff_document_snapshots`: Compare two snapshots

#### Search & Discovery
- `search_sections`: Full-text search across sections
- `semantic_search_sections`: Vector similarity search (requires semantic-search feature)
- `find_by_title`: Find sections by title
- `find_by_tag`: Find sections with specific metadata tags
- `table_of_contents`: Generate document outline

#### Maintenance
- `validate_document`: Check document integrity
- `normalize_document`: Fix structural issues
- `repair_document`: Attempt to fix corrupted documents
- `lock_section`: Prevent edits to a section
- `unlock_section`: Allow edits to a locked section
- `check_conflicts`: Verify version expectations

### Usage Tips

1. **Options are Optional**: Most operations accept optional `EditOptions` with fields for `expected_version`, `author`, and `change_summary`. When omitted, sensible defaults are used.

2. **Search Options**: Search operations accept optional configuration:
   - `SearchOptions`: Control content/title search, fuzzy matching, and result limits
   - `SemanticSearchOptions`: Configure HNSW parameters for vector search
   - `NormalizeOptions`: Control document normalization behavior

3. **Hierarchical Structure**: Sections maintain parent-child relationships. Moving or removing sections affects their descendants.

4. **Version Safety**: Use `expected_version` in `EditOptions` for optimistic concurrency control to prevent conflicting edits.

5. **Patch Shape**: Call `patch_section` with `patch.operations`, not a top-level `operations` field.

6. **Sibling Field Names**: Call `insert_section_before` and `insert_section_after` with `sibling_section_id`, not `sibling_id`.

### Example User Workflow

```bash
# Create Agent Onboarding Instructions
vds-mcp onboard

# Import existing markdown
vds-mcp import README.md --name "readme"

# List all documents
vds-mcp list

# Export a document
vds-mcp export <document-id>
```

### MCP Server Modes

- **stdio**: `vds-mcp serve` - For local MCP clients
- **HTTP**: `vds-mcp server --bind 127.0.0.1:8001 --path /mcp` - For remote access

### Database Location

By default, VDS stores data in `.vds/vds.db`. Override with `--database` flag.
