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

use uuid::Uuid;
#[cfg(feature = "semantic-search")]
use vds::document::TextEmbedding;
use vds::document::{EditOptions, PatchOp, SectionId, SectionPatch, VersionId};
#[cfg(feature = "semantic-search")]
use vds::markdown::import_markdown_str;
use vds::mcp::{
    AppendToSectionParams, CheckConflictsParams, CreateDocumentParams,
    CreateDocumentSnapshotParams, CreateSectionParams, DeleteDocumentParams,
    DiffDocumentSnapshotsParams, DiffSectionVersionsParams, ExportDocumentParams,
    FindByTitleParams, GetSectionParams, GetSectionTreeParams, ImportDocumentParams,
    InsertPosition, ListDocumentsParams, ListRecentChangesParams, LockSectionParams,
    MoveSectionParams, NormalizeDocumentParams, NormalizeOptions, PatchSectionParams,
    RemoveSectionParams, RenderDocumentMarkdownParams, RenderSectionMarkdownParams,
    RepairDocumentParams, RestoreDocumentSnapshotParams, SearchOptions, SearchSectionsParams,
    SectionVersionsParams, TableOfContentsParams, UnlockSectionParams, ValidateDocumentParams,
    VdsMcpSurface,
};
#[cfg(feature = "semantic-search")]
use vds::mcp::{SemanticSearchOptions, SemanticSearchSectionsParams};
use vds::service::VdsServer;
#[cfg(feature = "semantic-search")]
use vds::storage::DocumentStore;

const OVERVIEW: &str = include_str!("../docs/overview.md");

fn test_db_path(name: &str) -> PathBuf {
    let dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("integration-dbs");
    fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{name}-{}.redb", Uuid::now_v7()))
}

fn overview_server(name: &str) -> (VdsServer, vds::mcp::DocumentInfo) {
    let server = VdsServer::open(test_db_path(name)).unwrap();
    let document = server
        .create_document(CreateDocumentParams {
            relative_path: None,
            name: Some("overview".to_owned()),
            title: None,
            initial_content: Some(OVERVIEW.to_owned()),
        })
        .unwrap();
    (server, document)
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
fn creates_overview_document_and_navigates_section_tree() {
    let (server, document) = overview_server("navigate");

    let documents = server
        .list_documents(ListDocumentsParams::default())
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, document.id);

    let toc = server
        .table_of_contents(TableOfContentsParams {
            document_id: document.id.clone(),
        })
        .unwrap();
    let index_id = first_titled_section(&toc, "In-Memory Index").unwrap();

    let index_section = server
        .get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: index_id.clone(),
        })
        .unwrap();
    assert_eq!(index_section.title, "In-Memory Index");
    assert!(!index_section.children.is_empty());

    let tree = server
        .get_section_tree(GetSectionTreeParams {
            document_id: document.id.clone(),
            section_id: index_id,
            depth: Some(1),
        })
        .unwrap();
    assert_eq!(tree.section.title, "In-Memory Index");
    assert!(
        tree.children
            .iter()
            .any(|child| child.section.title == "Full-Text Search")
    );
}

