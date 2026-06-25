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

//! Golden tests for the exact Markdown diff produced by every VDS 2 mutation.
//!
//! Each test:
//!   1. Writes a known input Markdown file.
//!   2. Opens a `FilesystemVdsServer`, manages the file, and performs exactly
//!      one mutation through `server.call()`.
//!   3. Reads the file back from disk and asserts its exact byte content.
//!
//! These tests guard against accidental whitespace normalisation, heading
//! attribute loss, fenced-code corruption, or comment stripping.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use uuid::Uuid;
use vds::filesystem_service::FilesystemVdsServer;
use rmcp::model::JsonObject;

// ── helpers ──────────────────────────────────────────────────────────────────

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("vds-mcp-golden-tests")
        .join(format!("{name}-{}", Uuid::now_v7()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_md(workspace: &PathBuf, name: &str, content: &str) -> PathBuf {
    let path = workspace.join(name);
    fs::write(&path, content).unwrap();
    path
}

fn open(workspace: &PathBuf) -> FilesystemVdsServer {
    FilesystemVdsServer::open(workspace).expect("open server")
}

fn args(value: Value) -> Option<JsonObject> {
    match value {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

fn call_ok(server: &FilesystemVdsServer, tool: &str, arguments: Value) -> Value {
    let raw = server
        .call(tool, args(arguments))
        .unwrap_or_else(|e| panic!("{tool}: {:?}: {}", e.code, e.message));
    // Unwrap single-key transport envelopes produced by the dispatcher's to_value().
    match &raw {
        Value::Object(map) if map.len() == 1 => {
            if let Some(items) = map.get("items").filter(|v| v.is_array()) {
                return items.clone();
            }
            if let Some(content) = map.get("content").filter(|v| v.is_string()) {
                return content.clone();
            }
            raw
        }
        _ => raw,
    }
}

/// Open, list, manage, and return (server, doc_id).
fn managed_server(workspace: &PathBuf) -> (FilesystemVdsServer, String) {
    let server = open(workspace);
    let docs = call_ok(&server, "list_documents", json!({}));
    let doc_id = docs[0]["id"].as_str().unwrap().to_owned();
    call_ok(&server, "manage_document_file", json!({ "document_id": doc_id }));
    (server, doc_id)
}

/// Return the section_id for the first ToC entry with the given title.
fn section_id_by_title(server: &FilesystemVdsServer, doc_id: &str, title: &str) -> String {
    let toc = call_ok(server, "table_of_contents", json!({ "document_id": doc_id }));
    find_in_toc(&toc, title)
        .unwrap_or_else(|| panic!("section {title:?} not found in ToC:\n{toc}"))
}

fn find_in_toc(entries: &Value, title: &str) -> Option<String> {
    if let Some(arr) = entries.as_array() {
        for entry in arr {
            if entry["title"].as_str() == Some(title) {
                return entry["section_id"].as_str().map(|s| s.to_owned());
            }
            if let Some(id) = find_in_toc(&entry["children"], title) {
                return Some(id);
            }
        }
    }
    None
}

// ── update_section: surgical content replacement ──────────────────────────────

#[test]
fn golden_update_section_middle() {
    let ws = scratch("gold-update-mid");
    let file = write_md(
        &ws,
        "doc.md",
        "# Title\n\nPreamble.\n\n## Alpha\n\nOriginal alpha.\n\n## Beta\n\nBeta body.\n",
    );
    let (server, doc_id) = managed_server(&ws);
    let sid = section_id_by_title(&server, &doc_id, "Alpha");

    call_ok(&server, "update_section", json!({
        "document_id": doc_id,
        "section_id": sid,
        "content": "Replaced alpha."
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert_eq!(
        result,
        "# Title\n\nPreamble.\n\n## Alpha\n\nReplaced alpha.\n\n## Beta\n\nBeta body.\n",
        "update_section produced unexpected output"
    );
}

#[test]
fn golden_update_section_last() {
    let ws = scratch("gold-update-last");
    let file = write_md(
        &ws,
        "doc.md",
        "# Doc\n\n## First\n\nFirst body.\n\n## Last\n\nOriginal last.\n",
    );
    let (server, doc_id) = managed_server(&ws);
    let sid = section_id_by_title(&server, &doc_id, "Last");

    call_ok(&server, "update_section", json!({
        "document_id": doc_id,
        "section_id": sid,
        "content": "New last body."
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert_eq!(
        result,
        "# Doc\n\n## First\n\nFirst body.\n\n## Last\n\nNew last body.\n"
    );
}

// ── rename_section: only the heading line changes ─────────────────────────────

#[test]
fn golden_rename_section_heading_only() {
    let ws = scratch("gold-rename");
    let file = write_md(
        &ws,
        "doc.md",
        "# Doc\n\n## Alpha\n\nAlpha body.\n\n## Beta\n\nBeta body.\n",
    );
    let (server, doc_id) = managed_server(&ws);
    let sid = section_id_by_title(&server, &doc_id, "Alpha");

    call_ok(&server, "rename_section", json!({
        "document_id": doc_id,
        "section_id": sid,
        "new_title": "Renamed"
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert_eq!(
        result,
        "# Doc\n\n## Renamed\n\nAlpha body.\n\n## Beta\n\nBeta body.\n"
    );
}

// ── append_to_section ─────────────────────────────────────────────────────────

#[test]
fn golden_append_to_section() {
    let ws = scratch("gold-append");
    let file = write_md(
        &ws,
        "doc.md",
        "# Doc\n\n## Section\n\nExisting content.\n\n## After\n\nAfter body.\n",
    );
    let (server, doc_id) = managed_server(&ws);
    let sid = section_id_by_title(&server, &doc_id, "Section");

    call_ok(&server, "append_to_section", json!({
        "document_id": doc_id,
        "section_id": sid,
        "content": "Appended line."
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert_eq!(
        result,
        "# Doc\n\n## Section\n\nExisting content.\n\nAppended line.\n\n## After\n\nAfter body.\n",
        "append_to_section should add content after existing body"
    );
}

// ── create_section: structural re-render ─────────────────────────────────────

#[test]
fn golden_create_section_appended_to_document() {
    let ws = scratch("gold-create");
    let file = write_md(&ws, "doc.md", "# Doc\n\nRoot content.\n");
    let (server, doc_id) = managed_server(&ws);

    // Without a parent_id the section is created as a sibling of Doc (both children
    // of the synthetic root), so the new section gets level 1 (H1).
    call_ok(&server, "create_section", json!({
        "document_id": doc_id,
        "title": "New Section",
        "content": "New body."
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert_eq!(
        result,
        "# Doc\n\nRoot content.\n\n# New Section\n\nNew body.\n",
        "create_section without parent_id appends an H1 sibling of Doc"
    );
}

#[test]
fn golden_create_section_as_child_produces_h2() {
    let ws = scratch("gold-create-child");
    let file = write_md(&ws, "doc.md", "# Doc\n\nRoot content.\n");
    let (server, doc_id) = managed_server(&ws);
    let doc_sid = section_id_by_title(&server, &doc_id, "Doc");

    // Creating under Doc (H1) produces an H2.
    call_ok(&server, "create_section", json!({
        "document_id": doc_id,
        "parent_id": doc_sid,
        "title": "Child Section",
        "content": "Child body."
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert_eq!(
        result,
        "# Doc\n\nRoot content.\n\n## Child Section\n\nChild body.\n",
        "create_section under an H1 parent produces an H2 child"
    );
}

// ── remove_section ────────────────────────────────────────────────────────────

#[test]
fn golden_remove_section_middle() {
    let ws = scratch("gold-remove");
    let file = write_md(
        &ws,
        "doc.md",
        "# Doc\n\n## Keep A\n\nA body.\n\n## Remove\n\nRemove body.\n\n## Keep B\n\nB body.\n",
    );
    let (server, doc_id) = managed_server(&ws);
    let sid = section_id_by_title(&server, &doc_id, "Remove");

    call_ok(&server, "remove_section", json!({
        "document_id": doc_id,
        "section_id": sid,
        "remove_children": false
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert_eq!(
        result,
        "# Doc\n\n## Keep A\n\nA body.\n\n## Keep B\n\nB body.\n"
    );
}

// ── reorder_sections ──────────────────────────────────────────────────────────

#[test]
fn golden_reorder_sections_reverses_children() {
    let ws = scratch("gold-reorder");
    let file = write_md(
        &ws,
        "doc.md",
        "# Doc\n\n## First\n\nFirst.\n\n## Second\n\nSecond.\n\n## Third\n\nThird.\n",
    );
    let (server, doc_id) = managed_server(&ws);
    let id_first = section_id_by_title(&server, &doc_id, "First");
    let id_second = section_id_by_title(&server, &doc_id, "Second");
    let id_third = section_id_by_title(&server, &doc_id, "Third");

    // Get the doc root section id from ToC parent
    let toc = call_ok(&server, "table_of_contents", json!({ "document_id": doc_id }));
    // The "Doc" heading is the parent; get its section_id.
    let parent_id = find_in_toc(&toc, "Doc").expect("Doc section");

    call_ok(&server, "reorder_sections", json!({
        "document_id": doc_id,
        "parent_id": parent_id,
        "ordered_children": [id_third, id_second, id_first]
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert_eq!(
        result,
        "# Doc\n\n## Third\n\nThird.\n\n## Second\n\nSecond.\n\n## First\n\nFirst.\n"
    );
}

// ── split_section ─────────────────────────────────────────────────────────────

#[test]
fn golden_split_section_produces_two_sections() {
    let ws = scratch("gold-split");
    let file = write_md(
        &ws,
        "doc.md",
        "# Doc\n\n## Combined\n\nFirst part.\n\nSecond part.\n",
    );
    let (server, doc_id) = managed_server(&ws);
    let sid = section_id_by_title(&server, &doc_id, "Combined");

    // Content of Combined is "First part.\n\nSecond part." — split at byte 13
    // (after "First part.\n") puts "Second part." into the new section.
    call_ok(&server, "split_section", json!({
        "document_id": doc_id,
        "section_id": sid,
        "new_title": "Part Two",
        "split_at": 13
    }));

    let result = fs::read_to_string(&file).unwrap();
    // The original section keeps the first part; the new section gets the second.
    assert!(result.contains("## Combined\n"), "original heading kept");
    assert!(result.contains("First part."), "first part kept");
    assert!(result.contains("## Part Two\n"), "new section created");
    assert!(result.contains("Second part."), "second part in new section");
    // The split point separates them — second part must not appear under Combined.
    let combined_start = result.find("## Combined").unwrap();
    let part_two_start = result.find("## Part Two").unwrap();
    let second_in_combined = result[combined_start..part_two_start].contains("Second part.");
    assert!(!second_in_combined, "second part must not appear before Part Two heading");
}

// ── preservation: fenced code through structural mutation ─────────────────────

#[test]
fn golden_fenced_code_survives_create_section() {
    let ws = scratch("gold-fence-create");
    let file = write_md(
        &ws,
        "doc.md",
        concat!(
            "# Guide\n\n",
            "## Install\n\n",
            "```sh\ncargo install vds\n```\n",
        ),
    );
    let (server, doc_id) = managed_server(&ws);

    // Create Usage as a child of Guide (H1) so it becomes H2.
    let guide_sid = section_id_by_title(&server, &doc_id, "Guide");
    call_ok(&server, "create_section", json!({
        "document_id": doc_id,
        "parent_id": guide_sid,
        "title": "Usage",
        "content": "Run `vds`."
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert!(
        result.contains("```sh\ncargo install vds\n```"),
        "fenced code block must survive structural create_section:\n{result}"
    );
    assert!(result.contains("## Usage\n\nRun `vds`."), "new H2 section created:\n{result}");
}

#[test]
fn golden_fenced_code_survives_reorder() {
    let ws = scratch("gold-fence-reorder");
    let file = write_md(
        &ws,
        "doc.md",
        concat!(
            "# Doc\n\n",
            "## A\n\n",
            "```python\ndef foo(): pass\n```\n\n",
            "## B\n\n",
            "Plain text.\n",
        ),
    );
    let (server, doc_id) = managed_server(&ws);
    let id_a = section_id_by_title(&server, &doc_id, "A");
    let id_b = section_id_by_title(&server, &doc_id, "B");
    let toc = call_ok(&server, "table_of_contents", json!({ "document_id": doc_id }));
    let parent_id = find_in_toc(&toc, "Doc").expect("Doc section");

    call_ok(&server, "reorder_sections", json!({
        "document_id": doc_id,
        "parent_id": parent_id,
        "ordered_children": [id_b, id_a]
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert!(
        result.contains("```python\ndef foo(): pass\n```"),
        "fenced code block must survive reorder:\n{result}"
    );
    // B is now before A.
    let pos_b = result.find("## B").unwrap();
    let pos_a = result.find("## A").unwrap();
    assert!(pos_b < pos_a, "B should appear before A after reorder");
}

// ── preservation: heading attributes through structural mutation ───────────────

#[test]
fn golden_heading_id_survives_create_section() {
    let ws = scratch("gold-anchor-create");
    let file = write_md(
        &ws,
        "doc.md",
        "# Guide {#guide-anchor}\n\n## Install {#install-anchor}\n\nInstall body.\n",
    );
    let (server, doc_id) = managed_server(&ws);

    call_ok(&server, "create_section", json!({
        "document_id": doc_id,
        "title": "Usage",
        "content": "Usage body."
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert!(
        result.contains("# Guide {#guide-anchor}"),
        "H1 anchor must survive create_section:\n{result}"
    );
    assert!(
        result.contains("## Install {#install-anchor}"),
        "H2 anchor must survive create_section:\n{result}"
    );
}

// ── preservation: HTML comments through surgical edit ─────────────────────────

#[test]
fn golden_html_comment_survives_content_edit() {
    let ws = scratch("gold-comment-edit");
    let file = write_md(
        &ws,
        "doc.md",
        concat!(
            "# Doc\n\n",
            "<!-- top-level comment -->\n\n",
            "## Section A\n\n",
            "<!-- section comment -->\nA body.\n\n",
            "## Section B\n\n",
            "Original B.\n",
        ),
    );
    let (server, doc_id) = managed_server(&ws);
    let sid = section_id_by_title(&server, &doc_id, "Section B");

    call_ok(&server, "update_section", json!({
        "document_id": doc_id,
        "section_id": sid,
        "content": "New B."
    }));

    let result = fs::read_to_string(&file).unwrap();
    assert!(result.contains("<!-- top-level comment -->"), "top-level comment preserved");
    assert!(result.contains("<!-- section comment -->"), "section comment preserved");
    assert!(result.contains("## Section B\n\nNew B.\n"), "edit applied correctly");
}
