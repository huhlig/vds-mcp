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

use std::fs;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;
#[cfg(feature = "semantic-search")]
use vds::document::TextEmbedding;
use vds::document::{SectionId, SectionMetadata};
#[cfg(feature = "semantic-search")]
use vds::markdown::import_markdown_str;
use vds::mcp::*;
use vds::service::VdsServer;
#[cfg(feature = "semantic-search")]
use vds::storage::DocumentStore;

fn test_db_path(name: &str) -> PathBuf {
    let dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("integration-dbs");
    fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{name}-{}.redb", Uuid::now_v7()))
}

fn artifact_path(name: &str, extension: &str) -> PathBuf {
    let dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("integration-artifacts");
    fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{name}-{}.{}", Uuid::now_v7(), extension))
}

fn agent<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

fn first_titled_section(
    toc: &[vds::document::TableOfContentsEntry],
    title: &str,
) -> Option<SectionId> {
    for entry in toc {
        if entry.title == title {
            return Some(entry.section_id.clone());
        }
        if let Some(section_id) = first_titled_section(&entry.children, title) {
            return Some(section_id);
        }
    }
    None
}

#[test]
fn permanent_mcp_document_import_export_and_delete_smoke() {
    let source = artifact_path("mcp-import-source", "md");
    fs::write(
        &source,
        "# Imported Guide\n\nStart here.\n\n## Imported Child\n\nImported body.\n",
    )
    .unwrap();
    let export = artifact_path("mcp-export-output", "md");
    let server = VdsServer::open(test_db_path("mcp-import-export")).unwrap();

    assert!(
        server
            .list_documents(ListDocumentsParams::default())
            .unwrap()
            .is_empty()
    );

    let imported = server
        .import_document(agent(json!({
            "name": "imported-guide",
            "path": source.to_string_lossy()
        })))
        .unwrap();
    assert_eq!(imported.name, "imported-guide");

    let renamed = server
        .rename_document(agent(json!({
            "document_id": imported.id,
            "name": "renamed-imported-guide"
        })))
        .unwrap();
    assert_eq!(renamed.name, "renamed-imported-guide");

    let document = server
        .get_document(agent(json!({ "document_id": imported.id })))
        .unwrap();
    assert_eq!(document.name, "renamed-imported-guide");

    let exported = server
        .export_document(agent(json!({
            "document_id": imported.id,
            "path": export.to_string_lossy()
        })))
        .unwrap();
    let exported_markdown = fs::read_to_string(&export).unwrap();
    assert_eq!(exported.bytes_written as usize, exported_markdown.len());
    assert!(exported_markdown.contains("# Imported Guide"));
    assert!(exported_markdown.contains("## Imported Child"));

    let deleted = server
        .delete_document(agent(json!({ "document_id": imported.id })))
        .unwrap();
    assert!(deleted.deleted);
    assert!(
        server
            .list_documents(ListDocumentsParams::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn permanent_mcp_full_chained_section_version_snapshot_smoke() {
    let server = VdsServer::open(test_db_path("mcp-full-chain")).unwrap();
    let document = server
        .create_document(agent(json!({
            "name": "agent-smoke",
            "title": "Agent Smoke",
            "markdown": "# Agent Smoke\n\nA durable MCP smoke document.\n\n## Overview\n\nOriginal overview.\n\n## Workflow\n\nAlpha beta gamma delta.\n"
        })))
        .unwrap();

    let toc = server
        .table_of_contents(agent(json!({ "document_id": document.id })))
        .unwrap();
    let root_id = first_titled_section(&toc, "Agent Smoke").unwrap();
    let overview_id = first_titled_section(&toc, "Overview").unwrap();
    let workflow_id = first_titled_section(&toc, "Workflow").unwrap();

    let first = server
        .create_section(agent(json!({
            "document_id": document.id,
            "parent_section_id": root_id,
            "title": "First Agent Section",
            "content": "First body.",
            "position": "first"
        })))
        .unwrap();
    assert_eq!(first.parent_id, Some(root_id.clone()));
    assert_eq!(first.ordinal, 0);

    let storage = server
        .create_section(agent(json!({
            "document_id": document.id,
            "parent_section_id": root_id,
            "title": "Storage Model",
            "content": "Documents sections versions snapshots rollback.",
            "position": "last"
        })))
        .unwrap();
    let before_storage = server
        .insert_section_before(agent(json!({
            "document_id": document.id,
            "sibling_section_id": storage.section_id,
            "title": "Before Storage",
            "content": "Inserted before storage."
        })))
        .unwrap();
    let after_storage = server
        .insert_section_after(agent(json!({
            "document_id": document.id,
            "sibling_section_id": storage.section_id,
            "title": "After Storage",
            "content": "Inserted after storage."
        })))
        .unwrap();

    let sections = server
        .get_sections(agent(json!({
            "document_id": document.id,
            "section_ids": [overview_id, workflow_id, storage.section_id]
        })))
        .unwrap();
    assert_eq!(sections.len(), 3);

    let tree = server
        .get_section_tree(agent(json!({
            "document_id": document.id,
            "section_id": root_id,
            "depth": 2
        })))
        .unwrap();
    assert!(tree.children.len() >= 5);

    let rendered_storage = server
        .render_section_markdown(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "include_children": false
        })))
        .unwrap();
    assert!(rendered_storage.starts_with("## Storage Model"));

    let updated = server
        .update_section(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "content": "Documents, sections, versions, snapshots, and rollback.",
            "options": {
                "author": "agent-smoke",
                "change_summary": "replace storage body"
            }
        })))
        .unwrap();
    let appended = server
        .append_to_section(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "content": "\n\nAppend-only note.",
            "options": {
                "expected_version": updated.current_version,
                "author": "agent-smoke",
                "change_summary": "append note"
            }
        })))
        .unwrap();
    assert_ne!(updated.current_version, appended.current_version);

    let patched = server
        .patch_section(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "patch": {
                "operations": [
                    { "type": "prepend", "content": "Prelude. " },
                    { "type": "replace_range", "start": 0, "end": 8, "content": "Preface." },
                    { "type": "rename", "title": "Storage and History" },
                    {
                        "type": "set_metadata",
                        "metadata": {
                            "anchor": "storage-and-history",
                            "tags": ["storage", "history", "smoke"],
                            "summary": "Storage and history concepts.",
                            "locked": false
                        }
                    }
                ]
            },
            "options": {
                "author": "agent-smoke",
                "change_summary": "agent-style multi-op patch"
            }
        })))
        .unwrap();
    assert_eq!(patched.title, "Storage and History");

    let renamed = server
        .rename_section(agent(json!({
            "document_id": document.id,
            "section_id": after_storage.section_id,
            "new_title": "After Storage Renamed",
            "options": { "change_summary": "rename sibling" }
        })))
        .unwrap();
    assert_eq!(renamed.title, "After Storage Renamed");

    let metadata_updated = server
        .set_section_metadata(agent(json!({
            "document_id": document.id,
            "section_id": before_storage.section_id,
            "metadata": {
                "anchor": "before-storage",
                "tags": ["smoke", "before"],
                "summary": "Before storage sibling.",
                "locked": false
            },
            "options": { "change_summary": "set sibling metadata" }
        })))
        .unwrap();
    assert_eq!(metadata_updated.title, "Before Storage");

    let tag_results = server
        .find_by_tag(agent(json!({
            "document_id": document.id,
            "tag": "smoke"
        })))
        .unwrap();
    assert!(
        tag_results
            .iter()
            .any(|section| section.section_id == storage.section_id)
    );

    let title_results = server
        .find_by_title(agent(json!({
            "document_id": document.id,
            "title": "Storage and History",
            "fuzzy": false
        })))
        .unwrap();
    assert_eq!(title_results.len(), 1);

    let search_results = server
        .search_sections(agent(json!({
            "document_id": document.id,
            "query": "snapshots versions rollback",
            "options": { "limit": 2 }
        })))
        .unwrap();
    assert!(!search_results.is_empty());

    let moved = server
        .move_section(agent(json!({
            "document_id": document.id,
            "section_id": before_storage.section_id,
            "new_parent": storage.section_id,
            "position": { "index": 0 },
            "options": { "change_summary": "move before under storage" }
        })))
        .unwrap();
    assert_eq!(moved.parent_id, Some(storage.section_id.clone()));

    let parent_after_move = server
        .get_section(agent(json!({
            "document_id": document.id,
            "section_id": root_id
        })))
        .unwrap();
    let mut reversed_children = parent_after_move.children.clone();
    reversed_children.reverse();
    let reordered = server
        .reorder_sections(agent(json!({
            "document_id": document.id,
            "parent": root_id,
            "ordered_children": reversed_children,
            "options": { "change_summary": "reverse root children" }
        })))
        .unwrap();
    assert_eq!(reordered.len(), parent_after_move.children.len());

    let demoted = server
        .demote_section(agent(json!({
            "document_id": document.id,
            "section_id": first.section_id,
            "options": { "change_summary": "demote first" }
        })))
        .unwrap();
    assert!(demoted.level >= 2);
    let promoted = server
        .promote_section(agent(json!({
            "document_id": document.id,
            "section_id": first.section_id,
            "options": { "change_summary": "promote first" }
        })))
        .unwrap();
    assert!(promoted.level <= demoted.level);

    let split = server
        .split_section(agent(json!({
            "document_id": document.id,
            "section_id": workflow_id,
            "split_at": 11,
            "new_title": "Workflow Tail"
        })))
        .unwrap();
    assert_eq!(split.original.section_id, workflow_id);
    assert_eq!(split.created.title, "Workflow Tail");

    let versions = server
        .section_versions(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id
        })))
        .unwrap();
    assert!(versions.len() >= 4);
    let first_version = versions.first().unwrap().version_id.clone();
    let latest_version = versions.last().unwrap().version_id.clone();
    let historical = server
        .get_section_version(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "version_id": first_version
        })))
        .unwrap();
    assert_eq!(historical.title, "Storage Model");

    let diff = server
        .diff_section_versions(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "from_version": first_version,
            "to_version": latest_version
        })))
        .unwrap();
    assert!(!diff.hunks.is_empty());

    let stale_conflict = server
        .check_conflicts(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "expected_version": "ver-stale"
        })))
        .unwrap();
    assert!(stale_conflict.conflicted);

    let before_snapshot = server
        .create_document_snapshot(agent(json!({
            "document_id": document.id,
            "name": "before-switch",
            "description": "Before switching storage back"
        })))
        .unwrap();

    let switched = server
        .switch_section_version(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "version_id": first_version,
            "options": { "change_summary": "switch to first version" }
        })))
        .unwrap();
    assert_eq!(switched.title, "Storage Model");

    let after_snapshot = server
        .create_document_snapshot(agent(json!({
            "document_id": document.id,
            "label": "after-switch",
            "change_summary": "After switching storage back"
        })))
        .unwrap();
    let snapshots = server
        .document_snapshots(agent(json!({ "document_id": document.id })))
        .unwrap();
    assert_eq!(snapshots.len(), 2);

    let snapshot_diff = server
        .diff_document_snapshots(agent(json!({
            "document_id": document.id,
            "from_snapshot": before_snapshot.snapshot_id,
            "to_snapshot": after_snapshot.snapshot_id
        })))
        .unwrap();
    assert!(!snapshot_diff.hunks.is_empty());

    server
        .restore_document_snapshot(agent(json!({
            "document_id": document.id,
            "snapshot_id": before_snapshot.snapshot_id
        })))
        .unwrap();
    let restored_storage = server
        .get_section(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id
        })))
        .unwrap();
    assert_eq!(restored_storage.title, "Storage and History");

    let lock = server
        .lock_section(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "owner": "agent-smoke",
            "ttl_seconds": 30
        })))
        .unwrap();
    assert_eq!(lock.owner, "agent-smoke");
    assert!(
        server
            .append_to_section(agent(json!({
                "document_id": document.id,
                "section_id": storage.section_id,
                "content": "\nshould not write"
            })))
            .is_err()
    );
    assert!(
        server
            .unlock_section(agent(json!({
                "document_id": document.id,
                "section_id": storage.section_id,
                "owner": "agent-smoke"
            })))
            .unwrap()
            .unlocked
    );

    let rendered_document = server
        .render_document_markdown(agent(json!({ "document_id": document.id })))
        .unwrap();
    assert!(rendered_document.contains("# Agent Smoke"));
    assert!(rendered_document.contains("## Storage and History"));

    assert!(
        server
            .validate_document(agent(json!({ "document_id": document.id })))
            .unwrap()
            .is_empty()
    );
    assert!(
        !server
            .normalize_document(agent(json!({
                "document_id": document.id,
                "options": {
                    "fix_heading_levels": true,
                    "regenerate_anchors": true,
                    "trim_whitespace": true,
                    "remove_empty_sections": false
                }
            })))
            .unwrap()
            .changed
    );
    assert!(
        !server
            .repair_document(agent(json!({ "document_id": document.id })))
            .unwrap()
            .repaired
    );

    let changes = server
        .list_recent_changes(agent(json!({
            "document_id": document.id,
            "limit": 10
        })))
        .unwrap();
    assert!(!changes.is_empty());

    let removed = server
        .remove_section(agent(json!({
            "document_id": document.id,
            "section_id": storage.section_id,
            "remove_children": true,
            "options": { "change_summary": "remove storage subtree" }
        })))
        .unwrap();
    assert_eq!(removed.section_id, storage.section_id);
    assert!(
        removed
            .removed_children
            .contains(&before_storage.section_id)
    );

    let deleted = server
        .delete_document(agent(json!({ "document_id": document.id })))
        .unwrap();
    assert!(deleted.deleted);
}

