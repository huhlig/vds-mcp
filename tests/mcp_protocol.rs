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

//! End-to-end MCP protocol tests for the VDS 2 filesystem server.
//!
//! These tests exercise the JSON dispatch layer — `FilesystemVdsServer::call()` —
//! which is the central routing method that `call_tool` delegates to after
//! unwrapping the rmcp transport envelope. Testing this layer covers:
//!
//!   * Tool name routing
//!   * JSON argument deserialization
//!   * JSON result serialization
//!   * Error shape returned for invalid input or unknown tools
//!
//! This is deliberately separate from the direct-trait-call tests in `mcp_smoke.rs`
//! which call `VdsMcpSurface` methods with typed parameters.

use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use rmcp::model::JsonObject;
use serde_json::{Value, json};
use vds::filesystem_service::FilesystemVdsServer;

// ── helpers ───────────────────────────────────────────────────────────────────

fn scratch_workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("vds-mcp-protocol-tests")
        .join(format!("{name}-{}", Uuid::now_v7()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_markdown(workspace: &PathBuf, relative: &str, content: &str) {
    let path = workspace.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn open_server(workspace: &PathBuf) -> FilesystemVdsServer {
    FilesystemVdsServer::open(
        workspace,
        #[cfg(all(
            feature = "semantic-search",
            any(target_os = "linux", target_os = "macos")
        ))]
        None,
    )
    .expect("open server")
}

