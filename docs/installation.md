# Installing and Configuring VDS

VDS is distributed as the `vds-mcp` crate, which installs a Rust binary named `vds-mcp`. Once installed, it can be
launched directly by MCP clients over stdio or run as a streamable HTTP MCP server.

## Prerequisites

- Rust and Cargo installed.
- A local directory where VDS can create its database file.

The default database path is `.vds/vds.db`, relative to the process working directory. For MCP clients, prefer an
absolute `--database` path so data does not move when the client launches VDS from a different directory.

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

Confirm that Cargo installed the binary:

```powershell
vds-mcp --help
```

Cargo installs binaries into Cargo's bin directory, commonly `%USERPROFILE%\.cargo\bin` on Windows. Make sure that
directory is on `PATH` for any MCP client that needs to launch `vds-mcp`.

## Run VDS Manually

Start the stdio MCP server:

```powershell
vds-mcp --database E:\Dropbox\Projects\IBM\vds\.vds\vds.db serve
```

When `serve` starts, it announces itself on stderr with the service name, transport, capabilities, usage guidance, and
advertised tool list. Stdout remains reserved for MCP protocol messages, so stdio clients can safely launch it.

Start the streamable HTTP MCP server:

```powershell
vds-mcp --database E:\Dropbox\Projects\IBM\vds\.vds\vds.db server --bind 127.0.0.1:8001 --path /mcp
```

Use CLI document commands:

```powershell
vds-mcp --database E:\Dropbox\Projects\IBM\vds\.vds\vds.db import docs\overview.md
vds-mcp --database E:\Dropbox\Projects\IBM\vds\.vds\vds.db list
vds-mcp --database E:\Dropbox\Projects\IBM\vds\.vds\vds.db export <document_id> --output exported.md
```

## Add VDS to an MCP Agent

Most stdio MCP clients use a server entry with a command and argument list. Add an entry named `vds` that runs the
installed binary:

```json
{
  "mcpServers": {
    "vds": {
      "command": "vds-mcp",
      "args": [
        "--database",
        "E:\\Projects\\vds\\.vds\\vds.db",
        "serve"
      ]
    }
  }
}
```

Use the same shape for clients such as Claude Desktop, Cursor, Windsurf, or other agents that accept MCP server JSON.
Place the JSON in that client's MCP configuration file.

For Codex-style TOML configuration:

```toml
[mcp_servers.vds]
command = "vds"
args = ["--database", "E:\\Projects\\vds\\.vds\\vds.db", "serve"]
```

If the MCP client requires an absolute command path, use the full path to the installed binary:

```toml
[mcp_servers.vds]
command = "C:\\Users\\<you>\\.cargo\\bin\\vds.exe"
args = ["--database", "E:\\Projects\\vds\\.vds\\vds.db", "serve"]
```

## HTTP MCP Configuration

Some MCP clients connect to an HTTP endpoint instead of launching a stdio process. Start VDS yourself:

```powershell
vds-mcp --database E:\Projects\vds\.vds\vds.db server --bind 127.0.0.1:8001 --path /mcp
```

Then configure the client URL as:

```text
http://127.0.0.1:8001/mcp
```

After initialization, clients discover VDS through the standard MCP `tools/list` capability and invoke operations with
`tools/call`.

Use stdio unless the client specifically supports streamable HTTP MCP servers.

## Updating

Reinstall from the current checkout:

```powershell
cargo install --locked --path . --force
```

Reinstall from Git:

```powershell
cargo install --locked --git <repo-url> vds-mcp --force
```