#[test]
fn renders_searches_versions_and_checks_conflicts_for_overview() {
    let (server, document) = overview_server("service-surface");
    let toc = server
        .table_of_contents(TableOfContentsParams {
            document_id: document.id.clone(),
        })
        .unwrap();
    let search_section_id = first_titled_section(&toc, "Full-Text Search").unwrap();

    let rendered_section = server
        .render_section_markdown(RenderSectionMarkdownParams {
            document_id: document.id.clone(),
            section_id: search_section_id.clone(),
            include_children: false,
        })
        .unwrap();
    assert!(rendered_section.starts_with("### Full-Text Search"));
    assert!(rendered_section.contains("BM25"));

    let rendered_document = server
        .render_document_markdown(RenderDocumentMarkdownParams {
            document_id: document.id.clone(),
        })
        .unwrap();
    assert!(rendered_document.contains("# Versioned Document Service"));
    assert!(rendered_document.contains("## Mutation Durability"));

    let search_results = server
        .search_sections(SearchSectionsParams {
            document_id: document.id.clone(),
            query: "stable".to_owned(),
            options: Some(SearchOptions {
                include_content: true,
                include_titles: true,
                fuzzy_titles: false,
                max_results: None,
            }),
        })
        .unwrap();
    assert!(!search_results.is_empty());

    let title_results = server
        .find_by_title(FindByTitleParams {
            document_id: document.id.clone(),
            title: "Full-Text Search".to_owned(),
            fuzzy: false,
        })
        .unwrap();
    assert_eq!(title_results.len(), 1);

    let versions = server
        .section_versions(SectionVersionsParams {
            document_id: document.id.clone(),
            section_id: search_section_id.clone(),
        })
        .unwrap();
    assert_eq!(versions.len(), 1);

    let current = versions[0].version_id.clone();
    let clean_conflict_check = server
        .check_conflicts(CheckConflictsParams {
            document_id: document.id.clone(),
            section_id: search_section_id.clone(),
            expected_version: current,
        })
        .unwrap();
    assert!(!clean_conflict_check.conflicted);

    let stale_conflict_check = server
        .check_conflicts(CheckConflictsParams {
            document_id: document.id,
            section_id: search_section_id,
            expected_version: VersionId::new("ver-stale"),
        })
        .unwrap();
    assert!(stale_conflict_check.conflicted);
}

#[test]
fn imports_and_exports_overview_file_through_service() {
    let server = VdsServer::open(test_db_path("import-export")).unwrap();
    let document = server
        .import_document(ImportDocumentParams {
            name: "overview-file".to_owned(),
            path: "docs/overview.md".to_owned(),
        })
        .unwrap();
    let output = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("integration-dbs")
        .join(format!("overview-export-{}.md", Uuid::now_v7()));

    let export = server
        .export_document(ExportDocumentParams {
            document_id: document.id,
            path: output.to_string_lossy().into_owned(),
        })
        .unwrap();
    let exported = fs::read_to_string(&output).unwrap();

    assert_eq!(export.bytes_written as usize, exported.len());
    assert!(exported.contains("# Versioned Document Service"));
    assert!(exported.contains("## Mutation Durability"));
}

#[test]
fn validation_normalization_and_repair_are_noop_for_overview() {
    let (server, document) = overview_server("maintenance");

    let diagnostics = server
        .validate_document(ValidateDocumentParams {
            document_id: document.id.clone(),
        })
        .unwrap();
    assert!(diagnostics.is_empty());

    let normalized = server
        .normalize_document(NormalizeDocumentParams {
            document_id: document.id.clone(),
            options: Some(NormalizeOptions {
                fix_heading_levels: true,
                regenerate_anchors: true,
                trim_whitespace: true,
                remove_empty_sections: false,
            }),
        })
        .unwrap();
    assert!(!normalized.changed);
    assert!(normalized.diagnostics.is_empty());

    let repaired = server
        .repair_document(RepairDocumentParams {
            document_id: document.id,
        })
        .unwrap();
    assert!(!repaired.repaired);
    assert!(repaired.diagnostics.is_empty());
}