#[test]
fn permanent_mcp_agent_shape_deserialization_smoke() {
    assert!(matches!(
        agent::<InsertPosition>(json!("last")),
        InsertPosition::Last
    ));
    assert!(matches!(
        agent::<InsertPosition>(json!({ "before": "sec-before" })),
        InsertPosition::Before(_)
    ));
    assert!(matches!(
        agent::<InsertPosition>(json!({ "After": "sec-after" })),
        InsertPosition::After(_)
    ));
    assert!(matches!(
        agent::<InsertPosition>(json!({ "index": 3 })),
        InsertPosition::Index(3)
    ));
    assert!(matches!(
        agent::<InsertPosition>(json!(4)),
        InsertPosition::Index(4)
    ));

    let metadata = SectionMetadata {
        anchor: Some("agent-shape".to_owned()),
        tags: vec!["shape".to_owned()],
        summary: Some("Agent shape metadata.".to_owned()),
        locked: false,
    };
    let patch: vds::document::SectionPatch = agent(json!({
        "operations": [
            { "type": "replace_content", "content": "new" },
            { "type": "append", "content": " tail" },
            { "type": "prepend", "content": "head " },
            { "type": "replace_range", "start": 0, "end": 4, "content": "HEAD" },
            { "type": "rename", "title": "Renamed" },
            { "type": "set_metadata", "metadata": metadata }
        ]
    }));
    assert_eq!(patch.operations.len(), 6);
}

