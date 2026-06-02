//
// Copyright 2025-2026 Hans W. Uhlig. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

//! Wrapper for VDS, Handles CLAP

use std::error::Error;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use vds::document::DocumentId;
use vds::mcp::{
    ImportDocumentParams, ListDocumentsParams, RenderDocumentMarkdownParams, VdsMcpSurface,
};
use vds::service::VdsServer;

#[derive(Debug, Parser)]
#[command(name = "vds", about = "Versioned Document Service MCP server")]
struct Cli {
    #[arg(short, long, default_value = ".vds/vds.db", global = true)]
    database: PathBuf,

    #[arg(short, long, global = true)]
    workspace: Option<PathBuf>,

    #[arg(short, long, global = true)]
    output: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the MCP server over stdio.
    Serve,
    /// Start the MCP server over streamable HTTP.
    Server {
        #[arg(long, default_value = "127.0.0.1:8001")]
        bind: String,
        #[arg(long, default_value = "/mcp")]
        path: String,
    },
    /// List documents in storage.
    List,
    /// Import a Markdown document into storage.
    Import {
        /// Markdown file to import.
        path: PathBuf,
        /// Document name to store. Defaults to the input file stem.
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Export a Markdown document.
    Export {
        /// Document ID to export.
        document_id: String,
    },
    /// Create or append VDS-MCP usage instructions to AGENTS.md in the project root.
    Onboard,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    
    // Determine the database path: use workspace if provided, otherwise use database flag
    let database = if let Some(workspace) = cli.workspace {
        workspace.join(".vds").join("vds.db")
    } else {
        cli.database
    };
    
    match cli.command {
        Command::Serve => vds::service::serve_stdio(database).await?,
        Command::Server { bind, path } => {
            vds::service::serve_streamable_http(database, bind, path).await?
        }
        Command::List => {
            let server = VdsServer::open(database)?;
            let documents = mcp(server.list_documents(ListDocumentsParams::default()))?;
            write_text(cli.output, serde_json::to_string_pretty(&documents)?)?;
        }
        Command::Import { path, name } => {
            let server = VdsServer::open(database)?;
            let name = name.unwrap_or_else(|| document_name_from_path(&path));
            let document = mcp(server.import_document(ImportDocumentParams {
                name,
                path: path.to_string_lossy().into_owned(),
            }))?;
            write_text(cli.output, serde_json::to_string_pretty(&document)?)?;
        }
        Command::Export { document_id } => {
            let server = VdsServer::open(database)?;
            let markdown = mcp(
                server.render_document_markdown(RenderDocumentMarkdownParams {
                    document_id: DocumentId::new(document_id),
                }),
            )?;
            write_text(cli.output, markdown)?;
        }
        Command::Onboard => {
            onboard_agent()?;
        }
    }
    Ok(())
}

fn mcp<T>(result: vds::mcp::McpResult<T>) -> Result<T, Box<dyn Error>> {
    result.map_err(|error| {
        std::io::Error::other(format!("{:?}: {}", error.code, error.message)).into()
    })
}

fn write_text(output: Option<PathBuf>, text: String) -> Result<(), Box<dyn Error>> {
    if let Some(path) = output {
        std::fs::write(path, text)?;
    } else {
        println!("{text}");
    }
    Ok(())
}

fn document_name_from_path(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("document")
        .to_owned()
}

fn onboard_agent() -> Result<(), Box<dyn Error>> {
    let agents_file = PathBuf::from("AGENTS.md");

    let vds_instructions = r#"## VDS-MCP (Versioned Document Service)

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

#### Workspace & Database Management
- `set_workspace`: Set the workspace directory (database will be at `<workspace>/.vds/vds.db`)
- `get_workspace`: Get the current workspace directory and database path
- `set_database`: Set an explicit database file path
- `get_database`: Get the current database file path

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

By default, VDS stores data in `.vds/vds.db` relative to the current working directory. You can override this in several ways:

1. **CLI Flag**: Use `--database <path>` to specify an explicit database file path
2. **Workspace Flag**: Use `--workspace <path>` to use `<workspace>/.vds/vds.db`
3. **MCP Tools**: Use `set_workspace` or `set_database` tools to change the database location at runtime

**Important**: When Claude Desktop starts the MCP server, it runs in Claude's data directory, not your project directory. Use the `set_workspace` tool to point VDS to your project:

```javascript
// In your MCP client or at runtime:
set_workspace({ workspace: "/path/to/your/project" })
```

This will reopen the database at `/path/to/your/project/.vds/vds.db`, allowing you to work with project-specific documents.
"#;

    let content = if agents_file.exists() {
        // Append to existing file
        let existing = std::fs::read_to_string(&agents_file)?;
        if existing.contains("VDS-MCP") || existing.contains("Versioned Document Service") {
            println!("AGENTS.md already contains VDS-MCP instructions. Skipping.");
            return Ok(());
        }
        format!("{}\n\n{}", existing.trim_end(), vds_instructions)
    } else {
        // Create new file with header
        format!("# Agent Instructions\n\n{}", vds_instructions)
    };

    std::fs::write(&agents_file, content)?;
    println!("VDS-MCP usage instructions added to AGENTS.md");
    Ok(())
}