/// Converts a serde_json object literal into the JsonObject type used by the call() dispatcher.
fn args(value: serde_json::Value) -> Option<JsonObject> {
    match value {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Calls a tool by name with JSON arguments and asserts success, returning the parsed JSON result.
///
/// The VDS dispatcher wraps array results in `{"items": [...]}` and raw-string results in
/// `{"content": "..."}` (single-key object, value is a string) before serializing them into
/// the MCP response. This helper unwraps those envelopes so callers can assert on the logical
/// result directly, without accidentally stripping legitimate domain `content` fields on objects
/// like `Section` that also have a `content` key.
fn call_ok(server: &FilesystemVdsServer, tool: &str, arguments: serde_json::Value) -> Value {
    let raw = server
        .call(tool, args(arguments))
        .unwrap_or_else(|e| panic!("tool {tool:?} returned error: {:?}: {}", e.code, e.message));
    match &raw {
        // Array envelope: {"items": [...]}
        Value::Object(map) if map.len() == 1 => {
            if let Some(items) = map.get("items").filter(|v| v.is_array()) {
                return items.clone();
            }
            // String envelope: {"content": "..."} — only when the sole value is a string.
            if let Some(content) = map.get("content").filter(|v| v.is_string()) {
                return content.clone();
            }
            raw
        }
        _ => raw,
    }
}

/// Calls a tool by name and asserts it returns an MCP error.
fn call_err(
    server: &FilesystemVdsServer,
    tool: &str,
    arguments: serde_json::Value,
) -> vds::mcp::McpError {
    server
        .call(tool, args(arguments))
        .expect_err(&format!("tool {tool:?} should have returned an error"))
}

// ── tool dispatch: workspace / storage info ────────────────────────────────────

#[test]
fn dispatch_get_workspace_returns_filesystem_backend() {
    let workspace = scratch_workspace("gw-dispatch");
    let server = open_server(&workspace);

    let result = call_ok(&server, "get_workspace", json!({}));
    assert_eq!(
        result["database"], "filesystem",
        "database field should be 'filesystem'"
    );
    assert!(
        result["workspace"].is_string(),
        "workspace should be a string"
    );
    assert!(
        result["watcher_active"].is_boolean(),
        "watcher_active should be a bool"
    );
    assert!(
        result["reload_count"].is_number(),
        "reload_count should be a number"
    );
}

#[test]
fn dispatch_get_database_returns_filesystem() {
    let workspace = scratch_workspace("gdb-dispatch");
    let server = open_server(&workspace);

    let result = call_ok(&server, "get_database", json!({}));
    assert_eq!(result["database"], "filesystem");
}

// ── tool dispatch: document discovery ────────────────────────────────────────

#[test]
fn dispatch_list_documents_discovers_markdown_files() {
    let workspace = scratch_workspace("list-dispatch");
    write_markdown(&workspace, "alpha.md", "# Alpha\n\nFirst.\n");
    write_markdown(&workspace, "beta.md", "# Beta\n\nSecond.\n");
    let server = open_server(&workspace);

    let result = call_ok(&server, "list_documents", json!({}));
    let docs = result.as_array().expect("should be an array");
    assert_eq!(docs.len(), 2, "should discover exactly 2 documents");
    let names: Vec<&str> = docs
        .iter()
        .map(|d| d["name"].as_str().unwrap_or(""))
        .collect();
    assert!(
        names.contains(&"alpha") || names.contains(&"Alpha"),
        "alpha not found: {names:?}"
    );
}

#[test]
fn dispatch_list_documents_empty_workspace_returns_empty_array() {
    let workspace = scratch_workspace("empty-dispatch");
    let server = open_server(&workspace);

    let result = call_ok(&server, "list_documents", json!({}));
    let docs = result.as_array().expect("should be an array");
    assert!(docs.is_empty(), "empty workspace should return []");
}

// ── tool dispatch: table of contents ──────────────────────────────────────────

#[test]
fn dispatch_table_of_contents_returns_section_hierarchy() {
    let workspace = scratch_workspace("toc-dispatch");
    // Use two H1 headings so both appear as direct children of the virtual root,
    // making them directly visible in the table of contents without nesting.
    write_markdown(
        &workspace,
        "guide.md",
        "# Chapter 1\n\nContent.\n\n# Chapter 2\n\nMore.\n",
    );
    let server = open_server(&workspace);

    let docs = call_ok(&server, "list_documents", json!({}));
    let doc_id = docs[0]["id"].as_str().expect("doc id");

    let toc = call_ok(
        &server,
        "table_of_contents",
        json!({ "document_id": doc_id }),
    );
    let entries = toc.as_array().expect("toc should be array");
    assert_eq!(entries.len(), 2, "should have 2 top-level entries");
    let titles: Vec<&str> = entries
        .iter()
        .map(|e| e["title"].as_str().unwrap_or(""))
        .collect();
    assert!(titles.contains(&"Chapter 1") && titles.contains(&"Chapter 2"));
}

// ── tool dispatch: full-text search ───────────────────────────────────────────

#[test]
fn dispatch_full_text_search_finds_matching_content() {
    let workspace = scratch_workspace("fts-dispatch");
    write_markdown(
        &workspace,
        "doc.md",
        "# Doc\n\n## Installation\n\nRun cargo install vds-mcp.\n\n## Usage\n\nRun the binary.\n",
    );
    let server = open_server(&workspace);

    let results = call_ok(
        &server,
        "full_text_search",
        json!({ "query": "cargo", "require_all_terms": true }),
    );
    let hits = results.as_array().expect("results should be array");
    assert!(
        !hits.is_empty(),
        "search for 'cargo' should find at least one hit"
    );
    assert!(
        hits[0]["section"].is_object(),
        "each hit should have a section object"
    );
}

#[test]
fn dispatch_full_text_search_no_results_returns_empty_array() {
    let workspace = scratch_workspace("fts-empty-dispatch");
    write_markdown(&workspace, "doc.md", "# Doc\n\nHello world.\n");
    let server = open_server(&workspace);

    let results = call_ok(
        &server,
        "full_text_search",
        json!({ "query": "xyzzyzyqfoo9999" }),
    );
    assert_eq!(results.as_array().unwrap().len(), 0);
}

// ── tool dispatch: get_section ────────────────────────────────────────────────

#[test]
fn dispatch_get_section_returns_section_fields() {
    let workspace = scratch_workspace("get-section-dispatch");
    // Single H1 heading so it is directly accessible as toc[0].
    write_markdown(
        &workspace,
        "notes.md",
        "# Summary\n\nThe quick brown fox.\n",
    );
    let server = open_server(&workspace);

    let docs = call_ok(&server, "list_documents", json!({}));
    let doc_id = docs[0]["id"].as_str().unwrap();
    let toc = call_ok(
        &server,
        "table_of_contents",
        json!({ "document_id": doc_id }),
    );
    let section_id = toc[0]["section_id"].as_str().expect("section_id");

    let section = call_ok(
        &server,
        "get_section",
        json!({ "document_id": doc_id, "section_id": section_id }),
    );
    assert_eq!(section["title"], "Summary");
    assert!(
        section["content"]
            .as_str()
            .unwrap()
            .contains("quick brown fox")
    );
}

// ── tool dispatch: error cases ────────────────────────────────────────────────

#[test]
fn dispatch_unknown_tool_returns_error() {
    let workspace = scratch_workspace("unknown-tool-dispatch");
    let server = open_server(&workspace);

    let error = call_err(&server, "no_such_tool_xyz", json!({}));
    assert!(
        matches!(error.code, vds::mcp::McpErrorCode::InvalidInput),
        "expected InvalidInput, got {:?}",
        error.code
    );
}

#[test]
fn dispatch_get_document_with_bad_id_returns_not_found() {
    let workspace = scratch_workspace("not-found-dispatch");
    write_markdown(&workspace, "doc.md", "# Doc\n\nContent.\n");
    let server = open_server(&workspace);

    let error = call_err(
        &server,
        "get_document",
        json!({ "document_id": "doc-000000000000000000000000" }),
    );
    assert!(
        matches!(error.code, vds::mcp::McpErrorCode::NotFound),
        "expected NotFound, got {:?}: {}",
        error.code,
        error.message
    );
}

#[test]
fn dispatch_table_of_contents_with_missing_field_returns_error() {
    let workspace = scratch_workspace("missing-field-dispatch");
    let server = open_server(&workspace);

    // document_id is required but omitted.
    let error = call_err(&server, "table_of_contents", json!({}));
    assert!(
        matches!(error.code, vds::mcp::McpErrorCode::InvalidInput),
        "expected InvalidInput for missing required field, got {:?}",
        error.code
    );
}

// ── tool dispatch: mutation pipeline via JSON ──────────────────────────────────

#[test]
fn dispatch_manage_and_update_section_via_json() {
    let workspace = scratch_workspace("mutation-dispatch");
    // Use a single H1 so "Draft" is directly accessible as toc[0].
    write_markdown(&workspace, "notes.md", "# Draft\n\nInitial content.\n");
    let server = open_server(&workspace);

    // 1. list to find doc ID
    let docs = call_ok(&server, "list_documents", json!({}));
    let doc_id = docs[0]["id"].as_str().unwrap().to_owned();

    // 2. manage_document_file
    call_ok(
        &server,
        "manage_document_file",
        json!({ "document_id": doc_id }),
    );

    // 3. find the Draft section via table_of_contents
    let toc = call_ok(
        &server,
        "table_of_contents",
        json!({ "document_id": doc_id }),
    );
    let draft_id = toc
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["title"] == "Draft")
        .and_then(|e| e["section_id"].as_str())
        .expect("Draft section id")
        .to_owned();

    // 4. update_section with new content via JSON dispatch
    let updated = call_ok(
        &server,
        "update_section",
        json!({
            "document_id": doc_id,
            "section_id": draft_id,
            "content": "Updated via JSON dispatch test."
        }),
    );
    assert_eq!(updated["title"], "Draft");

    // 5. verify the file was actually written
    let file = fs::read_to_string(workspace.join("notes.md")).unwrap();
    assert!(
        file.contains("Updated via JSON dispatch test."),
        "file should contain updated content:\n{file}"
    );
}

#[test]
fn dispatch_create_section_via_json() {
    let workspace = scratch_workspace("create-section-dispatch");
    write_markdown(&workspace, "doc.md", "# Doc\n\nRoot content.\n");
    let server = open_server(&workspace);

    let docs = call_ok(&server, "list_documents", json!({}));
    let doc_id = docs[0]["id"].as_str().unwrap().to_owned();
    call_ok(
        &server,
        "manage_document_file",
        json!({ "document_id": doc_id }),
    );

    let new_section = call_ok(
        &server,
        "create_section",
        json!({
            "document_id": doc_id,
            "title": "New Section",
            "content": "Brand new."
        }),
    );
    assert_eq!(new_section["title"], "New Section");
    assert!(new_section["section_id"].is_string());

    // Confirm it appears in the table of contents.
    let toc = call_ok(
        &server,
        "table_of_contents",
        json!({ "document_id": doc_id }),
    );
    let titles: Vec<&str> = toc
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap_or(""))
        .collect();
    assert!(
        titles.contains(&"New Section"),
        "new section should appear in ToC: {titles:?}"
    );
}