#[cfg(feature = "semantic-search")]
#[test]
fn permanent_mcp_semantic_search_smoke() {
    let path = test_db_path("mcp-semantic-smoke");
    let store = DocumentStore::open(&path).unwrap();
    let document = import_markdown_str(
        &store,
        "semantic",
        None,
        "# Root\n\n## Apples\n\nFruit content.\n\n## Engines\n\nMechanical content.\n",
    )
    .unwrap();

    for mut section in store.list_document_sections(&document.id).unwrap() {
        section.embedding = Some(TextEmbedding {
            model: Some("test-model".to_owned()),
            vector: if section.title == "Apples" {
                vec![1.0, 0.0]
            } else if section.title == "Engines" {
                vec![0.0, 1.0]
            } else {
                vec![0.5, 0.5]
            },
        });
        store.put_section(&section).unwrap();
    }

    let server = VdsServer::open(path).unwrap();
    let results = server
        .semantic_search_sections(SemanticSearchSectionsParams {
            document_id: document.id,
            query: None,
            query_embedding: Some(TextEmbedding {
                model: Some("test-model".to_owned()),
                vector: vec![1.0, 0.0],
            }),
            model: None,
            options: Some(SemanticSearchOptions {
                max_results: Some(1),
                ef: Some(8),
                m: Some(8),
                ef_construction: Some(100),
                require_same_model: true,
            }),
        })
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].section.title, "Apples");
}
