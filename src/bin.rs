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
    /// Start the filesystem-authoritative VDS 2 server over stdio.
    ServeV2,
    /// Start the MCP server over streamable HTTP (legacy VDS 1 database mode).
    Server {
        #[arg(long, default_value = "127.0.0.1:8001")]
        bind: String,
        #[arg(long, default_value = "/mcp")]
        path: String,
    },
    /// Start the filesystem-authoritative VDS 2 server over streamable HTTP.
    ServerV2 {
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
    let workspace = cli.workspace.clone();
    let database = if let Some(workspace) = &cli.workspace {
        workspace.join(".vds").join("vds.db")
    } else {
        cli.database
    };

    match cli.command {
        Command::Serve => vds::service::serve_stdio(database).await?,
        Command::ServeV2 => {
            let workspace = workspace.unwrap_or(std::env::current_dir()?);
            vds::filesystem_service::serve_filesystem_stdio(workspace).await?
        }
        Command::Server { bind, path } => {
            vds::service::serve_streamable_http(database, bind, path).await?
        }
        Command::ServerV2 { bind, path } => {
            let workspace = workspace.unwrap_or(std::env::current_dir()?);
            vds::filesystem_service::serve_filesystem_http(workspace, bind, path).await?
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

**VDS 2.0** is production-ready and filesystem-authoritative: your Markdown files are the source of truth, and VDS
metadata (`.vds/`) stores stable IDs, version history, and snapshots as Git-friendly JSON files.

### Starting the Server

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

Use `serve-v2` (VDS 2.0 filesystem mode). `serve` is the legacy database-backed VDS 1 mode.

### Key Operations

#### Document Management
- `list_documents` — list all managed documents in the workspace
- `get_document` — get document metadata and root section ID
- `create_document` — create a new Markdown file and manage it
- `manage_document_file` — adopt an existing Markdown file into VDS tracking
- `get_document_location` — get the workspace-relative path of a document
- `rename_document` — rename the document (file move + metadata update)
- `import_document` — adopt an existing file by path
- `export_document` — render a document back to a Markdown file
- `remove_document_file` — soft-delete a document (archived, restorable)
- `restore_document_file` — restore a soft-deleted document
- `unmanage_document_file` — stop tracking a file without deleting it

#### Section Operations
- `get_section` — retrieve a section's title, content, level, and version
- `get_section_tree` — get a section and all its descendants
- `table_of_contents` — get a document's heading outline
- `create_section` — add a new section (child of a parent, or sibling)
- `update_section` — replace a section's content
- `append_to_section` — add content to the end of a section
- `rename_section` — change a section's heading title
- `insert_section_before` / `insert_section_after` — insert relative to a sibling (use `sibling_section_id`)
- `move_section` — relocate a section to a different parent
- `reorder_sections` — reorder children under a parent (use `parent_id` and `ordered_children`)
- `promote_section` / `demote_section` — change heading level
- `remove_section` — remove a section; set `remove_children: true` to remove descendants too
- `split_section` — split at a byte offset (`split_at` field, not `split_content`)
- `set_section_metadata` — update anchor, tags, summary, or lock state

#### Version History
- `section_versions` — list all version IDs for a section
- `get_section_version` — retrieve a historical version by version ID
- `diff_section_versions` — compare two versions
- `switch_section_version` — restore a section to a prior version

#### Snapshots
- `create_document_snapshot` — save the full document tree state with an optional label
- `document_snapshots` — list all saved snapshots
- `diff_document_snapshots` — compare two snapshots
- `restore_document_snapshot` — revert the document to a snapshot state

#### Search
- `full_text_search` — BM25 lexical search across all section titles and content
  - Supports `"quoted phrases"`, `prefix*` queries, AND/OR modes, and path filters
  - camelCase/PascalCase identifiers are tokenized into sub-words automatically
- `semantic_search_sections` — nearest-neighbor semantic search (requires `--features semantic-search` build)
  - **Requires pre-computed embeddings** — pass embedding vector via `query_embedding` parameter
  - VDS caches embeddings by (section_id, content_hash, model) but does not generate them
  - Uses HNSW index for fast approximate nearest neighbors
- `find_by_title` — exact or fuzzy title matching
- `find_by_tag` — search by section metadata tags

#### Workspace
- `get_workspace` — current workspace path, watcher status, and reload count
- `set_workspace` — switch to a different workspace at runtime
- `validate_document` — check content hash, version files, and snapshot references

### Usage Tips

1. **Conflict detection**: Every mutation accepts an optional `expected_content_hash`. Supply the hash returned
   by `get_document_location` or `get_section` to detect external edits before they are silently overwritten.

2. **Safe edits**: Read the section first, note its `current_version`, then call the mutation tool. If another
   agent or a human edited the file between read and write, VDS returns `ExternalContentConflict`.

3. **Structural vs. content mutations**: `update_section`, `append_to_section`, `rename_section`, and
   `set_section_metadata` are surgical (fast, byte-range). `create_section`, `remove_section`, `reorder_sections`,
   `move_section`, `promote_section`, `demote_section`, and `split_section` re-render the whole file canonically.

4. **remove_section**: The `remove_children` field is required. Pass `false` to detach children (they become
   siblings of the removed section) or `true` to delete the section and all descendants.

5. **reorder_sections**: Use `parent_id` (not `parent_section_id`) and `ordered_children` (not `ordered_section_ids`).

6. **split_section**: Use `split_at` (byte offset into the section content) to control the split point.
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