// ── tool dispatch: section history via JSON ───────────────────────────────────

#[test]
fn dispatch_section_versions_lists_history_after_edits() {
    let workspace = scratch_workspace("versions-dispatch");
    // Single H1 so the target section is directly accessible as toc[0].
    write_markdown(&workspace, "doc.md", "# Body\n\nVersion 1.\n");
    let server = open_server(&workspace);

    let docs = call_ok(&server, "list_documents", json!({}));
    let doc_id = docs[0]["id"].as_str().unwrap().to_owned();
    call_ok(
        &server,
        "manage_document_file",
        json!({ "document_id": doc_id }),
    );

    let toc = call_ok(
        &server,
        "table_of_contents",
        json!({ "document_id": doc_id }),
    );
    let section_id = toc[0]["section_id"].as_str().unwrap().to_owned();

    // Make an edit to create a second version.
    call_ok(
        &server,
        "update_section",
        json!({
            "document_id": doc_id,
            "section_id": section_id,
            "content": "Version 2."
        }),
    );

    let versions = call_ok(
        &server,
        "section_versions",
        json!({
            "document_id": doc_id,
            "section_id": section_id
        }),
    );
    let version_list = versions.as_array().expect("versions should be array");
    assert!(
        version_list.len() >= 2,
        "should have at least 2 versions after one edit"
    );
}