#[test]
fn editing_history_locking_and_deletion_are_storage_backed() {
    let server = VdsServer::open(test_db_path("editing")).unwrap();
    let document = server
        .create_document(CreateDocumentParams {
            relative_path: None,
            name: Some("editing".to_owned()),
            title: None,
            initial_content: Some("# Guide\n\nStart\n".to_owned()),
        })
        .unwrap();
    let toc = server
        .table_of_contents(TableOfContentsParams {
            document_id: document.id.clone(),
        })
        .unwrap();
    let guide_id = first_titled_section(&toc, "Guide").unwrap();

    let first = server
        .create_section(CreateSectionParams {
            document_id: document.id.clone(),
            parent_id: Some(guide_id.clone()),
            title: "First".to_owned(),
            content: "Alpha".to_owned(),
            position: Some(InsertPosition::First),
        })
        .unwrap();
    let second = server
        .create_section(CreateSectionParams {
            document_id: document.id.clone(),
            parent_id: Some(guide_id.clone()),
            title: "Second".to_owned(),
            content: "Beta".to_owned(),
            position: Some(InsertPosition::Last),
        })
        .unwrap();

    let edit_options = EditOptions {
        expected_version: Some(first.current_version.clone()),
        author: Some("test".to_owned()),
        change_summary: Some("patch first".to_owned()),
    };
    let patched = server
        .patch_section(PatchSectionParams {
            document_id: document.id.clone(),
            section_id: first.section_id.clone(),
            patch: SectionPatch {
                operations: vec![
                    PatchOp::AppendContent {
                        content: "\nMore".to_owned(),
                    },
                    PatchOp::Rename {
                        title: "First Updated".to_owned(),
                    },
                ],
            },
            options: Some(edit_options),
        })
        .unwrap();
    assert_eq!(patched.title, "First Updated");

    let versions = server
        .section_versions(SectionVersionsParams {
            document_id: document.id.clone(),
            section_id: first.section_id.clone(),
        })
        .unwrap();
    assert_eq!(versions.len(), 2);
    let version_diff = server
        .diff_section_versions(DiffSectionVersionsParams {
            document_id: document.id.clone(),
            section_id: first.section_id.clone(),
            from_version: versions[0].version_id.clone(),
            to_version: versions[1].version_id.clone(),
        })
        .unwrap();
    assert!(!version_diff.hunks[0].lines.is_empty());

    let before_move = server
        .create_document_snapshot(CreateDocumentSnapshotParams {
            document_id: document.id.clone(),
            label: Some("before move".to_owned()),
            change_summary: None,
        })
        .unwrap();

    server
        .move_section(MoveSectionParams {
            document_id: document.id.clone(),
            section_id: second.section_id.clone(),
            new_parent_id: Some(first.section_id.clone()),
            position: Some(InsertPosition::Last),
            options: Some(EditOptions {
                expected_version: None,
                author: None,
                change_summary: Some("move second".to_owned()),
            }),
        })
        .unwrap();
    let after_move = server
        .create_document_snapshot(CreateDocumentSnapshotParams {
            document_id: document.id.clone(),
            label: Some("after move".to_owned()),
            change_summary: None,
        })
        .unwrap();
    let snapshot_diff = server
        .diff_document_snapshots(DiffDocumentSnapshotsParams {
            document_id: document.id.clone(),
            from_snapshot: before_move.snapshot_id.clone(),
            to_snapshot: after_move.snapshot_id.clone(),
        })
        .unwrap();
    assert!(!snapshot_diff.hunks[0].lines.is_empty());

    server
        .restore_document_snapshot(RestoreDocumentSnapshotParams {
            document_id: document.id.clone(),
            snapshot_id: before_move.snapshot_id,
        })
        .unwrap();
    let rendered = server
        .render_document_markdown(RenderDocumentMarkdownParams {
            document_id: document.id.clone(),
        })
        .unwrap();
    assert!(rendered.contains("## First Updated"));
    assert!(rendered.contains("## Second"));

    let lock = server
        .lock_section(LockSectionParams {
            document_id: document.id.clone(),
            section_id: first.section_id.clone(),
            owner: "test".to_owned(),
            ttl_seconds: Some(30),
        })
        .unwrap();
    assert_eq!(lock.owner, "test");
    assert!(
        server
            .append_to_section(AppendToSectionParams {
                document_id: document.id.clone(),
                section_id: first.section_id.clone(),
                content: "\nBlocked".to_owned(),
                options: Some(EditOptions {
                    expected_version: None,
                    author: None,
                    change_summary: None,
                }),
            })
            .is_err()
    );
    assert!(
        server
            .unlock_section(UnlockSectionParams {
                document_id: document.id.clone(),
                section_id: first.section_id.clone(),
                owner: "test".to_owned(),
            })
            .unwrap()
            .unlocked
    );

    let changes = server
        .list_recent_changes(ListRecentChangesParams {
            document_id: document.id.clone(),
            limit: Some(5),
        })
        .unwrap();
    assert!(!changes.is_empty());

    server
        .remove_section(RemoveSectionParams {
            document_id: document.id.clone(),
            section_id: second.section_id,
            remove_children: true,
            options: Some(EditOptions {
                expected_version: None,
                author: None,
                change_summary: Some("remove second".to_owned()),
            }),
        })
        .unwrap();
    let deleted = server
        .delete_document(DeleteDocumentParams {
            document_id: document.id,
        })
        .unwrap();
    assert!(deleted.deleted);
    assert!(deleted.sections_deleted >= 2);
    assert!(deleted.versions_deleted >= 2);
    assert!(deleted.snapshots_deleted >= 1);
}

