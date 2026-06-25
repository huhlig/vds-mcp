# Installing and Configuring VDS

VDS is distributed as the `vds-mcp` crate. Installation produces a binary named `vds-mcp` that can be launched by MCP clients over stdio or run as a streamable HTTP MCP server.

## Prerequisites

- Rust and Cargo (stable toolchain).
- A project directory where your Markdown files live (the VDS 2 workspace).

VDS 2 does not require a database file. It creates a `.vds/` directory inside the workspace on first use and stores all metadata there as plain JSON.

## Install With Cargo

From a local checkout:

```powershell
cargo install --locked --path .
```

From a Git repository:

```powershell
cargo install --locked --git <repo-url> vds-mcp
```

After publication to crates.io:

```powershell
cargo install --locked vds-mcp
```

Confirm the binary is available:

```powershell
vds-mcp --help
```

Cargo installs binaries to `%USERPROFILE%\.cargo\bin` on Windows and `~/.cargo/bin` on Unix. Make sure that directory is on `PATH` for any MCP client that needs to launch `vds-mcp`.

## Run the VDS 2 Server

**Stdio (recommended for MCP clients):**

```powershell
vds-mcp --workspace E:\Projects\myproject serve-v2
```

**HTTP:**

```powershell
vds-mcp --workspace E:\Projects\myproject server-v2 --bind 127.0.0.1:8001 --path /mcp
```

When `serve-v2` starts it announces itself on stderr (service name, transport, and tool list). Stdout is reserved for MCP messages. On first run it initializes `.vds/workspace.json` and the documents directory inside the workspace.

## MCP Client Configuration

### Claude Desktop / Cursor / Windsurf (JSON)

```json
{
  "mcpServers": {
    "vds": {
      "command": "vds-mcp",
      "args": ["--workspace", "E:\\Projects\\myproject", "serve-v2"]
    }
  }
}
```

Use an absolute workspace path. If `vds-mcp` is not on the MCP client's `PATH`, provide the full binary path:

```json
{
  "mcpServers": {
    "vds": {
      "command": "C:\\Users\\you\\.cargo\\bin\\vds-mcp.exe",
      "args": ["--workspace", "E:\\Projects\\myproject", "serve-v2"]
    }
  }
}
```

### Codex TOML

```toml
[mcp_servers.vds]
command = "vds-mcp"
args = ["--workspace", "/absolute/path/to/project", "serve-v2"]
```

### HTTP Clients

Start the server separately and point the client at the configured URL:

```powershell
vds-mcp --workspace E:\Projects\myproject server-v2 --bind 127.0.0.1:8001 --path /mcp
```

```text
http://127.0.0.1:8001/mcp
```

## Workspace Initialization

VDS initializes the workspace on first use:

```
myproject/
└── .vds/
    └── workspace.json   ← created automatically
```

Existing Markdown files in the workspace are discovered immediately (subject to `.vdsignore`). They are not tracked by VDS until `manage_document_file` is called or a document is created through VDS tools.

Add `.vds/` to `.gitignore` if you do not want to commit history — but committing `.vds/` is safe and useful for persistence across machines. The only file that should not be committed is the lock file (`vds.lock`), which is outside `.vds/`.

## .vdsignore

Create `.vdsignore` in the workspace root to exclude Markdown files from discovery:

```
# Exclude all files in the generated/ directory
generated/

# Re-include one specific file within that directory
!generated/operator-guide.md

# Exclude a specific file
CONTRIBUTING.md
```

Patterns follow a subset of Gitignore syntax:
- `pattern` — matches any file or directory component with that name
- `dir/` — matches a directory and all its contents (trailing slash required)
- `path/to/file.md` — matches a specific path from the workspace root (slash required to anchor)
- `!pattern` — negates a previous match, re-including the path
- `*`, `**`, `?` — standard glob wildcards

`.gitignore` is not read automatically. Duplicate relevant lines into `.vdsignore` if needed.

## Runtime Workspace Switching

Use the `set_workspace` MCP tool to switch to a different workspace without restarting the server:

```json
{ "tool": "set_workspace", "arguments": { "workspace": "/path/to/other/project" } }
```

This flushes the current state, acquires a new workspace lease, rebuilds the in-memory index, and starts the filesystem watcher for the new location.

## Updating

Reinstall from the current checkout:

```powershell
cargo install --locked --path . --force
```

Reinstall from Git:

```powershell
cargo install --locked --git <repo-url> vds-mcp --force
```

VDS 2 metadata format is versioned. The server will report `UnsupportedFormat` if an old `.vds/` directory uses a format it does not recognize. No automatic migration is performed; contact the project maintainer for migration guidance.