// ── tool dispatch: set_workspace ──────────────────────────────────────────────

#[test]
fn dispatch_set_workspace_switches_to_new_workspace() {
    let ws_a = scratch_workspace("sw-a");
    write_markdown(&ws_a, "alpha.md", "# Alpha\n\nFrom workspace A.\n");
    let ws_b = scratch_workspace("sw-b");
    write_markdown(&ws_b, "beta.md", "# Beta\n\nFrom workspace B.\n");

    let server = open_server(&ws_a);

    // Verify we see workspace A.
    let docs_a = call_ok(&server, "list_documents", json!({}));
    assert_eq!(docs_a.as_array().unwrap().len(), 1);
    let name_a = docs_a[0]["name"].as_str().unwrap().to_ascii_lowercase();
    assert!(
        name_a.contains("alpha"),
        "expected alpha document: {name_a}"
    );

    // Switch to workspace B.
    let info = call_ok(
        &server,
        "set_workspace",
        json!({ "workspace": ws_b.to_string_lossy() }),
    );
    assert_eq!(info["database"], "filesystem");
    assert!(
        info["workspace"].as_str().unwrap().contains("sw-b"),
        "workspace should point to B"
    );

    // Now list_documents should see workspace B.
    let docs_b = call_ok(&server, "list_documents", json!({}));
    assert_eq!(docs_b.as_array().unwrap().len(), 1);
    let name_b = docs_b[0]["name"].as_str().unwrap().to_ascii_lowercase();
    assert!(
        name_b.contains("beta"),
        "expected beta document after switch: {name_b}"
    );
}

#[test]
fn dispatch_set_workspace_invalid_path_returns_error() {
    let ws = scratch_workspace("sw-err");
    let server = open_server(&ws);

    // A path that doesn't exist should fail gracefully.
    let err = call_err(
        &server,
        "set_workspace",
        json!({
            "workspace": "/nonexistent/path/that/cannot/exist/12345"
        }),
    );
    assert!(
        matches!(err.code, vds::mcp::McpErrorCode::InvalidInput),
        "expected InvalidInput for bad workspace path, got {:?}: {}",
        err.code,
        err.message
    );
}