#[test]
fn mcp_json_request_compatibility_accepts_agent_friendly_shapes() {
    let server = VdsServer::open(test_db_path("compat")).unwrap();
    let document = server
        .create_document(CreateDocumentParams {
            relative_path: None,
            name: Some("compat".to_owned()),
            title: None,
            initial_content: Some("# Guide\n\nStart\n".to_owned()),
        })
        .unwrap();
    let toc = server
        .table_of_contents(TableOfContentsParams {
            document_id: document.id.clone(),
        })
        .unwrap();
    let guide_id = first_titled_section(&toc, "Guide").unwrap();

    let created = server
        .create_section(
            serde_json::from_value(serde_json::json!({
                "document_id": document.id,
                "parent_section_id": guide_id,
                "title": "Storage Model",
                "content": "Snapshots capture document-wide points in time while versions support rollback.",
                "position": "last"
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(created.level, 2);
    assert_eq!(created.parent_id, Some(guide_id.clone()));

    let patched = server
        .patch_section(
            serde_json::from_value(serde_json::json!({
                "document_id": document.id,
                "section_id": created.section_id,
                "patch": {
                    "operations": [
                        {
                            "type": "append",
                            "content": "\n\nSections keep edits focused."
                        },
                        {
                            "type": "rename",
                            "title": "Storage and Versions"
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(patched.title, "Storage and Versions");

    let snapshot = server
        .create_document_snapshot(
            serde_json::from_value(serde_json::json!({
                "document_id": document.id,
                "name": "compat snapshot",
                "description": "Created through alias fields"
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(snapshot.label.as_deref(), Some("compat snapshot"));
    assert_eq!(
        snapshot.change_summary.as_deref(),
        Some("Created through alias fields")
    );
}

#[test]
fn search_sections_matches_multi_term_queries_and_honors_limit_alias() {
    let server = VdsServer::open(test_db_path("multi-term-search")).unwrap();
    let document = server
        .create_document(CreateDocumentParams {
            relative_path: None,
            name: Some("search".to_owned()),
            title: None,
            initial_content: Some(
                "# Guide\n\nSnapshots capture document-wide points in time, while versions make rollback possible.\n"
                    .to_owned(),
            ),
        })
        .unwrap();

    let results = server
        .search_sections(
            serde_json::from_value(serde_json::json!({
                "document_id": document.id,
                "query": "snapshots versions rollback",
                "options": {
                    "limit": 1
                }
            }))
            .unwrap(),
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].section.title, "Guide");
    assert!(!results[0].content_matches.is_empty());
}

#[cfg(feature = "semantic-search")]
#[test]
fn semantic_search_returns_nearest_embedded_sections() {
    let path = test_db_path("semantic-search");
    let store = DocumentStore::open(&path).unwrap();
    let document = import_markdown_str(
        &store,
        "semantic",
        None,
        "# Root\n\n## Apples\n\nFruit content.\n\n## Engines\n\nMechanical content.\n",
    )
    .unwrap();

    let sections = store.list_document_sections(&document.id).unwrap();
    for mut section in sections {
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
            options: SemanticSearchOptions {
                max_results: Some(1),
                ef: Some(8),
                m: Some(8),
                ef_construction: Some(100),
                require_same_model: true,
            },
        })
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].section.title, "Apples");
}
