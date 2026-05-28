# Versioned Document Service

Versioned Document Service (VDS) is a Rust MCP service for managing long-form Markdown documents as versioned section
trees. It is designed for agents and tools that need to edit, inspect, search, and export large documents without
rewriting the whole file each time.

Markdown is the import and export format. Internally, VDS stores documents as stable, addressable sections with document
IDs, section IDs, version IDs, metadata, and parent/child relationships.

## Features

- Import Markdown into a persistent section tree.
- Export stored documents back to Markdown.
- Serve MCP tools over stdio or streamable HTTP.
- List, read, rename, and render documents.
- Read sections, section trees, and tables of contents.
- Search section titles and content.
- Store section versions and document snapshots.
- Check optimistic concurrency conflicts against section versions.

Several editing and history tools are defined in the MCP surface but are still being implemented in the storage-backed
service.

## Installation

Install from crates.io with:

```powershell
cargo install --locked vds-mcp
```

Install from this repository checkout:

```powershell
cargo install --locked --path .
```

Install from a Git repository:

```powershell
cargo install --locked --git <repo-url> vds-mcp
```

Verify the installed binary:

```powershell
vds-mcp --help
```

Onboard your project with:

```powershell
cd /path/to/project
vds-mcp onboard
```

## Project Structure

```
vds-mcp/
├── benches/               # Service Benchmakrs 
│   └── benchmark.rs
├── docs/                  # Project Documentation
│   ├── installation.md    # Command Line Wrapper
│   └── overview.md
├── src/
│   ├── bin.rs             # Command Line Wrapper
│   ├── document.rs        # Core Document Model
│   ├── lib.rs             # Main Library File
│   ├── markdown.rs        # Markdown Utilities
│   ├── mcp.rs             # MCP Facade
│   ├── service.rs         # MCP Service
│   └── storage.rs         # Storage Backend
├── tests/
│   ├── mcp_smoke.rs       # MCP Smoke Tests
│   └── overview.rs        # Integration Tests
├── AGENTS.md              # VDS Agent Usage Instructions
├── Cargo.toml             # Workspace configuration
├── CONTRIBUTING.md        # This file
├── LICENSE.md             # Apache2 License
└── README.md              # Project Readme.
```

## Usage

Build locally without installing:

```powershell
cargo build
```

Run the stdio MCP server:

```powershell
cargo run -- serve
```

In `serve` mode, VDS is meant to be launched by an MCP client. It writes a startup banner to stderr that advertises the
service name, transport, `tools/list` and `tools/call` capabilities, usage guidance, and every available MCP tool. Stdout
is reserved for MCP protocol messages.

Run the streamable HTTP MCP server:

```powershell
cargo run -- server --bind 127.0.0.1:8001 --path /mcp
```

Import a Markdown document:

```powershell
cargo run -- import docs/context1.md
```

List stored documents:

```powershell
cargo run -- list
```

Export a document:

```powershell
cargo run -- export <document_id> --output exported.md
```

By default, VDS stores data in `.vds/vds.db`. Use `--database <path>` to choose another database file.

## MCP Agent Configuration

For stdio-based MCP clients, add VDS as an MCP server with the installed `vds-mcp` binary. Use an absolute database path so
the agent stores documents in a predictable location (it will store in `./.vds/vds.db` by default):

Simple Use:
```json
{
  "mcpServers": {
    "vds-mcp": {
      "command": "vds-mcp",
      "args": [
        "serve"
      ]
    }
  }
}
```


With Database: 
```json
{
  "mcpServers": {
    "vds-mcp": {
      "command": "vds-mcp",
      "args": [
        "--database",
        "E:\\Projects\\vds-mcp\\.vds\\vds.db",
        "serve"
      ]
    }
  }
}
```

For Codex-style TOML MCP configuration:

```toml
[mcp_servers.vds-mcp]
command = "vds-mcp"
args = ["--database", "E:\\Projects\\vds-mcp\\.vds\\vds.db", "serve"]
```

For HTTP-capable MCP clients, run VDS separately:

```powershell
vds-mcp --database E:\Dropbox\Projects\IBM\vds\.vds\vds.db server --bind 127.0.0.1:8001 --path /mcp
```

Then point the client at:

```text
http://127.0.0.1:8001/mcp
```

Both server modes advertise the same tool capability set during MCP initialization. Clients can call `tools/list` to get
the full catalog and `tools/call` to invoke a tool.

See [docs/installation.md](docs/installation.md) for more installation and MCP client examples.

## Documentation

See [docs/overview.md](docs/overview.md) for a deeper explanation of the project purpose, current behavior, data flow,
and architecture.
