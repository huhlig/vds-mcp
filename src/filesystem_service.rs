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

//! Read-only MCP service backed by a filesystem-authoritative workspace.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use axum;

use chrono::Utc;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, JsonObject, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as RmcpError, ServiceExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::document::{
    DiagnosticSeverity, Document, DocumentId, DocumentSnapshot, EditOptions, PatchOp, Section,
    SectionId, SectionInfo, SectionMetadata, SectionPatch, SectionVersion, TableOfContentsEntry,
    ValidationDiagnostic, VersionId,
};
use crate::mcp::{
    AppendToSectionParams, CreateDocumentParams, CreateDocumentSnapshotParams, CreateSectionParams,
    DemoteSectionParams, DiffDocumentSnapshotsParams, DiffFormat, DiffHunk, DiffLine,
    DiffLineKind, DiffResult, DiffSectionVersionsParams, DocumentFileMutationResult, DocumentInfo,
    DocumentLocation, DocumentSnapshotInfo, DocumentSnapshotsParams, DatabaseInfo,
    ExportDocumentParams, ExportResult, FindByTagParams, FindByTitleParams,
    FullTextSearchParams, FullTextSearchResult, GetDatabaseParams, GetDocumentLocationParams,
    GetDocumentParams, GetSectionParams, GetSectionTreeParams, GetSectionsParams,
    GetSectionVersionParams, GetWorkspaceParams, ImportDocumentParams, InsertSectionAfterParams,
    InsertSectionBeforeParams, ListDocumentsParams, ManageDocumentFileParams, McpError,
    McpErrorCode, McpResult, MoveDocumentFileParams, MoveSectionParams, PatchSectionParams,
    PromoteSectionParams, RemoveDocumentFileParams, RemovedSectionInfo, RenameDocumentFileParams,
    RenameDocumentParams, RenameSectionParams, RemoveSectionParams, RenderDocumentMarkdownParams,
    RenderSectionMarkdownParams, ReorderSectionsParams, RestoreDocumentFileParams,
    RestoreDocumentSnapshotParams, SearchSectionsParams, SectionSearchResult, SectionTree,
    SectionVersionInfo, SectionVersionsParams, SetSectionMetadataParams, SetWorkspaceParams,
    SplitSectionParams, SplitSectionResult, SwitchSectionVersionParams, TableOfContentsParams,
    TextMatch, UnmanageDocumentFileParams, UpdateSectionParams, ValidateDocumentParams,
    VdsMcpSurface, WorkspaceInfo, tool_documentation,
};
use crate::markdown::{apply_content_edit, apply_heading_rename, render_sections_to_markdown};
use crate::metadata::{CurrentDocumentRecord, MetadataRepository, SectionRecord, SectionMatchingRecord, WorkspaceLease, METADATA_FORMAT_VERSION};
use crate::search::{FullTextIndex, FullTextSearchOptions};
use crate::service::{to_rmcp_error, tool_from_doc};
use crate::workspace::{
    MaterializedDocument, WorkspaceError, WorkspaceState, is_markdown_path_eligible,
};

const AVAILABLE_TOOL_NAMES: &[&str] = &[
    "list_documents",
    "get_document",
    "rename_document",
    "import_document",
    "export_document",
    "get_document_location",
    "create_document",
    "remove_document_file",
    "unmanage_document_file",
    "restore_document_file",
    "manage_document_file",
    "move_document_file",
    "rename_document_file",
    "table_of_contents",
    "get_section",
    "get_section_tree",
    "get_sections",
    "render_section_markdown",
    "render_document_markdown",
    "update_section",
    "append_to_section",
    "rename_section",
    "set_section_metadata",
    "create_section",
    "insert_section_before",
    "insert_section_after",
    "remove_section",
    "reorder_sections",
    "move_section",
    "promote_section",
    "demote_section",
    "patch_section",
    "split_section",
    "section_versions",
    "get_section_version",
    "switch_section_version",
    "diff_section_versions",
    "create_document_snapshot",
    "document_snapshots",
    "restore_document_snapshot",
    "diff_document_snapshots",
    "search_sections",
    "full_text_search",
    #[cfg(feature = "semantic-search")]
    "semantic_search_sections",
    "find_by_title",
    "find_by_tag",
    "validate_document",
    "get_workspace",
    "set_workspace",
    "get_database",
];

struct WorkspaceGeneration {
    state: WorkspaceState,
    full_text: FullTextIndex,
    #[cfg(feature = "semantic-search")]
    semantic: crate::semantic::SemanticIndex,
    /// Monotonically increasing counter incremented on every live reload.
    /// Zero on initial load; non-zero means at least one external change was
    /// integrated since the server started.
    reload_count: u64,
}

impl WorkspaceGeneration {
    fn build(state: WorkspaceState, reload_count: u64) -> Self {
        let full_text = FullTextIndex::build(&state);
        #[cfg(feature = "semantic-search")]
        let semantic = {
            let options = crate::semantic::SemanticSearchOptions::default();
            crate::semantic::SemanticIndex::build(&state, &options)
        };
        Self {
            state,
            full_text,
            #[cfg(feature = "semantic-search")]
            semantic,
            reload_count,
        }
    }
}

/// Workspace-specific state that may be atomically replaced during `set_workspace`.
/// All fields are dropped together when the workspace switches, which stops the old
/// watcher thread (tx drops → rx.recv() returns Err → thread exits) and releases
/// the old write lease before the new one is acquired.
struct SwitchableState {
    workspace_root: PathBuf,
    lease: WorkspaceLease,
    watcher: Option<notify::RecommendedWatcher>,
    watcher_active: bool,
}

/// VDS 2 service over project Markdown and `.vds` JSON metadata.
pub struct FilesystemVdsServer {
    /// Mutable workspace-specific state; replaced atomically on `set_workspace`.
    switchable: Mutex<SwitchableState>,
    /// In-memory materialized workspace and full-text index; shared with the watcher thread.
    /// The `Arc` identity never changes — only the `WorkspaceGeneration` value inside it.
    generation: Arc<RwLock<WorkspaceGeneration>>,
    /// Prevents concurrent mutations; also guards `set_workspace` from racing with writes.
    mutation_lock: Mutex<()>,
}

impl FilesystemVdsServer {
    /// Parses and indexes one workspace, acquiring an exclusive write lease.
    ///
    /// Returns `Err(WorkspaceError::Lease)` if another VDS writer process
    /// already holds the lease for the same workspace root.
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let lease = WorkspaceLease::acquire(workspace_root.as_ref())
            .map_err(WorkspaceError::Lease)?;
        if let Ok(repository) = MetadataRepository::open(workspace_root.as_ref()) {
            repository
                .recover_transactions()
                .map_err(WorkspaceError::Metadata)?;
        }
        let state = WorkspaceState::load(workspace_root)?;
        let workspace_root = state.root().to_path_buf();
        let generation = Arc::new(RwLock::new(WorkspaceGeneration::build(state, 0)));

        let (watcher, watcher_active) = start_watcher(workspace_root.clone(), Arc::clone(&generation));

        Ok(Self {
            switchable: Mutex::new(SwitchableState { workspace_root, lease, watcher, watcher_active }),
            generation,
            mutation_lock: Mutex::new(()),
        })
    }

    /// Returns the current workspace root path.
    fn workspace_root(&self) -> PathBuf {
        self.switchable.lock().unwrap().workspace_root.clone()
    }

    /// Returns whether the filesystem watcher is currently active.
    fn watcher_active(&self) -> bool {
        self.switchable.lock().unwrap().watcher_active
    }

    /// Builds a replacement generation off-lock and publishes it atomically.
    pub fn reload(&self) -> Result<(), WorkspaceError> {
        MetadataRepository::open(&self.workspace_root())
            .map_err(WorkspaceError::Metadata)?
            .recover_transactions()
            .map_err(WorkspaceError::Metadata)?;
        let state = WorkspaceState::load(&self.workspace_root())?;
        let mut lock = self.generation.write().unwrap();
        let reload_count = lock.reload_count + 1;
        *lock = WorkspaceGeneration::build(state, reload_count);
        Ok(())
    }

    /// Like `reload`, but updates only one document's postings in the full-text index
    /// instead of rebuilding from scratch.  The workspace state is still fully reloaded
    /// from disk so that all in-memory structures are consistent; only the index update
    /// step is made incremental.
    fn reload_incremental(&self, document_id: &DocumentId) -> Result<(), WorkspaceError> {
        MetadataRepository::open(&self.workspace_root())
            .map_err(WorkspaceError::Metadata)?
            .recover_transactions()
            .map_err(WorkspaceError::Metadata)?;
        let state = WorkspaceState::load(&self.workspace_root())?;

        let mut lock = self.generation.write().unwrap();
        // Take the old indices by value so we can mutate them without a full rebuild.
        let mut full_text = std::mem::take(&mut lock.full_text);
        #[cfg(feature = "semantic-search")]
        let mut semantic = std::mem::take(&mut lock.semantic);

        full_text.remove_document(document_id);
        #[cfg(feature = "semantic-search")]
        semantic.remove_document(document_id);

        if let Some(document) = state.document_by_id(document_id) {
            let sections_by_id = document
                .sections
                .iter()
                .map(|s| (s.section_id.clone(), s))
                .collect::<BTreeMap<_, _>>();
            full_text.add_document(document, &sections_by_id);
            #[cfg(feature = "semantic-search")]
            semantic.add_document(document, &sections_by_id);
        }
        let reload_count = lock.reload_count + 1;
        *lock = WorkspaceGeneration {
            state,
            full_text,
            #[cfg(feature = "semantic-search")]
            semantic,
            reload_count
        };
        Ok(())
    }

    fn require_document(&self, document_id: &DocumentId) -> McpResult<MaterializedDocument> {
        self.generation
            .read()
            .unwrap()
            .state
            .document_by_id(document_id)
            .cloned()
            .ok_or_else(|| not_found("document", document_id.as_str()))
    }

    fn require_document_by_path(&self, relative_path: &str) -> McpResult<MaterializedDocument> {
        self.generation
            .read()
            .unwrap()
            .state
            .document_by_path(relative_path)
            .cloned()
            .ok_or_else(|| not_found("document at path", relative_path))
    }

    fn require_section(
        &self,
        document_id: &DocumentId,
        section_id: &SectionId,
    ) -> McpResult<Section> {
        self.require_document(document_id)?
            .sections
            .into_iter()
            .find(|section| &section.section_id == section_id)
            .ok_or_else(|| not_found("section", section_id.as_str()))
    }

    /// Dispatches one MCP tool call by name with JSON-encoded arguments and returns a JSON value.
    /// This is the central routing layer; `call_tool` delegates here after unwrapping the rmcp envelope.
    pub fn call(&self, name: &str, arguments: Option<JsonObject>) -> Result<Value, McpError> {
        match name {
            "list_documents" => to_value(self.list_documents(parse(arguments)?)),
            "get_document" => to_value(self.get_document(parse(arguments)?)),
            "rename_document" => to_value(self.rename_document(parse(arguments)?)),
            "import_document" => to_value(self.import_document(parse(arguments)?)),
            "export_document" => to_value(self.export_document(parse(arguments)?)),
            "get_document_location" => to_value(self.get_document_location(parse(arguments)?)),
            "create_document" => to_value(self.create_document(parse(arguments)?)),
            "remove_document_file" => to_value(self.remove_document_file(parse(arguments)?)),
            "unmanage_document_file" => to_value(self.unmanage_document_file(parse(arguments)?)),
            "restore_document_file" => to_value(self.restore_document_file(parse(arguments)?)),
            "manage_document_file" => to_value(self.manage_document_file(parse(arguments)?)),
            "move_document_file" => to_value(self.move_document_file(parse(arguments)?)),
            "rename_document_file" => to_value(self.rename_document_file(parse(arguments)?)),
            "table_of_contents" => to_value(self.table_of_contents(parse(arguments)?)),
            "get_section" => to_value(self.get_section(parse(arguments)?)),
            "get_section_tree" => to_value(self.get_section_tree(parse(arguments)?)),
            "get_sections" => to_value(self.get_sections(parse(arguments)?)),
            "render_section_markdown" => to_value(self.render_section_markdown(parse(arguments)?)),
            "render_document_markdown" => {
                to_value(self.render_document_markdown(parse(arguments)?))
            }
            "update_section" => to_value(self.update_section(parse(arguments)?)),
            "append_to_section" => to_value(self.append_to_section(parse(arguments)?)),
            "rename_section" => to_value(self.rename_section(parse(arguments)?)),
            "set_section_metadata" => to_value(self.set_section_metadata(parse(arguments)?)),
            "create_section" => to_value(self.create_section(parse(arguments)?)),
            "insert_section_before" => to_value(self.insert_section_before(parse(arguments)?)),
            "insert_section_after" => to_value(self.insert_section_after(parse(arguments)?)),
            "remove_section" => to_value(self.remove_section(parse(arguments)?)),
            "reorder_sections" => to_value(self.reorder_sections(parse(arguments)?)),
            "move_section" => to_value(self.move_section(parse(arguments)?)),
            "promote_section" => to_value(self.promote_section(parse(arguments)?)),
            "demote_section" => to_value(self.demote_section(parse(arguments)?)),
            "patch_section" => to_value(self.patch_section(parse(arguments)?)),
            "split_section" => to_value(self.split_section(parse(arguments)?)),
            "section_versions" => to_value(self.section_versions(parse(arguments)?)),
            "get_section_version" => to_value(self.get_section_version(parse(arguments)?)),
            "switch_section_version" => to_value(self.switch_section_version(parse(arguments)?)),
            "diff_section_versions" => to_value(self.diff_section_versions(parse(arguments)?)),
            "create_document_snapshot" => {
                to_value(self.create_document_snapshot(parse(arguments)?))
            }
            "document_snapshots" => to_value(self.document_snapshots(parse(arguments)?)),
            "restore_document_snapshot" => {
                to_value(self.restore_document_snapshot(parse(arguments)?))
            }
            "diff_document_snapshots" => to_value(self.diff_document_snapshots(parse(arguments)?)),
            "search_sections" => to_value(self.search_sections(parse(arguments)?)),
            "full_text_search" => to_value(self.full_text_search(parse(arguments)?)),
            #[cfg(feature = "semantic-search")]
            "semantic_search_sections" => to_value(self.semantic_search_sections(parse(arguments)?)),
            "find_by_title" => to_value(self.find_by_title(parse(arguments)?)),
            "find_by_tag" => to_value(self.find_by_tag(parse(arguments)?)),
            "validate_document" => to_value(self.validate_document(parse(arguments)?)),
            "get_workspace" => to_value(self.get_workspace(parse(arguments)?)),
            "set_workspace" => to_value(self.set_workspace(parse(arguments)?)),
            "get_database" => to_value(self.get_database(parse(arguments)?)),
            _ => Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: format!("tool {name:?} is not available in filesystem mode"),
            }),
        }
    }
}

impl VdsMcpSurface for FilesystemVdsServer {
    fn list_documents(&self, _params: ListDocumentsParams) -> McpResult<Vec<DocumentInfo>> {
        Ok(self
            .generation
            .read()
            .unwrap()
            .state
            .documents()
            .map(|document| document_info(document.document.clone()))
            .collect())
    }

    fn get_document(&self, params: GetDocumentParams) -> McpResult<Document> {
        Ok(self.require_document(&params.document_id)?.document)
    }

    fn rename_document(&self, params: RenameDocumentParams) -> McpResult<DocumentInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        self.require_managed_document(&params.document_id)?;
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        repo.rename_document(&params.document_id, &params.name)
            .map_err(metadata_error)?;
        self.reload().map_err(workspace_error)?;
        let doc = self.require_document(&params.document_id)?;
        Ok(document_info(doc.document))
    }

    fn get_document_location(
        &self,
        params: GetDocumentLocationParams,
    ) -> McpResult<DocumentLocation> {
        self.document_location(&self.require_document(&params.document_id)?)
    }

    fn manage_document_file(
        &self,
        params: ManageDocumentFileParams,
    ) -> McpResult<DocumentLocation> {
        let document = self.require_document(&params.document_id)?;
        if !document.managed {
            MetadataRepository::initialize(&self.workspace_root())
                .map_err(metadata_error)?
                .promote(&document)
                .map_err(metadata_error)?;
            self.reload().map_err(workspace_error)?;
        }
        self.document_location(&self.require_document(&params.document_id)?)
    }

    fn create_document(&self, params: CreateDocumentParams) -> McpResult<DocumentInfo> {
        let relative_path = params.relative_path.ok_or_else(|| McpError {
            code: McpErrorCode::InvalidInput,
            message: "`relative_path` is required in filesystem mode".to_owned(),
        })?;
        let initial_markdown = params.initial_content.unwrap_or_default();
        let repo = MetadataRepository::initialize(&self.workspace_root()).map_err(metadata_error)?;
        repo.create_document_file(&relative_path, &initial_markdown, params.title)
            .map_err(metadata_error)?;
        self.reload().map_err(workspace_error)?;
        let doc = self.require_document_by_path(&relative_path)?;
        Ok(document_info(doc.document))
    }

    fn import_document(&self, params: ImportDocumentParams) -> McpResult<DocumentInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        // Treat `path` as workspace-relative; find the unmanaged document.
        let doc = self.require_document_by_path(&params.path)?;
        if doc.managed {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: format!("document at '{}' is already managed", params.path),
            });
        }
        let doc_id = doc.document.id.clone();
        let repo = MetadataRepository::initialize(&self.workspace_root()).map_err(metadata_error)?;
        repo.promote(&doc).map_err(metadata_error)?;
        if !params.name.is_empty() {
            repo.rename_document(&doc_id, &params.name)
                .map_err(metadata_error)?;
        }
        self.reload().map_err(workspace_error)?;
        let refreshed = self.require_document(&doc_id)?;
        Ok(document_info(refreshed.document))
    }

    fn export_document(&self, params: ExportDocumentParams) -> McpResult<ExportResult> {
        let doc = self.require_document(&params.document_id)?;
        let root_id = doc.document.root.clone();
        let sections: BTreeMap<SectionId, Section> = doc
            .sections
            .iter()
            .map(|s| (s.section_id.clone(), s.clone()))
            .collect();
        let markdown = render_sections_to_markdown(&sections, &root_id);

        // Treat `path` as workspace-relative.
        let dest = self.workspace_root().join(path_from_vds(&params.path));
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| McpError {
                code: McpErrorCode::InvalidInput,
                message: format!("failed to create export directory: {e}"),
            })?;
        }
        let bytes = markdown.as_bytes();
        fs::write(&dest, bytes).map_err(|e| McpError {
            code: McpErrorCode::InvalidInput,
            message: format!("failed to write export: {e}"),
        })?;
        Ok(ExportResult {
            document_id: params.document_id,
            path: params.path,
            bytes_written: bytes.len() as u64,
        })
    }

    fn remove_document_file(
        &self,
        params: RemoveDocumentFileParams,
    ) -> McpResult<DocumentFileMutationResult> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let previous_path = document.relative_path.clone();
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        repo.remove_document_file(&params.document_id, &params.expected_content_hash)
            .map_err(metadata_error)?;
        self.reload().map_err(workspace_error)?;
        Ok(DocumentFileMutationResult {
            document_id: params.document_id,
            previous_relative_path: previous_path,
            relative_path: None,
            content_hash: None,
            managed: false,
        })
    }

    fn unmanage_document_file(
        &self,
        params: UnmanageDocumentFileParams,
    ) -> McpResult<DocumentFileMutationResult> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let previous_path = document.relative_path.clone();
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        repo.unmanage_document_file(
            &params.document_id,
            &params.expected_content_hash,
            params.archive_history,
        )
        .map_err(metadata_error)?;
        self.reload().map_err(workspace_error)?;
        // After unmanage, the file still exists but is no longer tracked.
        let doc = self.require_document_by_path(&previous_path).ok();
        let (current_path, content_hash) = if let Some(ref d) = doc {
            let file_path = self.workspace_root().join(path_from_vds(&d.relative_path));
            let hash = fs::read(&file_path)
                .ok()
                .map(|b| format!("sha256:{:x}", sha2::Sha256::digest(&b)));
            (Some(d.relative_path.clone()), hash)
        } else {
            (None, None)
        };
        Ok(DocumentFileMutationResult {
            document_id: params.document_id,
            previous_relative_path: previous_path,
            relative_path: current_path,
            content_hash,
            managed: false,
        })
    }

    fn restore_document_file(
        &self,
        params: RestoreDocumentFileParams,
    ) -> McpResult<DocumentFileMutationResult> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let restored_path = repo
            .restore_document_file(&params.document_id, params.relative_path.as_deref())
            .map_err(metadata_error)?;
        self.reload().map_err(workspace_error)?;
        let doc = self.require_document(&params.document_id)?;
        let file_path = self.workspace_root().join(path_from_vds(&restored_path));
        let content_hash = fs::read(&file_path)
            .ok()
            .map(|b| format!("sha256:{:x}", sha2::Sha256::digest(&b)));
        Ok(DocumentFileMutationResult {
            document_id: params.document_id,
            previous_relative_path: doc.relative_path.clone(),
            relative_path: Some(restored_path),
            content_hash,
            managed: true,
        })
    }

    fn move_document_file(
        &self,
        params: MoveDocumentFileParams,
    ) -> McpResult<DocumentFileMutationResult> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_document(&params.document_id)?;
        if !document.managed {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "document must be managed before it can be moved".to_owned(),
            });
        }
        if !is_markdown_path_eligible(&self.workspace_root(), &params.new_relative_path)
            .map_err(workspace_error)?
        {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: format!(
                    "destination is not an eligible Markdown path: {}",
                    params.new_relative_path
                ),
            });
        }
        let previous_relative_path = document.relative_path.clone();
        let relocated = MetadataRepository::open(&self.workspace_root())
            .map_err(metadata_error)?
            .relocate_document(
                &params.document_id,
                &params.new_relative_path,
                &params.expected_content_hash,
                params.create_parent_directories,
            )
            .map_err(metadata_error)?;
        self.reload().map_err(workspace_error)?;
        Ok(DocumentFileMutationResult {
            document_id: relocated.document_id,
            previous_relative_path,
            relative_path: Some(relocated.relative_path),
            content_hash: Some(relocated.content_hash),
            managed: true,
        })
    }

    fn rename_document_file(
        &self,
        params: RenameDocumentFileParams,
    ) -> McpResult<DocumentFileMutationResult> {
        if params.new_filename.is_empty()
            || params.new_filename.contains('/')
            || params.new_filename.contains('\\')
            || !Path::new(&params.new_filename)
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "new_filename must be a Markdown filename without directories".to_owned(),
            });
        }
        let document = self.require_document(&params.document_id)?;
        let parent = Path::new(&document.relative_path)
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.to_string_lossy().replace('\\', "/"));
        let new_relative_path = parent
            .map(|parent| format!("{parent}/{}", params.new_filename))
            .unwrap_or(params.new_filename);
        self.move_document_file(MoveDocumentFileParams {
            document_id: params.document_id,
            new_relative_path,
            expected_content_hash: params.expected_content_hash,
            create_parent_directories: false,
        })
    }

    fn table_of_contents(
        &self,
        params: TableOfContentsParams,
    ) -> McpResult<Vec<TableOfContentsEntry>> {
        let document = self.require_document(&params.document_id)?;
        let sections = sections_by_id(&document);
        Ok(build_toc(&document.document.root, &sections))
    }

    fn get_section(&self, params: GetSectionParams) -> McpResult<Section> {
        self.require_section(&params.document_id, &params.section_id)
    }

    fn get_section_tree(&self, params: GetSectionTreeParams) -> McpResult<SectionTree> {
        let document = self.require_document(&params.document_id)?;
        let sections = sections_by_id(&document);
        let section = sections
            .get(&params.section_id)
            .cloned()
            .ok_or_else(|| not_found("section", params.section_id.as_str()))?;
        Ok(build_tree(section, params.depth, &sections))
    }

    fn get_sections(&self, params: GetSectionsParams) -> McpResult<Vec<Section>> {
        params
            .section_ids
            .iter()
            .map(|section_id| self.require_section(&params.document_id, section_id))
            .collect()
    }

    fn render_section_markdown(&self, params: RenderSectionMarkdownParams) -> McpResult<String> {
        let document = self.require_document(&params.document_id)?;
        let sections = sections_by_id(&document);
        let section = sections
            .get(&params.section_id)
            .ok_or_else(|| not_found("section", params.section_id.as_str()))?;
        let mut markdown = String::new();
        append_section(&mut markdown, section);
        if params.include_children {
            append_children(&mut markdown, section, &sections);
        }
        Ok(markdown)
    }

    fn render_document_markdown(&self, params: RenderDocumentMarkdownParams) -> McpResult<String> {
        let document = self.require_document(&params.document_id)?;
        let path = self
            .workspace_root()
            .join(path_from_vds(&document.relative_path));
        fs::read_to_string(&path).map_err(|error| McpError {
            code: McpErrorCode::Storage,
            message: format!("{}: {error}", path.display()),
        })
    }

    fn search_sections(&self, params: SearchSectionsParams) -> McpResult<Vec<SectionSearchResult>> {
        let search_options = params.options.unwrap_or_default();
        let max_results = search_options.max_results.unwrap_or(50).max(1) as usize;
        let generation = self.generation.read().unwrap();
        if generation
            .state
            .document_by_id(&params.document_id)
            .is_none()
        {
            return Err(not_found("document", params.document_id.as_str()));
        }
        let options = FullTextSearchOptions {
            document_id: Some(params.document_id.clone()),
            max_results,
            ..FullTextSearchOptions::default()
        };
        Ok(generation
            .full_text
            .search(&params.query, &options)
            .into_iter()
            .filter(|result| {
                (search_options.include_titles && result.title_match)
                    || (search_options.include_content && !result.content_matches.is_empty())
            })
            .filter_map(|result| {
                let document = generation.state.document_by_id(&result.document_id)?;
                let section = document
                    .sections
                    .iter()
                    .find(|section| section.section_id == result.section_id)?;
                Some(SectionSearchResult {
                    section: section_info(section.clone()),
                    score: result.score,
                    title_match: result.title_match,
                    content_matches: result
                        .content_matches
                        .into_iter()
                        .map(|matched| TextMatch {
                            start: matched.start,
                            end: matched.end,
                            snippet: matched.snippet,
                        })
                        .collect(),
                })
            })
            .collect())
    }

    fn full_text_search(
        &self,
        params: FullTextSearchParams,
    ) -> McpResult<Vec<FullTextSearchResult>> {
        let generation = self.generation.read().unwrap();
        if let Some(document_id) = &params.document_id
            && generation.state.document_by_id(document_id).is_none()
        {
            return Err(not_found("document", document_id.as_str()));
        }
        let options = FullTextSearchOptions {
            document_id: params.document_id,
            path_prefix: params.path_prefix,
            require_all_terms: params.require_all_terms,
            max_results: params.max_results.unwrap_or(50).max(1) as usize,
        };
        Ok(generation
            .full_text
            .search(&params.query, &options)
            .into_iter()
            .filter_map(|result| {
                let document = generation.state.document_by_id(&result.document_id)?;
                let section = document
                    .sections
                    .iter()
                    .find(|section| section.section_id == result.section_id)?;
                Some(FullTextSearchResult {
                    document_id: result.document_id,
                    relative_path: result.relative_path,
                    section: section_info(section.clone()),
                    heading_ancestry: result.heading_ancestry,
                    score: result.score,
                    title_match: result.title_match,
                    content_matches: result
                        .content_matches
                        .into_iter()
                        .map(|matched| TextMatch {
                            start: matched.start,
                            end: matched.end,
                            snippet: matched.snippet,
                        })
                        .collect(),
                })
            })
            .collect())
    }

    #[cfg(feature = "semantic-search")]
    fn semantic_search_sections(
        &self,
        params: crate::mcp::SemanticSearchSectionsParams,
    ) -> McpResult<Vec<SectionSearchResult>> {
        use crate::semantic::SemanticSearchOptions;

        // Extract or validate query embedding
        let query_embedding = if let Some(embedding) = params.query_embedding {
            embedding
        } else {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "semantic_search requires query_embedding parameter. VDS 2.0 does not perform embedding generation - please provide precomputed embeddings.".to_owned(),
            });
        };

        if query_embedding.vector.is_empty() {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "query_embedding.vector must not be empty".to_owned(),
            });
        }

        let generation = self.generation.read().unwrap();

        // Validate document_id if provided
        if generation.state.document_by_id(&params.document_id).is_none() {
            return Err(not_found("document", params.document_id.as_str()));
        }

        let mcp_options = params.options.unwrap_or_default();
        let options = SemanticSearchOptions {
            document_id: Some(params.document_id),
            path_prefix: None,
            require_same_model: mcp_options.require_same_model,
            max_results: mcp_options.max_results.unwrap_or(10).max(1) as usize,
            m: mcp_options.m.map(|v| v as usize),
            ef_construction: mcp_options.ef_construction.map(|v| v as usize),
            ef: mcp_options.ef.map(|v| v as usize),
        };

        Ok(generation
            .semantic
            .search(&query_embedding, &options)
            .into_iter()
            .filter_map(|result| {
                let document = generation.state.document_by_id(&result.document_id)?;
                let section = document
                    .sections
                    .iter()
                    .find(|section| section.section_id == result.section_id)?;
                Some(SectionSearchResult {
                    section: section_info(section.clone()),
                    score: result.score,
                    title_match: false, // Semantic search doesn't have explicit title matching
                    content_matches: vec![], // No specific match locations in semantic search
                })
            })
            .collect())
    }

    fn find_by_title(&self, params: FindByTitleParams) -> McpResult<Vec<SectionSearchResult>> {
        let query = params.title.to_lowercase();
        Ok(self
            .require_document(&params.document_id)?
            .sections
            .into_iter()
            .filter(|section| {
                let title = section.title.to_lowercase();
                if params.fuzzy {
                    title.contains(&query) || query.contains(&title)
                } else {
                    title == query
                }
            })
            .map(|section| SectionSearchResult {
                section: section_info(section),
                score: 1.0,
                title_match: true,
                content_matches: Vec::new(),
            })
            .collect())
    }

    fn find_by_tag(&self, params: FindByTagParams) -> McpResult<Vec<SectionInfo>> {
        Ok(self
            .require_document(&params.document_id)?
            .sections
            .into_iter()
            .filter(|section| section.metadata.tags.iter().any(|tag| tag == &params.tag))
            .map(section_info)
            .collect())
    }

    fn validate_document(
        &self,
        params: ValidateDocumentParams,
    ) -> McpResult<Vec<ValidationDiagnostic>> {
        let document = self.require_document(&params.document_id)?;
        let mut diagnostics: Vec<ValidationDiagnostic> = Vec::new();

        // Check 1: content hash
        if document.managed && !document.source_matches_metadata {
            diagnostics.push(ValidationDiagnostic {
                severity: DiagnosticSeverity::Warning,
                section_id: None,
                message: "Markdown content differs from the hash recorded in .vds metadata"
                    .to_owned(),
            });
        }

        if !document.managed {
            return Ok(diagnostics);
        }

        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;

        // Check 2: each section's current_version has a version file on disk.
        for section in &document.sections {
            let available = repo
                .list_section_versions(&document.document.id, &section.section_id)
                .map_err(metadata_error)?;
            if !available.contains(&section.current_version) {
                diagnostics.push(ValidationDiagnostic {
                    severity: DiagnosticSeverity::Error,
                    section_id: Some(section.section_id.clone()),
                    message: format!(
                        "current version {} is not present in the versions directory",
                        section.current_version.as_str()
                    ),
                });
            }
        }

        // Check 3: each snapshot's root_version file exists for the root section.
        let root_section_id = &document.document.root;
        let root_version_ids = repo
            .list_section_versions(&document.document.id, root_section_id)
            .map_err(metadata_error)?;
        let snapshots = repo
            .list_snapshots(&document.document.id)
            .map_err(metadata_error)?;
        for snapshot in &snapshots {
            if !root_version_ids.contains(&snapshot.root_version) {
                diagnostics.push(ValidationDiagnostic {
                    severity: DiagnosticSeverity::Warning,
                    section_id: None,
                    message: format!(
                        "snapshot {} root_version {} not found in root section versions",
                        snapshot.snapshot_id.as_str(),
                        snapshot.root_version.as_str()
                    ),
                });
            }
        }

        Ok(diagnostics)
    }

    fn set_workspace(&self, params: SetWorkspaceParams) -> McpResult<WorkspaceInfo> {
        // Hold the mutation lock for the whole switch so no write races with us.
        let _mutation = self.mutation_lock.lock().unwrap();

        let new_root = PathBuf::from(&params.workspace);

        let new_lease = WorkspaceLease::acquire(&new_root).map_err(|e| McpError {
            code: McpErrorCode::InvalidInput,
            message: format!("cannot acquire workspace lease for {:?}: {e}", new_root),
        })?;

        if let Ok(repo) = MetadataRepository::open(&new_root) {
            repo.recover_transactions().map_err(metadata_error)?;
        }

        let new_state = WorkspaceState::load(&new_root).map_err(|e| McpError {
            code: McpErrorCode::InvalidInput,
            message: format!("cannot load workspace at {:?}: {e:?}", new_root),
        })?;
        let canonical_root = new_state.root().to_path_buf();

        // Atomically replace the in-memory generation (same Arc, new contents).
        {
            let mut guard = self.generation.write().unwrap();
            *guard = WorkspaceGeneration::build(new_state, 0);
        }

        let (new_watcher, new_watcher_active) =
            start_watcher(canonical_root.clone(), Arc::clone(&self.generation));

        // Replace switchable state; drop() releases old lease and stops old watcher thread.
        *self.switchable.lock().unwrap() = SwitchableState {
            workspace_root: canonical_root,
            lease: new_lease,
            watcher: new_watcher,
            watcher_active: new_watcher_active,
        };

        Ok(WorkspaceInfo {
            workspace: Some(self.workspace_root().to_string_lossy().into_owned()),
            database: "filesystem".to_owned(),
            watcher_active: self.watcher_active(),
            reload_count: 0,
        })
    }

    fn get_workspace(&self, _params: GetWorkspaceParams) -> McpResult<WorkspaceInfo> {
        let reload_count = self.generation.read().unwrap().reload_count;
        Ok(WorkspaceInfo {
            workspace: Some(self.workspace_root().to_string_lossy().into_owned()),
            database: "filesystem".to_owned(),
            watcher_active: self.watcher_active(),
            reload_count,
        })
    }

    fn get_database(&self, _params: GetDatabaseParams) -> McpResult<DatabaseInfo> {
        Ok(DatabaseInfo {
            database: "filesystem".to_owned(),
        })
    }

    fn update_section(&self, params: UpdateSectionParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        check_version(&section, &params.options)?;
        let markdown = self.read_markdown_file(&document)?;
        let span = document
            .source_spans
            .get(&params.section_id)
            .ok_or_else(|| no_source_span(&params.section_id))?;
        let new_markdown = apply_content_edit(&markdown, span, &params.content)
            .ok_or_else(|| span_out_of_range(&params.section_id))?;
        self.commit_content_mutation(
            &document,
            &section,
            &new_markdown,
            &params.content,
            section.title.clone(),
            "update_section",
            &params.options,
        )
    }

    fn append_to_section(&self, params: AppendToSectionParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        check_version(&section, &params.options)?;
        let markdown = self.read_markdown_file(&document)?;
        let span = document
            .source_spans
            .get(&params.section_id)
            .ok_or_else(|| no_source_span(&params.section_id))?;
        let separator = if section.content.is_empty() { "" } else { "\n\n" };
        let new_content = format!("{}{}{}", section.content.trim_end(), separator, params.content.trim_start());
        let new_markdown = apply_content_edit(&markdown, span, &new_content)
            .ok_or_else(|| span_out_of_range(&params.section_id))?;
        self.commit_content_mutation(
            &document,
            &section,
            &new_markdown,
            &new_content,
            section.title.clone(),
            "append_to_section",
            &params.options,
        )
    }

    fn rename_section(&self, params: RenameSectionParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        check_version(&section, &params.options)?;
        let markdown = self.read_markdown_file(&document)?;
        let span = document
            .source_spans
            .get(&params.section_id)
            .ok_or_else(|| no_source_span(&params.section_id))?;
        let new_markdown = apply_heading_rename(&markdown, span, &params.new_title)
            .ok_or_else(|| span_out_of_range(&params.section_id))?;
        self.commit_content_mutation(
            &document,
            &section,
            &new_markdown,
            &section.content,
            params.new_title,
            "rename_section",
            &params.options,
        )
    }

    fn set_section_metadata(&self, params: SetSectionMetadataParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        check_version(&section, &params.options)?;
        let now = Utc::now();
        let new_version_id = VersionId::new_v7();
        let updated_section_record = SectionRecord {
            format_version: METADATA_FORMAT_VERSION,
            section_id: section.section_id.clone(),
            current_version: new_version_id.clone(),
            metadata: params.metadata.clone(),
            matching: section_matching_record(&section),
            updated_at: now,
        };
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let catalog = repo.load_catalog().map_err(metadata_error)?;
        let managed = catalog.document_by_id(&params.document_id).ok_or_else(|| {
            McpError { code: McpErrorCode::NotFound, message: format!("document not managed") }
        })?;
        let updated_current = CurrentDocumentRecord {
            format_version: METADATA_FORMAT_VERSION,
            content_hash: managed.current.content_hash.clone(),
            current_document_version: managed.current.current_document_version.clone(),
            root_section_id: managed.current.root_section_id.clone(),
            updated_at: now,
        };
        repo.commit_metadata_only_mutation(
            &params.document_id,
            &updated_section_record,
            &updated_current,
        ).map_err(metadata_error)?;
        self.reload().map_err(workspace_error)?;
        let doc = self.require_document(&params.document_id)?;
        let updated = self.require_section_from(&doc, &params.section_id)?;
        Ok(section_info(updated))
    }

    fn create_section(&self, params: CreateSectionParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let mut sections = sections_by_id(&document);
        let root_id = document.document.root.clone();
        let parent_id = params.parent_id.clone().unwrap_or_else(|| root_id.clone());
        let parent = sections.get(&parent_id).cloned().ok_or_else(|| {
            not_found("parent section", parent_id.as_str())
        })?;
        let new_id = SectionId::new_v7();
        let ordinal = compute_insert_ordinal(&parent, &params.position);
        insert_child_at(&mut sections, &parent_id, new_id.clone(), ordinal);
        let parent_level = sections.get(&parent_id).map(|s| s.level).unwrap_or(0);
        let new_level = if parent_id == root_id { 1 } else { (parent_level + 1).min(6) };
        let now = Utc::now();
        let new_version_id = VersionId::new_v7();
        sections.insert(new_id.clone(), Section {
            section_id: new_id.clone(),
            document_id: params.document_id.clone(),
            parent_id: Some(parent_id),
            children: Vec::new(),
            title: params.title.clone(),
            level: new_level,
            content: params.content.trim_end().to_owned(),
            ordinal,
            current_version: new_version_id.clone(),
            metadata: SectionMetadata { anchor: None, tags: Vec::new(), summary: None, locked: false },
            embedding: None,
            created_at: now,
            updated_at: now,
        });
        self.commit_structural(&document, sections, vec![new_id.clone()], "create_section")?;
        let doc = self.require_document(&params.document_id)?;
        let created = self.require_section_from(&doc, &new_id)?;
        Ok(section_info(created))
    }

    fn insert_section_before(&self, params: InsertSectionBeforeParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let sibling = self.require_section_from(&document, &params.sibling_section_id)?;
        let parent_id = sibling.parent_id.clone().ok_or_else(|| McpError {
            code: McpErrorCode::InvalidInput,
            message: "cannot insert before the root section".to_owned(),
        })?;
        let mut sections = sections_by_id(&document);
        let new_id = SectionId::new_v7();
        let ordinal = sibling.ordinal;
        insert_child_at(&mut sections, &parent_id, new_id.clone(), ordinal);
        let now = Utc::now();
        let new_version_id = VersionId::new_v7();
        let parent_level = sections.get(&parent_id).map(|s| s.level).unwrap_or(0);
        let root_id = document.document.root.clone();
        let new_level = if parent_id == root_id { 1 } else { (parent_level + 1).min(6) };
        sections.insert(new_id.clone(), Section {
            section_id: new_id.clone(),
            document_id: params.document_id.clone(),
            parent_id: Some(parent_id),
            children: Vec::new(),
            title: params.title.clone(),
            level: new_level,
            content: params.content.trim_end().to_owned(),
            ordinal,
            current_version: new_version_id.clone(),
            metadata: SectionMetadata { anchor: None, tags: Vec::new(), summary: None, locked: false },
            embedding: None,
            created_at: now,
            updated_at: now,
        });
        self.commit_structural(&document, sections, vec![new_id.clone()], "insert_section_before")?;
        let doc = self.require_document(&params.document_id)?;
        let created = self.require_section_from(&doc, &new_id)?;
        Ok(section_info(created))
    }

    fn insert_section_after(&self, params: InsertSectionAfterParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let sibling = self.require_section_from(&document, &params.sibling_section_id)?;
        let parent_id = sibling.parent_id.clone().ok_or_else(|| McpError {
            code: McpErrorCode::InvalidInput,
            message: "cannot insert after the root section".to_owned(),
        })?;
        let mut sections = sections_by_id(&document);
        let new_id = SectionId::new_v7();
        let ordinal = sibling.ordinal + 1;
        insert_child_at(&mut sections, &parent_id, new_id.clone(), ordinal);
        let now = Utc::now();
        let new_version_id = VersionId::new_v7();
        let parent_level = sections.get(&parent_id).map(|s| s.level).unwrap_or(0);
        let root_id = document.document.root.clone();
        let new_level = if parent_id == root_id { 1 } else { (parent_level + 1).min(6) };
        sections.insert(new_id.clone(), Section {
            section_id: new_id.clone(),
            document_id: params.document_id.clone(),
            parent_id: Some(parent_id),
            children: Vec::new(),
            title: params.title.clone(),
            level: new_level,
            content: params.content.trim_end().to_owned(),
            ordinal,
            current_version: new_version_id.clone(),
            metadata: SectionMetadata { anchor: None, tags: Vec::new(), summary: None, locked: false },
            embedding: None,
            created_at: now,
            updated_at: now,
        });
        self.commit_structural(&document, sections, vec![new_id.clone()], "insert_section_after")?;
        let doc = self.require_document(&params.document_id)?;
        let created = self.require_section_from(&doc, &new_id)?;
        Ok(section_info(created))
    }

    fn remove_section(&self, params: RemoveSectionParams) -> McpResult<RemovedSectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        if section.parent_id.is_none() {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "cannot remove the root section".to_owned(),
            });
        }
        let parent_id = section.parent_id.clone().unwrap();
        let mut sections = sections_by_id(&document);
        let mut removed_children = Vec::new();
        if params.remove_children {
            collect_descendants(&section, &sections, &mut removed_children);
            for child_id in &removed_children {
                sections.remove(child_id);
            }
        } else {
            // Reparent children to the removed section's parent
            let children = section.children.clone();
            for (i, child_id) in children.iter().enumerate() {
                if let Some(child) = sections.get_mut(child_id) {
                    child.parent_id = Some(parent_id.clone());
                    child.ordinal = section.ordinal + i as u32;
                }
            }
            if let Some(parent) = sections.get_mut(&parent_id) {
                let pos = parent.children.iter().position(|id| id == &params.section_id);
                if let Some(pos) = pos {
                    let mut new_children = parent.children[..pos].to_vec();
                    new_children.extend_from_slice(&children);
                    new_children.extend_from_slice(&parent.children[pos + 1..]);
                    parent.children = new_children;
                    renumber_children(&mut sections, &parent_id);
                }
            }
        }
        let children_after_remove = if let Some(parent) = sections.get_mut(&parent_id) {
            parent.children.retain(|id| id != &params.section_id);
            parent.children.clone()
        } else {
            Vec::new()
        };
        renumber_children_vec(&mut sections, &children_after_remove);
        sections.remove(&params.section_id);
        self.commit_structural(&document, sections, vec![], "remove_section")?;
        Ok(RemovedSectionInfo {
            section_id: params.section_id,
            parent_id: Some(parent_id),
            removed_children,
        })
    }

    fn reorder_sections(&self, params: ReorderSectionsParams) -> McpResult<Vec<SectionInfo>> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let root_id = document.document.root.clone();
        let parent_id = params.parent_id.clone().unwrap_or_else(|| root_id.clone());
        let mut sections = sections_by_id(&document);
        let current_parent = sections.get(&parent_id).cloned().ok_or_else(|| {
            not_found("parent section", parent_id.as_str())
        })?;
        let current_set: std::collections::BTreeSet<_> = current_parent.children.iter().cloned().collect();
        let requested_set: std::collections::BTreeSet<_> = params.ordered_children.iter().cloned().collect();
        if current_set != requested_set {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "ordered_children must contain exactly the current children of the parent".to_owned(),
            });
        }
        if let Some(parent) = sections.get_mut(&parent_id) {
            parent.children = params.ordered_children.clone();
        }
        for (i, child_id) in params.ordered_children.iter().enumerate() {
            if let Some(child) = sections.get_mut(child_id) {
                child.ordinal = i as u32;
            }
        }
        self.commit_structural(&document, sections, vec![], "reorder_sections")?;
        let doc = self.require_document(&params.document_id)?;
        let updated_sections = sections_by_id(&doc);
        Ok(params.ordered_children.iter()
            .filter_map(|id| updated_sections.get(id).cloned().map(section_info))
            .collect())
    }

    fn move_section(&self, params: MoveSectionParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        if section.parent_id.is_none() {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "cannot move the root section".to_owned(),
            });
        }
        let root_id = document.document.root.clone();
        let new_parent_id = params.new_parent_id.clone().unwrap_or_else(|| root_id.clone());
        if new_parent_id == params.section_id {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "a section cannot be its own parent".to_owned(),
            });
        }
        let old_parent_id = section.parent_id.clone().unwrap();
        let mut sections = sections_by_id(&document);
        // Remove from old parent
        if let Some(old_parent) = sections.get_mut(&old_parent_id) {
            old_parent.children.retain(|id| id != &params.section_id);
        }
        renumber_children(&mut sections, &old_parent_id);
        // Add to new parent
        let new_parent = sections.get(&new_parent_id).cloned().ok_or_else(|| {
            not_found("new parent section", new_parent_id.as_str())
        })?;
        let ordinal = compute_insert_ordinal(&new_parent, &params.position);
        insert_child_at(&mut sections, &new_parent_id, params.section_id.clone(), ordinal);
        // Update section's parent and level
        let new_parent_level = sections.get(&new_parent_id).map(|s| s.level).unwrap_or(0);
        let new_level = if new_parent_id == root_id { 1 } else { (new_parent_level + 1).min(6) };
        if let Some(sec) = sections.get_mut(&params.section_id) {
            sec.parent_id = Some(new_parent_id.clone());
            sec.level = new_level;
        }
        // Update descendant levels
        update_descendant_levels(&mut sections, &params.section_id, new_level);
        self.commit_structural(&document, sections, vec![], "move_section")?;
        let doc = self.require_document(&params.document_id)?;
        let updated = self.require_section_from(&doc, &params.section_id)?;
        Ok(section_info(updated))
    }

    fn promote_section(&self, params: PromoteSectionParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        if section.level <= 1 || section.parent_id.is_none() {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "section is already at the top level".to_owned(),
            });
        }
        let parent_id = section.parent_id.clone().unwrap();
        let mut sections = sections_by_id(&document);
        let grandparent_id = sections.get(&parent_id)
            .and_then(|p| p.parent_id.clone())
            .ok_or_else(|| McpError {
                code: McpErrorCode::InvalidInput,
                message: "section's parent has no parent to promote into".to_owned(),
            })?;
        let parent_ordinal = sections.get(&parent_id).map(|p| p.ordinal).unwrap_or(0);
        // Remove from parent
        if let Some(parent) = sections.get_mut(&parent_id) {
            parent.children.retain(|id| id != &params.section_id);
        }
        renumber_children(&mut sections, &parent_id);
        // Insert into grandparent after parent
        let ordinal = parent_ordinal + 1;
        insert_child_at(&mut sections, &grandparent_id, params.section_id.clone(), ordinal);
        let new_level = section.level.saturating_sub(1).max(1);
        if let Some(sec) = sections.get_mut(&params.section_id) {
            sec.parent_id = Some(grandparent_id.clone());
            sec.level = new_level;
        }
        update_descendant_levels(&mut sections, &params.section_id, new_level);
        self.commit_structural(&document, sections, vec![], "promote_section")?;
        let doc = self.require_document(&params.document_id)?;
        let updated = self.require_section_from(&doc, &params.section_id)?;
        Ok(section_info(updated))
    }

    fn demote_section(&self, params: DemoteSectionParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        if section.parent_id.is_none() {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "cannot demote the root section".to_owned(),
            });
        }
        if section.level >= 6 {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "section is already at the maximum heading depth".to_owned(),
            });
        }
        let parent_id = section.parent_id.clone().unwrap();
        let mut sections = sections_by_id(&document);
        let parent = sections.get(&parent_id).cloned().ok_or_else(|| {
            not_found("parent section", parent_id.as_str())
        })?;
        // Find the preceding sibling to become the new parent
        let my_ordinal = section.ordinal;
        let preceding_sibling = parent.children.iter()
            .filter_map(|id| sections.get(id))
            .find(|s| s.ordinal + 1 == my_ordinal)
            .map(|s| s.section_id.clone());
        let new_parent_id = preceding_sibling.ok_or_else(|| McpError {
            code: McpErrorCode::InvalidInput,
            message: "no preceding sibling to demote into".to_owned(),
        })?;
        // Remove from current parent
        if let Some(p) = sections.get_mut(&parent_id) {
            p.children.retain(|id| id != &params.section_id);
        }
        renumber_children(&mut sections, &parent_id);
        // Add as last child of preceding sibling
        let new_ordinal = sections.get(&new_parent_id).map(|s| s.children.len() as u32).unwrap_or(0);
        if let Some(new_parent) = sections.get_mut(&new_parent_id) {
            new_parent.children.push(params.section_id.clone());
        }
        let new_level = (section.level + 1).min(6);
        if let Some(sec) = sections.get_mut(&params.section_id) {
            sec.parent_id = Some(new_parent_id.clone());
            sec.ordinal = new_ordinal;
            sec.level = new_level;
        }
        update_descendant_levels(&mut sections, &params.section_id, new_level);
        self.commit_structural(&document, sections, vec![], "demote_section")?;
        let doc = self.require_document(&params.document_id)?;
        let updated = self.require_section_from(&doc, &params.section_id)?;
        Ok(section_info(updated))
    }

    fn patch_section(&self, params: PatchSectionParams) -> McpResult<SectionInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;
        check_version(&section, &params.options)?;

        let mut current_content = section.content.clone();
        let mut current_title = section.title.clone();
        let mut new_metadata: Option<SectionMetadata> = None;
        let mut has_content_change = false;
        let mut has_title_change = false;

        for op in &params.patch.operations {
            match op {
                PatchOp::ReplaceContent { content } => {
                    current_content = content.clone();
                    has_content_change = true;
                }
                PatchOp::AppendContent { content } => {
                    let sep = if current_content.trim_end().is_empty() { "" } else { "\n\n" };
                    current_content = format!("{}{}{}", current_content.trim_end(), sep, content.trim_start());
                    has_content_change = true;
                }
                PatchOp::PrependContent { content } => {
                    let sep = if current_content.trim_start().is_empty() { "" } else { "\n\n" };
                    current_content = format!("{}{}{}", content.trim_end(), sep, current_content.trim_start());
                    has_content_change = true;
                }
                PatchOp::ReplaceRange { start, end, content } => {
                    if *end > current_content.len() || *start > *end {
                        return Err(McpError {
                            code: McpErrorCode::InvalidInput,
                            message: format!("ReplaceRange {start}..{end} is out of bounds for content of length {}", current_content.len()),
                        });
                    }
                    let mut result = String::with_capacity(current_content.len() - (end - start) + content.len());
                    result.push_str(&current_content[..*start]);
                    result.push_str(content);
                    result.push_str(&current_content[*end..]);
                    current_content = result;
                    has_content_change = true;
                }
                PatchOp::Rename { title } => {
                    current_title = title.clone();
                    has_title_change = true;
                }
                PatchOp::SetMetadata { metadata } => {
                    new_metadata = Some(metadata.clone());
                }
            }
        }

        if new_metadata.is_some() && !has_content_change && !has_title_change {
            // Metadata-only path: no Markdown rewrite needed.
            let effective_metadata = new_metadata.unwrap_or_else(|| section.metadata.clone());
            let now = Utc::now();
            let new_version_id = VersionId::new_v7();
            let updated_section_record = SectionRecord {
                format_version: METADATA_FORMAT_VERSION,
                section_id: section.section_id.clone(),
                current_version: new_version_id.clone(),
                metadata: effective_metadata.clone(),
                matching: section_matching_record(&section),
                updated_at: now,
            };
            let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
            let catalog = repo.load_catalog().map_err(metadata_error)?;
            let managed = catalog.document_by_id(&params.document_id).ok_or_else(|| {
                not_found("managed document", params.document_id.as_str())
            })?;
            let updated_current = CurrentDocumentRecord {
                format_version: METADATA_FORMAT_VERSION,
                content_hash: managed.current.content_hash.clone(),
                current_document_version: managed.current.current_document_version.clone(),
                root_section_id: managed.current.root_section_id.clone(),
                updated_at: now,
            };
            repo.commit_metadata_only_mutation(
                &params.document_id,
                &updated_section_record,
                &updated_current,
            ).map_err(metadata_error)?;
            self.reload().map_err(workspace_error)?;
            let doc = self.require_document(&params.document_id)?;
            let updated = self.require_section_from(&doc, &params.section_id)?;
            return Ok(section_info(updated));
        }

        // Apply Markdown content/title changes surgically.
        let effective_metadata = new_metadata.unwrap_or_else(|| section.metadata.clone());
        let markdown = self.read_markdown_file(&document)?;
        let span = document
            .source_spans
            .get(&params.section_id)
            .ok_or_else(|| no_source_span(&params.section_id))?;

        let new_markdown = if has_title_change && has_content_change {
            // Rename the heading first, then apply the content edit.
            // `apply_heading_rename` only changes the heading line; everything after
            // `content_start` is unchanged, so we can adjust the span by the byte delta.
            let after_rename = apply_heading_rename(&markdown, span, &current_title)
                .ok_or_else(|| span_out_of_range(&params.section_id))?;
            let old_heading_len = span.content_start - span.heading_start;
            let new_heading_len = after_rename.len() + old_heading_len - markdown.len();
            let delta = new_heading_len as isize - old_heading_len as isize;
            let adjusted_span = crate::markdown::SectionSourceSpan {
                heading_start: span.heading_start,
                content_start: (span.content_start as isize + delta) as usize,
                content_end: (span.content_end as isize + delta) as usize,
            };
            apply_content_edit(&after_rename, &adjusted_span, &current_content)
                .ok_or_else(|| span_out_of_range(&params.section_id))?
        } else if has_title_change {
            apply_heading_rename(&markdown, span, &current_title)
                .ok_or_else(|| span_out_of_range(&params.section_id))?
        } else {
            apply_content_edit(&markdown, span, &current_content)
                .ok_or_else(|| span_out_of_range(&params.section_id))?
        };

        let now = Utc::now();
        let new_version_id = VersionId::new_v7();
        let new_section_version = SectionVersion {
            version_id: new_version_id.clone(),
            section_id: section.section_id.clone(),
            title: current_title.clone(),
            content: current_content.trim_end().to_owned(),
            metadata: effective_metadata.clone(),
            embedding: None,
            created_at: now,
            author: params.options.as_ref().and_then(|o| o.author.clone()),
            change_summary: params.options.as_ref().and_then(|o| o.change_summary.clone())
                .or_else(|| Some("patch_section".to_owned())),
        };
        let sections_by_id_map = document.sections.iter()
            .map(|s| (s.section_id.clone(), s))
            .collect::<BTreeMap<_, _>>();
        let ancestry = compute_ancestry(&section.section_id, &sections_by_id_map);
        let updated_section_record = SectionRecord {
            format_version: METADATA_FORMAT_VERSION,
            section_id: section.section_id.clone(),
            current_version: new_version_id,
            metadata: effective_metadata,
            matching: SectionMatchingRecord {
                embedded_marker: None,
                last_known_title: current_title.clone(),
                last_known_ancestry: ancestry,
                last_known_ordinal: section.ordinal,
                content_fingerprint: sha256_str(current_content.as_bytes()),
            },
            updated_at: now,
        };
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let catalog = repo.load_catalog().map_err(metadata_error)?;
        let managed = catalog
            .document_by_id(&document.document.id)
            .ok_or_else(|| not_found("managed document", document.document.id.as_str()))?;
        repo.commit_section_mutation(
            &document.document.id,
            &section.section_id,
            &managed.current.content_hash,
            &new_markdown,
            "patch_section",
            &new_section_version,
            &updated_section_record,
        ).map_err(metadata_error)?;
        self.reload().map_err(workspace_error)?;
        let doc = self.require_document(&params.document_id)?;
        let updated = self.require_section_from(&doc, &params.section_id)?;
        Ok(section_info(updated))
    }

    fn split_section(&self, params: SplitSectionParams) -> McpResult<SplitSectionResult> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let section = self.require_section_from(&document, &params.section_id)?;

        if params.split_at > section.content.len() {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: format!(
                    "split_at {} is out of bounds for content of length {}",
                    params.split_at,
                    section.content.len()
                ),
            });
        }

        let first_content = section.content[..params.split_at].trim_end().to_owned();
        let second_content = section.content[params.split_at..].trim_start().to_owned();

        let new_section_id = SectionId::new_v7();
        let new_version_id = VersionId::new_v7();
        let now = Utc::now();

        // Build the updated section tree.
        let mut sections = sections_by_id(&document);

        // Update the original section content.
        if let Some(sec) = sections.get_mut(&params.section_id) {
            sec.content = first_content.clone();
        }

        // Insert the new section immediately after the original in the parent's children list.
        let parent_id = section.parent_id.clone();
        let insert_ordinal = section.ordinal + 1;
        let new_section = Section {
            section_id: new_section_id.clone(),
            document_id: params.document_id.clone(),
            parent_id: parent_id.clone(),
            level: section.level,
            ordinal: insert_ordinal,
            title: params.new_title.clone(),
            content: second_content.clone(),
            metadata: SectionMetadata { anchor: None, tags: Vec::new(), summary: None, locked: false },
            children: vec![],
            current_version: new_version_id.clone(),
            embedding: None,
            created_at: now,
            updated_at: now,
        };
        sections.insert(new_section_id.clone(), new_section);

        // Insert new section into parent's children list and renumber.
        let effective_parent = parent_id.as_ref().unwrap_or(&document.document.root);
        insert_child_at(&mut sections, effective_parent, new_section_id.clone(), insert_ordinal);
        renumber_children(&mut sections, effective_parent);

        self.commit_structural(
            &document,
            sections,
            vec![new_section_id.clone()],
            "split_section",
        )?;

        let doc = self.require_document(&params.document_id)?;
        let original = self.require_section_from(&doc, &params.section_id)?;
        let created = self.require_section_from(&doc, &new_section_id)?;

        Ok(SplitSectionResult {
            original: section_info(original),
            created: section_info(created),
        })
    }

    fn section_versions(
        &self,
        params: SectionVersionsParams,
    ) -> McpResult<Vec<SectionVersionInfo>> {
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let ids = repo
            .list_section_versions(&params.document_id, &params.section_id)
            .map_err(metadata_error)?;
        let mut infos = Vec::with_capacity(ids.len());
        for version_id in ids {
            let v = repo
                .read_section_version(&params.document_id, &params.section_id, &version_id)
                .map_err(metadata_error)?;
            infos.push(SectionVersionInfo {
                version_id: v.version_id,
                section_id: v.section_id,
                created_at: v.created_at,
                author: v.author,
                change_summary: v.change_summary,
            });
        }
        Ok(infos)
    }

    fn get_section_version(
        &self,
        params: GetSectionVersionParams,
    ) -> McpResult<SectionVersion> {
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        repo.read_section_version(&params.document_id, &params.section_id, &params.version_id)
            .map_err(metadata_error)
    }

    fn switch_section_version(
        &self,
        params: SwitchSectionVersionParams,
    ) -> McpResult<SectionInfo> {
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let historical = repo
            .read_section_version(&params.document_id, &params.section_id, &params.version_id)
            .map_err(metadata_error)?;
        drop(repo);

        // Restore title + content via patch_section so both ops commit atomically.
        let patch_params = PatchSectionParams {
            document_id: params.document_id.clone(),
            section_id: params.section_id.clone(),
            patch: SectionPatch {
                operations: vec![
                    PatchOp::Rename { title: historical.title },
                    PatchOp::ReplaceContent { content: historical.content },
                ],
            },
            options: params.options,
        };
        self.patch_section(patch_params)
    }

    fn diff_section_versions(&self, params: DiffSectionVersionsParams) -> McpResult<DiffResult> {
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let from_v = repo
            .read_section_version(&params.document_id, &params.section_id, &params.from_version)
            .map_err(metadata_error)?;
        let to_v = repo
            .read_section_version(&params.document_id, &params.section_id, &params.to_version)
            .map_err(metadata_error)?;
        Ok(compute_diff(
            params.from_version.as_str(),
            &from_v.content,
            params.to_version.as_str(),
            &to_v.content,
        ))
    }

    fn create_document_snapshot(
        &self,
        params: CreateDocumentSnapshotParams,
    ) -> McpResult<DocumentSnapshot> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let root_version = document
            .sections
            .iter()
            .find(|s| s.section_id == document.document.root)
            .map(|s| s.current_version.clone())
            .ok_or_else(|| not_found("root section", document.document.root.as_str()))?;
        let sections = document.sections.clone();
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        repo.create_snapshot(
            &params.document_id,
            sections,
            root_version,
            params.label,
            params.change_summary,
        )
        .map_err(metadata_error)
    }

    fn document_snapshots(
        &self,
        params: DocumentSnapshotsParams,
    ) -> McpResult<Vec<DocumentSnapshotInfo>> {
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let snapshots = repo
            .list_snapshots(&params.document_id)
            .map_err(metadata_error)?;
        Ok(snapshots
            .into_iter()
            .map(|s| DocumentSnapshotInfo {
                snapshot_id: s.snapshot_id,
                document_id: s.document_id,
                label: s.label,
                created_at: s.created_at,
                author: s.author,
                change_summary: s.change_summary,
            })
            .collect())
    }

    fn restore_document_snapshot(
        &self,
        params: RestoreDocumentSnapshotParams,
    ) -> McpResult<DocumentInfo> {
        let _mutation = self.mutation_lock.lock().unwrap();
        let document = self.require_managed_document(&params.document_id)?;
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let snapshot = repo
            .read_snapshot(&params.document_id, &params.snapshot_id)
            .map_err(metadata_error)?;

        // Build a BTreeMap from the snapshot's section list and re-render.
        let sections: BTreeMap<SectionId, Section> = snapshot
            .sections
            .into_iter()
            .map(|s| (s.section_id.clone(), s))
            .collect();
        let new_section_ids: Vec<SectionId> = sections.keys().cloned().collect();
        self.commit_structural(&document, sections, new_section_ids, "restore_document_snapshot")?;
        self.reload().map_err(workspace_error)?;
        let doc = self.require_document(&params.document_id)?;
        Ok(document_info(doc.document))
    }

    fn diff_document_snapshots(
        &self,
        params: DiffDocumentSnapshotsParams,
    ) -> McpResult<DiffResult> {
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let from_s = repo
            .read_snapshot(&params.document_id, &params.from_snapshot)
            .map_err(metadata_error)?;
        let to_s = repo
            .read_snapshot(&params.document_id, &params.to_snapshot)
            .map_err(metadata_error)?;

        // Produce a diff of rendered Markdown for the two snapshots.
        let from_sections: BTreeMap<SectionId, Section> =
            from_s.sections.into_iter().map(|s| (s.section_id.clone(), s)).collect();
        let to_sections: BTreeMap<SectionId, Section> =
            to_s.sections.into_iter().map(|s| (s.section_id.clone(), s)).collect();
        // Use the document's current root as anchor (both snapshots share the same root ID).
        let doc = self.require_document(&params.document_id)?;
        let root_id = doc.document.root.clone();
        let from_md = render_sections_to_markdown(&from_sections, &root_id);
        let to_md = render_sections_to_markdown(&to_sections, &root_id);
        Ok(compute_diff(
            params.from_snapshot.as_str(),
            &from_md,
            params.to_snapshot.as_str(),
            &to_md,
        ))
    }
}

impl FilesystemVdsServer {
    fn document_location(&self, document: &MaterializedDocument) -> McpResult<DocumentLocation> {
        let path = self
            .workspace_root()
            .join(path_from_vds(&document.relative_path));
        let contents = fs::read(&path).map_err(|error| McpError {
            code: McpErrorCode::Storage,
            message: format!("{}: {error}", path.display()),
        })?;
        let content_hash = format!("sha256:{:x}", Sha256::digest(contents));
        let relative = Path::new(&document.relative_path);
        let filename = relative
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| McpError {
                code: McpErrorCode::Internal,
                message: format!(
                    "document has invalid relative path: {}",
                    document.relative_path
                ),
            })?
            .to_owned();
        let folder = relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let source_matches_metadata = if document.managed {
            let catalog = MetadataRepository::open(&self.workspace_root())
                .map_err(metadata_error)?
                .load_catalog()
                .map_err(metadata_error)?;
            catalog
                .document_by_id(&document.document.id)
                .is_some_and(|managed| managed.current.content_hash == content_hash)
        } else {
            true
        };
        Ok(DocumentLocation {
            document_id: document.document.id.clone(),
            relative_path: document.relative_path.clone(),
            folder,
            filename,
            managed: document.managed,
            source_matches_metadata,
            content_hash,
        })
    }

    fn require_managed_document(&self, document_id: &DocumentId) -> McpResult<MaterializedDocument> {
        let doc = self.require_document(document_id)?;
        if !doc.managed {
            return Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: "document must be managed before mutations; call manage_document_file first".to_owned(),
            });
        }
        Ok(doc)
    }

    fn require_section_from(&self, document: &MaterializedDocument, section_id: &SectionId) -> McpResult<Section> {
        document
            .sections
            .iter()
            .find(|s| &s.section_id == section_id)
            .cloned()
            .ok_or_else(|| not_found("section", section_id.as_str()))
    }

    fn read_markdown_file(&self, document: &MaterializedDocument) -> McpResult<String> {
        let path = self.workspace_root().join(path_from_vds(&document.relative_path));
        fs::read_to_string(&path).map_err(|error| McpError {
            code: McpErrorCode::Storage,
            message: format!("{}: {error}", path.display()),
        })
    }

    fn commit_content_mutation(
        &self,
        document: &MaterializedDocument,
        section: &Section,
        new_markdown: &str,
        new_content: &str,
        new_title: String,
        mutation_kind: &str,
        options: &Option<EditOptions>,
    ) -> McpResult<SectionInfo> {
        let now = Utc::now();
        let new_version_id = VersionId::new_v7();
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let catalog = repo.load_catalog().map_err(metadata_error)?;
        let managed = catalog
            .document_by_id(&document.document.id)
            .ok_or_else(|| not_found("managed document", document.document.id.as_str()))?;
        let current_hash = managed.current.content_hash.clone();

        let new_section_version = SectionVersion {
            version_id: new_version_id.clone(),
            section_id: section.section_id.clone(),
            title: new_title.clone(),
            content: new_content.trim_end().to_owned(),
            metadata: section.metadata.clone(),
            embedding: None,
            created_at: now,
            author: options.as_ref().and_then(|o| o.author.clone()),
            change_summary: options.as_ref().and_then(|o| o.change_summary.clone())
                .or_else(|| Some(mutation_kind.to_owned())),
        };
        let sections_by_id = document.sections.iter()
            .map(|s| (s.section_id.clone(), s))
            .collect::<BTreeMap<_, _>>();
        let ancestry = compute_ancestry(&section.section_id, &sections_by_id);
        let updated_section_record = SectionRecord {
            format_version: METADATA_FORMAT_VERSION,
            section_id: section.section_id.clone(),
            current_version: new_version_id,
            metadata: section.metadata.clone(),
            matching: SectionMatchingRecord {
                embedded_marker: None,
                last_known_title: new_title,
                last_known_ancestry: ancestry,
                last_known_ordinal: section.ordinal,
                content_fingerprint: sha256_str(new_content.as_bytes()),
            },
            updated_at: now,
        };
        repo.commit_section_mutation(
            &document.document.id,
            &section.section_id,
            &current_hash,
            new_markdown,
            mutation_kind,
            &new_section_version,
            &updated_section_record,
        ).map_err(metadata_error)?;
        self.reload_incremental(&document.document.id).map_err(workspace_error)?;
        let doc = self.require_document(&document.document.id)?;
        let updated = self.require_section_from(&doc, &section.section_id)?;
        Ok(section_info(updated))
    }

    fn commit_structural(
        &self,
        document: &MaterializedDocument,
        sections: BTreeMap<SectionId, Section>,
        new_section_ids: Vec<SectionId>,
        mutation_kind: &str,
    ) -> McpResult<()> {
        let repo = MetadataRepository::open(&self.workspace_root()).map_err(metadata_error)?;
        let catalog = repo.load_catalog().map_err(metadata_error)?;
        let managed = catalog
            .document_by_id(&document.document.id)
            .ok_or_else(|| not_found("managed document", document.document.id.as_str()))?;
        let current_hash = managed.current.content_hash.clone();
        let root_id = document.document.root.clone();
        let original_markdown = self.read_markdown_file(document)?;
        let use_crlf = detect_crlf(&original_markdown);
        let new_markdown = apply_line_endings(render_sections_to_markdown(&sections, &root_id), use_crlf);
        let now = Utc::now();
        let new_doc_version = crate::document::VersionId::new_v7();

        let mut new_versions: Vec<SectionVersion> = Vec::new();
        let mut updated_records: Vec<SectionRecord> = Vec::new();

        let sections_ref: BTreeMap<_, &Section> = sections.iter()
            .map(|(id, s)| (id.clone(), s))
            .collect();

        for (id, section) in &sections {
            let is_new = new_section_ids.contains(id);
            let original = document.sections.iter().find(|s| &s.section_id == id);
            let changed = is_new || original.is_none()
                || original.is_some_and(|o| {
                    o.ordinal != section.ordinal
                        || o.parent_id != section.parent_id
                        || o.level != section.level
                        || o.title != section.title
                        || o.content != section.content
                });

            if changed {
                let new_version_id = if is_new {
                    section.current_version.clone()
                } else {
                    VersionId::new_v7()
                };
                new_versions.push(SectionVersion {
                    version_id: new_version_id.clone(),
                    section_id: id.clone(),
                    title: section.title.clone(),
                    content: section.content.clone(),
                    metadata: section.metadata.clone(),
                    embedding: None,
                    created_at: now,
                    author: None,
                    change_summary: Some(mutation_kind.to_owned()),
                });
                let ancestry = compute_ancestry(id, &sections_ref);
                updated_records.push(SectionRecord {
                    format_version: METADATA_FORMAT_VERSION,
                    section_id: id.clone(),
                    current_version: new_version_id,
                    metadata: section.metadata.clone(),
                    matching: SectionMatchingRecord {
                        embedded_marker: None,
                        last_known_title: section.title.clone(),
                        last_known_ancestry: ancestry,
                        last_known_ordinal: section.ordinal,
                        content_fingerprint: sha256_str(section.content.as_bytes()),
                    },
                    updated_at: now,
                });
            }
        }

        let updated_current = CurrentDocumentRecord {
            format_version: METADATA_FORMAT_VERSION,
            content_hash: String::new(), // filled by commit
            current_document_version: new_doc_version,
            root_section_id: root_id,
            updated_at: now,
        };
        repo.commit_structural_mutation(
            &document.document.id,
            &current_hash,
            &new_markdown,
            &new_versions,
            &updated_records,
            &updated_current,
        ).map_err(metadata_error)?;
        self.reload_incremental(&document.document.id).map_err(workspace_error)
    }
}

impl ServerHandler for FilesystemVdsServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                    .with_title("Versioned Document Service 2")
                    .with_description(
                        "Filesystem-authoritative VDS 2: discovers project Markdown, materializes a full section tree in memory, and provides durable filesystem-backed section mutations with full crash recovery.",
                    ),
            )
            .with_instructions(
                "Start with list_documents or manage_document_file, use table_of_contents to discover stable section IDs, read targeted sections before editing, pass expected_version in EditOptions for writes, and use create_document_snapshot before broad structural changes.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, RmcpError> {
        Ok(ListToolsResult {
            meta: None,
            next_cursor: None,
            tools: tool_documentation()
                .into_iter()
                .filter(|documentation| AVAILABLE_TOOL_NAMES.contains(&documentation.name))
                .map(tool_from_doc)
                .collect(),
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, RmcpError> {
        let value = self
            .call(&request.name, request.arguments)
            .map_err(to_rmcp_error)?;
        Ok(CallToolResult::structured(value))
    }
}

/// Returns `true` for events that should trigger a workspace reload.
/// Filters out VDS-internal transaction writes, editor temp files, and OS noise.
fn is_relevant_notify_event(event: &notify::Event, workspace_root: &Path) -> bool {
    use notify::EventKind;
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    let recovery_root = workspace_root.join(".vds").join("recovery");
    for path in &event.paths {
        if path.starts_with(&recovery_root) {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with('~')
                || name.starts_with(".~")
                || name.ends_with(".tmp")
                || name.ends_with(".TMP")
                || name.ends_with(".swp")
                || name.ends_with(".swx")
                || name == ".DS_Store"
                || name == "desktop.ini"
                || name == "Thumbs.db"
            {
                continue;
            }
        }
        return true;
    }
    false
}

/// Starts a filesystem watcher that debounces events and rebuilds the workspace
/// generation whenever relevant files change outside of VDS mutations.
///
/// Returns `(Some(watcher), true)` on success, `(None, false)` if watching is
/// unavailable on this filesystem or platform.
fn start_watcher(
    workspace_root: PathBuf,
    generation: Arc<RwLock<WorkspaceGeneration>>,
) -> (Option<notify::RecommendedWatcher>, bool) {
    use notify::Watcher;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let (tx, rx) = mpsc::channel::<()>();

    // Background thread: drains the signal channel with a debounce window,
    // then rebuilds the workspace generation.
    let workspace_root_reload = workspace_root.clone();
    let generation_weak = Arc::downgrade(&generation);
    let _ = std::thread::Builder::new()
        .name("vds-watcher-reload".to_owned())
        .spawn(move || loop {
            // Block until the first event signal arrives.
            if rx.recv().is_err() {
                break;
            }
            // Drain additional signals within the debounce window.
            let deadline = Instant::now() + Duration::from_millis(300);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(()) => {}
                    Err(_) => break,
                }
            }
            // Upgrade the weak reference; exit if the server was dropped.
            let Some(arc) = generation_weak.upgrade() else {
                break;
            };
            if let Ok(state) = WorkspaceState::load(&workspace_root_reload) {
                let mut lock = arc.write().unwrap();
                let reload_count = lock.reload_count + 1;
                *lock = WorkspaceGeneration::build(state, reload_count);
            }
        });

    let workspace_root_filter = workspace_root.clone();
    let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result {
            if is_relevant_notify_event(&event, &workspace_root_filter) {
                let _ = tx.send(());
            }
        }
    });

    match watcher {
        Ok(mut w) => {
            // Watch the workspace root non-recursively (catches top-level .md files and
            // config), then recursively watch every top-level subdirectory that is NOT
            // `.vds/`.  This avoids holding handles into `.vds/documents/` which would
            // prevent directory renames on Windows.
            let started = w
                .watch(&workspace_root, notify::RecursiveMode::NonRecursive)
                .is_ok();
            if let Ok(entries) = std::fs::read_dir(&workspace_root) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let name = entry.file_name();
                        if name == ".vds" || name == ".git" {
                            continue;
                        }
                        let _ = w.watch(&entry.path(), notify::RecursiveMode::Recursive);
                    }
                }
            }
            if started {
                (Some(w), true)
            } else {
                (None, false)
            }
        }
        Err(_) => (None, false),
    }
}

/// Starts the filesystem-authoritative VDS 2 server over MCP stdio.
pub async fn serve_filesystem_stdio(
    workspace_root: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = FilesystemVdsServer::open(workspace_root)?
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

/// Starts the filesystem-authoritative VDS 2 server over streamable HTTP.
pub async fn serve_filesystem_http(
    workspace_root: PathBuf,
    bind: String,
    path: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = format!("http://{bind}{path}");
    eprintln!(
        "vds-mcp v{} starting in streamable HTTP mode (filesystem-authoritative)",
        env!("CARGO_PKG_VERSION")
    );
    eprintln!("Workspace: {}", workspace_root.display());
    eprintln!("Endpoint: {endpoint}");
    let workspace_root = Arc::new(workspace_root);
    let service: rmcp::transport::streamable_http_server::tower::StreamableHttpService<
        FilesystemVdsServer,
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
    > = rmcp::transport::streamable_http_server::tower::StreamableHttpService::new(
        move || {
            FilesystemVdsServer::open((*workspace_root).clone())
                .map_err(|error| std::io::Error::other(error.to_string()))
        },
        Default::default(),
        rmcp::transport::StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service(&path, service);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    eprintln!("VDS 2 MCP streamable HTTP server listening on {endpoint}");
    axum::serve(listener, router).await?;
    Ok(())
}

fn sections_by_id(document: &MaterializedDocument) -> BTreeMap<SectionId, Section> {
    document
        .sections
        .iter()
        .map(|section| (section.section_id.clone(), section.clone()))
        .collect()
}

fn build_toc(
    parent_id: &SectionId,
    sections: &BTreeMap<SectionId, Section>,
) -> Vec<TableOfContentsEntry> {
    let Some(parent) = sections.get(parent_id) else {
        return Vec::new();
    };
    parent
        .children
        .iter()
        .filter_map(|child_id| sections.get(child_id))
        .map(|section| TableOfContentsEntry {
            section_id: section.section_id.clone(),
            title: section.title.clone(),
            level: section.level,
            ordinal: section.ordinal,
            children: build_toc(&section.section_id, sections),
        })
        .collect()
}

fn build_tree(
    section: Section,
    depth: Option<u32>,
    sections: &BTreeMap<SectionId, Section>,
) -> SectionTree {
    let children = if depth == Some(0) {
        Vec::new()
    } else {
        let next_depth = depth.map(|value| value.saturating_sub(1));
        section
            .children
            .iter()
            .filter_map(|child_id| sections.get(child_id).cloned())
            .map(|child| build_tree(child, next_depth, sections))
            .collect()
    };
    SectionTree { section, children }
}

fn append_children(
    markdown: &mut String,
    parent: &Section,
    sections: &BTreeMap<SectionId, Section>,
) {
    for child_id in &parent.children {
        if let Some(child) = sections.get(child_id) {
            append_section(markdown, child);
            append_children(markdown, child, sections);
        }
    }
}

fn append_section(markdown: &mut String, section: &Section) {
    if !markdown.is_empty() && !markdown.ends_with("\n\n") {
        if !markdown.ends_with('\n') {
            markdown.push('\n');
        }
        markdown.push('\n');
    }
    if section.level > 0 {
        markdown.push_str(&"#".repeat(section.level.clamp(1, 6) as usize));
        markdown.push(' ');
        markdown.push_str(&section.title);
        markdown.push_str("\n\n");
    }
    let content = section.content.trim_end();
    if !content.is_empty() {
        markdown.push_str(content);
        markdown.push('\n');
    }
}

fn document_info(document: Document) -> DocumentInfo {
    DocumentInfo {
        id: document.id,
        name: document.name,
        title: document.metadata.title,
        root: document.root,
        current_version: document.current_version,
        updated_at: document.updated_at,
    }
}

fn section_info(section: Section) -> SectionInfo {
    SectionInfo {
        section_id: section.section_id,
        parent_id: section.parent_id,
        title: section.title,
        level: section.level,
        ordinal: section.ordinal,
        current_version: section.current_version,
        child_count: section.children.len(),
        updated_at: section.updated_at,
    }
}

fn path_from_vds(relative_path: &str) -> PathBuf {
    relative_path.split('/').collect()
}

fn not_found(kind: &str, id: &str) -> McpError {
    McpError {
        code: McpErrorCode::NotFound,
        message: format!("{kind} not found: {id}"),
    }
}

/// Produces a structured line-level diff between `from_text` and `to_text`.
fn compute_diff(from_label: &str, from_text: &str, to_label: &str, to_text: &str) -> DiffResult {
    let from_lines: Vec<&str> = from_text.lines().collect();
    let to_lines: Vec<&str> = to_text.lines().collect();

    // Simple LCS-based diff using Myers-style line diffing.
    let mut hunks = Vec::new();
    let changes = line_diff(&from_lines, &to_lines);

    // Group contiguous change blocks into hunks with up to 3 lines of context.
    const CONTEXT: usize = 3;
    let mut i = 0;
    while i < changes.len() {
        if matches!(changes[i], LineChange::Same(_)) {
            i += 1;
            continue;
        }
        // Found a changed region; determine hunk bounds.
        let hunk_start = i.saturating_sub(CONTEXT);
        let mut j = i;
        while j < changes.len() && !matches!(changes[j], LineChange::Same(_)) {
            j += 1;
        }
        // Include trailing context.
        let hunk_end = (j + CONTEXT).min(changes.len());
        let mut old_start = 0usize;
        let mut new_start = 0usize;
        // Count lines to compute offsets.
        for k in 0..hunk_start {
            match &changes[k] {
                LineChange::Same(_) | LineChange::Removed(_) => old_start += 1,
                LineChange::Added(_) => {}
            }
            match &changes[k] {
                LineChange::Same(_) | LineChange::Added(_) => new_start += 1,
                LineChange::Removed(_) => {}
            }
        }
        let mut lines = Vec::new();
        let mut old_lines = 0;
        let mut new_lines = 0;
        for k in hunk_start..hunk_end {
            match &changes[k] {
                LineChange::Same(t) => {
                    lines.push(DiffLine { kind: DiffLineKind::Context, text: (*t).to_owned() });
                    old_lines += 1;
                    new_lines += 1;
                }
                LineChange::Removed(t) => {
                    lines.push(DiffLine { kind: DiffLineKind::Removed, text: (*t).to_owned() });
                    old_lines += 1;
                }
                LineChange::Added(t) => {
                    lines.push(DiffLine { kind: DiffLineKind::Added, text: (*t).to_owned() });
                    new_lines += 1;
                }
            }
        }
        hunks.push(DiffHunk { old_start, old_lines, new_start, new_lines, lines });
        i = hunk_end;
    }

    DiffResult {
        left: from_label.to_owned(),
        right: to_label.to_owned(),
        format: DiffFormat::Structured,
        hunks,
    }
}

enum LineChange<'a> {
    Same(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

/// Myers-style line diff — returns a sequence of Same/Removed/Added operations.
fn line_diff<'a>(from: &[&'a str], to: &[&'a str]) -> Vec<LineChange<'a>> {
    let n = from.len();
    let m = to.len();
    // dp[i][j] = minimum edit distance reaching from[i], to[j].
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n { dp[i][0] = i; }
    for j in 0..=m { dp[0][j] = j; }
    for i in 1..=n {
        for j in 1..=m {
            if from[i - 1] == to[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            } else {
                dp[i][j] = 1 + dp[i - 1][j].min(dp[i][j - 1]);
            }
        }
    }
    // Backtrack.
    let mut result = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && from[i - 1] == to[j - 1] {
            result.push(LineChange::Same(from[i - 1]));
            i -= 1; j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] <= dp[i - 1][j]) {
            result.push(LineChange::Added(to[j - 1]));
            j -= 1;
        } else {
            result.push(LineChange::Removed(from[i - 1]));
            i -= 1;
        }
    }
    result.reverse();
    result
}

fn check_version(section: &Section, options: &Option<EditOptions>) -> McpResult<()> {
    if let Some(expected) = options.as_ref().and_then(|o| o.expected_version.as_ref()) {
        if *expected != section.current_version {
            return Err(McpError {
                code: McpErrorCode::Conflict,
                message: format!(
                    "section {} version conflict: expected {}, found {}",
                    section.section_id.as_str(),
                    expected.as_str(),
                    section.current_version.as_str()
                ),
            });
        }
    }
    Ok(())
}

fn no_source_span(section_id: &SectionId) -> McpError {
    McpError {
        code: McpErrorCode::Internal,
        message: format!("source span not available for section {}", section_id.as_str()),
    }
}

fn span_out_of_range(section_id: &SectionId) -> McpError {
    McpError {
        code: McpErrorCode::Internal,
        message: format!("source span is out of range for section {}", section_id.as_str()),
    }
}

fn section_matching_record(section: &Section) -> SectionMatchingRecord {
    SectionMatchingRecord {
        embedded_marker: None,
        last_known_title: section.title.clone(),
        last_known_ancestry: Vec::new(),
        last_known_ordinal: section.ordinal,
        content_fingerprint: sha256_str(section.content.as_bytes()),
    }
}

fn sha256_str(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Returns true if the Markdown file uses CRLF line endings (Windows-style).
fn detect_crlf(markdown: &str) -> bool {
    markdown.contains("\r\n")
}

/// Converts LF line endings to CRLF if `use_crlf` is true.
fn apply_line_endings(markdown: String, use_crlf: bool) -> String {
    if use_crlf {
        // Only convert bare LF (not already preceded by CR).
        let mut result = String::with_capacity(markdown.len() + markdown.len() / 20);
        let mut chars = markdown.chars().peekable();
        let mut prev_was_cr = false;
        while let Some(c) = chars.next() {
            if c == '\n' && !prev_was_cr {
                result.push('\r');
            }
            result.push(c);
            prev_was_cr = c == '\r';
        }
        result
    } else {
        markdown
    }
}

fn compute_ancestry(
    section_id: &SectionId,
    sections: &BTreeMap<SectionId, &Section>,
) -> Vec<String> {
    let mut titles = Vec::new();
    let Some(section) = sections.get(section_id) else { return titles };
    let mut parent_id = section.parent_id.as_ref();
    while let Some(id) = parent_id {
        let Some(parent) = sections.get(id) else { break };
        if parent.parent_id.is_some() {
            titles.push(parent.title.clone());
        }
        parent_id = parent.parent_id.as_ref();
    }
    titles.reverse();
    titles
}

fn compute_insert_ordinal(parent: &Section, position: &Option<crate::mcp::InsertPosition>) -> u32 {
    use crate::mcp::InsertPosition;
    match position {
        None | Some(InsertPosition::Last) => parent.children.len() as u32,
        Some(InsertPosition::First) => 0,
        Some(InsertPosition::Index(i)) => (*i).min(parent.children.len() as u32),
        Some(InsertPosition::Before(sibling_id)) => {
            parent.children.iter().position(|id| id == sibling_id)
                .map(|i| i as u32)
                .unwrap_or(parent.children.len() as u32)
        }
        Some(InsertPosition::After(sibling_id)) => {
            parent.children.iter().position(|id| id == sibling_id)
                .map(|i| i as u32 + 1)
                .unwrap_or(parent.children.len() as u32)
        }
    }
}

fn insert_child_at(
    sections: &mut BTreeMap<SectionId, Section>,
    parent_id: &SectionId,
    child_id: SectionId,
    ordinal: u32,
) {
    let children = {
        let Some(parent) = sections.get_mut(parent_id) else { return };
        let ordinal = ordinal.min(parent.children.len() as u32) as usize;
        parent.children.insert(ordinal, child_id.clone());
        parent.children.clone()
    };
    renumber_children_vec(sections, &children);
}

fn renumber_children(sections: &mut BTreeMap<SectionId, Section>, parent_id: &SectionId) {
    let children = sections.get(parent_id).map(|p| p.children.clone()).unwrap_or_default();
    renumber_children_vec(sections, &children);
}

fn renumber_children_vec(sections: &mut BTreeMap<SectionId, Section>, children: &[SectionId]) {
    for (i, child_id) in children.iter().enumerate() {
        if let Some(child) = sections.get_mut(child_id) {
            child.ordinal = i as u32;
        }
    }
}

fn update_descendant_levels(
    sections: &mut BTreeMap<SectionId, Section>,
    section_id: &SectionId,
    parent_level: u8,
) {
    let children = sections.get(section_id).map(|s| s.children.clone()).unwrap_or_default();
    for child_id in children {
        let new_level = (parent_level + 1).min(6);
        if let Some(child) = sections.get_mut(&child_id) {
            child.level = new_level;
        }
        update_descendant_levels(sections, &child_id, new_level);
    }
}

fn collect_descendants(
    section: &Section,
    sections: &BTreeMap<SectionId, Section>,
    collected: &mut Vec<SectionId>,
) {
    for child_id in &section.children {
        collected.push(child_id.clone());
        if let Some(child) = sections.get(child_id) {
            collect_descendants(child, sections, collected);
        }
    }
}

fn metadata_error(error: crate::metadata::MetadataError) -> McpError {
    let code = match &error {
        crate::metadata::MetadataError::ContentHashConflict { .. }
        | crate::metadata::MetadataError::DestinationExists(_)
        | crate::metadata::MetadataError::DocumentPathAlreadyManaged(_)
        | crate::metadata::MetadataError::RecoveryConflict { .. } => McpErrorCode::Conflict,
        crate::metadata::MetadataError::InvalidRelativePath(_)
        | crate::metadata::MetadataError::DocumentNotManaged(_)
        | crate::metadata::MetadataError::SameDocumentPath(_)
        | crate::metadata::MetadataError::CaseOnlyRenameUnsupported { .. }
        | crate::metadata::MetadataError::MissingDestinationParent(_) => McpErrorCode::InvalidInput,
        _ => McpErrorCode::Storage,
    };
    McpError {
        code,
        message: error.to_string(),
    }
}

fn workspace_error(error: WorkspaceError) -> McpError {
    McpError {
        code: McpErrorCode::Storage,
        message: error.to_string(),
    }
}

fn parse<T: DeserializeOwned>(arguments: Option<JsonObject>) -> Result<T, McpError> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default())).map_err(|error| McpError {
        code: McpErrorCode::InvalidInput,
        message: error.to_string(),
    })
}

fn to_value<T: Serialize>(result: McpResult<T>) -> Result<Value, McpError> {
    result.and_then(|value| {
        let value = serde_json::to_value(value).map_err(|error| McpError {
            code: McpErrorCode::Internal,
            message: error.to_string(),
        })?;
        if value.is_array() {
            Ok(json!({ "items": value }))
        } else if value.is_string() {
            Ok(json!({ "content": value }))
        } else {
            Ok(value)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct TestWorkspace(PathBuf);

    impl TestWorkspace {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!("vds-filesystem-service-{nonce}"));
            fs::create_dir_all(root.join("docs")).unwrap();
            fs::write(
                root.join("docs/architecture.md"),
                "# Architecture\n\nOverview.\n\n## Storage\n\nFilesystem metadata search.\n",
            )
            .unwrap();
            Self(root)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exposes_filesystem_reads_and_indexed_search() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let documents = server
            .list_documents(ListDocumentsParams::default())
            .unwrap();
        let document = &documents[0];
        let unmanaged_location = server
            .get_document_location(GetDocumentLocationParams {
                document_id: document.id.clone(),
            })
            .unwrap();
        let managed_location = server
            .manage_document_file(ManageDocumentFileParams {
                document_id: document.id.clone(),
            })
            .unwrap();
        let toc = server
            .table_of_contents(TableOfContentsParams {
                document_id: document.id.clone(),
            })
            .unwrap();
        let results = server
            .search_sections(SearchSectionsParams {
                document_id: document.id.clone(),
                query: "metadata search".to_owned(),
                options: None,
            })
            .unwrap();
        let workspace_results = server
            .full_text_search(FullTextSearchParams {
                query: "filesys*".to_owned(),
                document_id: None,
                path_prefix: Some("docs".to_owned()),
                require_all_terms: true,
                max_results: None,
            })
            .unwrap();
        let rendered = server
            .render_document_markdown(RenderDocumentMarkdownParams {
                document_id: document.id.clone(),
            })
            .unwrap();

        assert_eq!(documents.len(), 1);
        assert!(!unmanaged_location.managed);
        assert!(managed_location.managed);
        assert_eq!(managed_location.folder, "docs");
        assert_eq!(managed_location.filename, "architecture.md");
        assert!(managed_location.source_matches_metadata);
        assert_eq!(toc[0].title, "Architecture");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].section.title, "Storage");
        assert_eq!(workspace_results.len(), 1);
        assert_eq!(workspace_results[0].relative_path, "docs/architecture.md");
        assert_eq!(workspace_results[0].heading_ancestry, vec!["Architecture"]);
        assert!(rendered.contains("## Storage"));
    }

    #[test]
    fn does_not_advertise_mutations_before_filesystem_writes_are_ready() {
        assert!(AVAILABLE_TOOL_NAMES.contains(&"search_sections"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"full_text_search"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"get_document_location"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"create_document"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"manage_document_file"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"move_document_file"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"rename_document_file"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"remove_document_file"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"unmanage_document_file"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"update_section"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"create_section"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"rename_section"));
        // History and snapshot operations are now implemented.
        assert!(AVAILABLE_TOOL_NAMES.contains(&"section_versions"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"get_section_version"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"switch_section_version"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"diff_section_versions"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"create_document_snapshot"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"document_snapshots"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"restore_document_snapshot"));
        assert!(AVAILABLE_TOOL_NAMES.contains(&"diff_document_snapshots"));

        let params: FullTextSearchParams = serde_json::from_value(json!({
            "query": "filesystem",
            "path_prefix": "docs",
            "limit": 7
        }))
        .unwrap();
        assert!(params.require_all_terms);
        assert_eq!(params.max_results, Some(7));
    }

    #[test]
    fn moves_and_renames_managed_markdown_with_hash_guards() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server
            .list_documents(ListDocumentsParams::default())
            .unwrap()
            .remove(0);
        let managed = server
            .manage_document_file(ManageDocumentFileParams {
                document_id: document.id.clone(),
            })
            .unwrap();

        let renamed = server
            .rename_document_file(RenameDocumentFileParams {
                document_id: document.id.clone(),
                new_filename: "design.md".to_owned(),
                expected_content_hash: managed.content_hash.clone(),
            })
            .unwrap();
        assert_eq!(renamed.relative_path.as_deref(), Some("docs/design.md"));
        assert!(!workspace.0.join("docs/architecture.md").exists());
        assert!(workspace.0.join("docs/design.md").exists());

        let moved = server
            .move_document_file(MoveDocumentFileParams {
                document_id: document.id.clone(),
                new_relative_path: "archive/design.md".to_owned(),
                expected_content_hash: renamed.content_hash.clone().unwrap(),
                create_parent_directories: true,
            })
            .unwrap();
        assert_eq!(moved.relative_path.as_deref(), Some("archive/design.md"));
        assert!(workspace.0.join("archive/design.md").exists());
        let current = server
            .get_document_location(GetDocumentLocationParams {
                document_id: document.id.clone(),
            })
            .unwrap();
        assert_eq!(current.relative_path, "archive/design.md");
        assert_eq!(current.document_id, document.id);

        let stale = server.rename_document_file(RenameDocumentFileParams {
            document_id: document.id,
            new_filename: "stale.md".to_owned(),
            expected_content_hash: "sha256:stale".to_owned(),
        });
        assert!(matches!(
            stale,
            Err(McpError {
                code: McpErrorCode::Conflict,
                ..
            })
        ));
    }

    #[test]
    fn update_and_rename_section_surgically_edits_markdown() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        let managed = server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();
        assert!(managed.managed);

        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let storage_entry = toc[0].children.iter().find(|e| e.title == "Storage").expect("Storage section");
        let storage_id = storage_entry.section_id.clone();

        let updated = server.update_section(UpdateSectionParams {
            document_id: document.id.clone(),
            section_id: storage_id.clone(),
            content: "Updated storage content.".to_owned(),
            options: None,
        }).unwrap();
        assert_eq!(updated.title, "Storage");

        let rendered = server.render_document_markdown(RenderDocumentMarkdownParams { document_id: document.id.clone() }).unwrap();
        assert!(rendered.contains("Updated storage content."));
        assert!(rendered.contains("## Storage"));
        assert!(rendered.contains("# Architecture"));

        let renamed = server.rename_section(RenameSectionParams {
            document_id: document.id.clone(),
            section_id: storage_id.clone(),
            new_title: "Persistence".to_owned(),
            options: None,
        }).unwrap();
        assert_eq!(renamed.title, "Persistence");
        let rendered2 = server.render_document_markdown(RenderDocumentMarkdownParams { document_id: document.id.clone() }).unwrap();
        assert!(rendered2.contains("## Persistence"));
        assert!(!rendered2.contains("## Storage"));
    }

    #[test]
    fn create_and_remove_sections_update_markdown_structurally() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let arch_id = toc[0].section_id.clone();

        let created = server.create_section(CreateSectionParams {
            document_id: document.id.clone(),
            parent_id: Some(arch_id.clone()),
            title: "Caching".to_owned(),
            content: "Cache details.".to_owned(),
            position: None,
        }).unwrap();
        assert_eq!(created.title, "Caching");

        let rendered = server.render_document_markdown(RenderDocumentMarkdownParams { document_id: document.id.clone() }).unwrap();
        assert!(rendered.contains("## Caching"));
        assert!(rendered.contains("Cache details."));

        let removed = server.remove_section(RemoveSectionParams {
            document_id: document.id.clone(),
            section_id: created.section_id.clone(),
            remove_children: false,
            options: None,
        }).unwrap();
        assert_eq!(removed.section_id, created.section_id);

        let rendered2 = server.render_document_markdown(RenderDocumentMarkdownParams { document_id: document.id.clone() }).unwrap();
        assert!(!rendered2.contains("## Caching"));
    }

    #[test]
    fn append_and_set_metadata_work_on_managed_section() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let storage_id = toc[0].children[0].section_id.clone();

        server.append_to_section(AppendToSectionParams {
            document_id: document.id.clone(),
            section_id: storage_id.clone(),
            content: "Appended paragraph.".to_owned(),
            options: None,
        }).unwrap();

        let section = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: storage_id.clone(),
        }).unwrap();
        assert!(section.content.contains("Appended paragraph."));
        assert!(section.content.contains("Filesystem metadata search."));

        let metadata_updated = server.set_section_metadata(SetSectionMetadataParams {
            document_id: document.id.clone(),
            section_id: storage_id.clone(),
            metadata: crate::document::SectionMetadata { anchor: None, tags: vec!["storage".to_owned()], summary: Some("storage summary".to_owned()), locked: false },
            options: None,
        }).unwrap();
        assert_eq!(metadata_updated.title, "Storage");

        let section2 = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: storage_id,
        }).unwrap();
        assert_eq!(section2.metadata.tags, vec!["storage".to_owned()]);
    }

    #[test]
    fn create_document_creates_file_and_promotes_it() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();

        let info = server.create_document(CreateDocumentParams {
            relative_path: Some("notes/new-doc.md".to_owned()),
            name: None,
            title: Some("New Doc".to_owned()),
            initial_content: Some("# New Doc\n\nInitial content.\n".to_owned()),
        }).unwrap();

        assert_eq!(info.title, Some("New Doc".to_owned()));
        let path = workspace.0.join("notes").join("new-doc.md");
        assert!(path.exists(), "Markdown file must be created on disk");
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("# New Doc"), "initial content written");

        // The document is now managed and discoverable.
        let doc = server.require_document(&info.id).unwrap();
        assert!(doc.managed);

        // Creating again at the same path must fail.
        let err = server.create_document(CreateDocumentParams {
            relative_path: Some("notes/new-doc.md".to_owned()),
            name: None,
            title: None,
            initial_content: None,
        });
        assert!(err.is_err(), "duplicate path must be rejected");
    }

    #[test]
    fn remove_document_file_soft_deletes_file_and_leaves_tombstone() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let location = server.get_document_location(GetDocumentLocationParams {
            document_id: document.id.clone(),
        }).unwrap();
        let md_path = workspace.0.join(location.relative_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        assert!(md_path.exists());

        let result = server.remove_document_file(RemoveDocumentFileParams {
            document_id: document.id.clone(),
            expected_content_hash: location.content_hash.clone(),
        }).unwrap();

        assert_eq!(result.relative_path, None, "no active path after removal");
        assert!(!md_path.exists(), "Markdown file must be deleted");

        // Tombstone must exist in inactive archive.
        let tombstone_path = workspace.0
            .join(".vds/inactive")
            .join(document.id.as_str())
            .join("tombstone.json");
        assert!(tombstone_path.exists(), "tombstone written");

        // Document must no longer appear in the workspace.
        let docs = server.list_documents(ListDocumentsParams::default()).unwrap();
        assert!(!docs.iter().any(|d| d.id == document.id), "removed doc not in listing");
    }

    #[test]
    fn unmanage_document_file_leaves_markdown_and_archives_history() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let location = server.get_document_location(GetDocumentLocationParams {
            document_id: document.id.clone(),
        }).unwrap();
        let md_path = workspace.0.join(location.relative_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        assert!(md_path.exists());

        let result = server.unmanage_document_file(UnmanageDocumentFileParams {
            document_id: document.id.clone(),
            expected_content_hash: location.content_hash.clone(),
            archive_history: true,
        }).unwrap();

        assert!(result.relative_path.is_some(), "Markdown file path still returned");
        assert!(md_path.exists(), "Markdown file must NOT be deleted");

        // Tombstone and archived metadata must exist.
        let inactive_dir = workspace.0.join(".vds/inactive").join(document.id.as_str());
        assert!(inactive_dir.join("tombstone.json").exists(), "tombstone written");
        assert!(inactive_dir.join("archive/metadata/document.json").exists(), "history archived");

        // Active VDS metadata directory must be gone.
        let active_dir = workspace.0.join(".vds/documents").join(document.id.as_str());
        assert!(!active_dir.exists(), "active metadata removed");

        // The file still appears as unmanaged in the workspace.
        let docs = server.list_documents(ListDocumentsParams::default()).unwrap();
        let unmanaged = docs.iter().find(|d| d.id == document.id);
        // After unmanage, the doc may appear as unmanaged or not at all depending on reload.
        // What matters is it's no longer managed.
        if let Some(d) = unmanaged {
            let mat = server.require_document(&d.id).unwrap();
            assert!(!mat.managed, "document must be unmanaged after unmanage_document_file");
        }
    }

    #[test]
    fn restore_document_file_revives_removed_document() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let location = server.get_document_location(GetDocumentLocationParams {
            document_id: document.id.clone(),
        }).unwrap();
        let original_path = location.relative_path.clone();
        let md_path = workspace.0.join(original_path.replace('/', std::path::MAIN_SEPARATOR_STR));

        // Soft-delete.
        server.remove_document_file(RemoveDocumentFileParams {
            document_id: document.id.clone(),
            expected_content_hash: location.content_hash.clone(),
        }).unwrap();
        assert!(!md_path.exists(), "Markdown deleted after remove");

        // Verify inactive archive exists.
        let inactive_dir = workspace.0.join(".vds/inactive").join(document.id.as_str());
        assert!(inactive_dir.join("tombstone.json").exists());
        assert!(inactive_dir.join("archive/content.md").exists(), "content archived");

        // Restore.
        let result = server.restore_document_file(RestoreDocumentFileParams {
            document_id: document.id.clone(),
            relative_path: None,
        }).unwrap();

        assert_eq!(result.relative_path.as_deref(), Some(original_path.as_str()));
        assert!(md_path.exists(), "Markdown recreated after restore");
        assert!(!inactive_dir.exists(), "inactive archive cleaned up");

        // Document is active and managed again.
        let active_dir = workspace.0.join(".vds/documents").join(document.id.as_str());
        assert!(active_dir.exists(), "active metadata restored");
        let mat = server.require_document(&document.id).unwrap();
        assert!(mat.managed, "document is managed after restore");
    }

    #[test]
    fn structural_mutation_preserves_crlf_line_endings() {
        use crate::mcp::CreateSectionParams;

        // Write the fixture with CRLF line endings.
        let nonce: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as u64;
        let root = std::env::temp_dir().join(format!("vds-crlf-{nonce}"));
        fs::create_dir_all(root.join("docs")).unwrap();
        let crlf_markdown = "# Architecture\r\n\r\nOverview.\r\n\r\n## Storage\r\n\r\nFilesystem metadata search.\r\n";
        fs::write(root.join("docs/architecture.md"), crlf_markdown.as_bytes()).unwrap();

        let server = FilesystemVdsServer::open(&root).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        // A structural mutation (create_section) must preserve CRLF.
        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let root_id = toc[0].section_id.clone();
        server.create_section(CreateSectionParams {
            document_id: document.id.clone(),
            parent_id: Some(root_id),
            title: "New Section".to_owned(),
            content: "New content.".to_owned(),
            position: None,
        }).unwrap();

        let written = fs::read(root.join("docs/architecture.md")).unwrap();
        let written_str = String::from_utf8(written).unwrap();
        assert!(written_str.contains("\r\n"), "CRLF must be preserved after structural mutation");
        // Every \n must be preceded by \r — no bare LF should exist.
        let bare_lf = written_str.replace("\r\n", "").contains('\n');
        assert!(!bare_lf, "no bare LF in CRLF file: {:?}", written_str);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn patch_section_applies_ordered_ops_and_commits_surgically() {
        use crate::document::{PatchOp, SectionMetadata, SectionPatch};
        use crate::mcp::PatchSectionParams;

        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let section_id = toc[0].children[0].section_id.clone(); // "Storage"

        // ReplaceContent + Rename in a single patch.
        let info = server.patch_section(PatchSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            patch: SectionPatch {
                operations: vec![
                    PatchOp::ReplaceContent { content: "New storage content.".to_owned() },
                    PatchOp::Rename { title: "Storage (updated)".to_owned() },
                ],
            },
            options: None,
        }).unwrap();
        assert_eq!(info.title, "Storage (updated)");

        let section = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        assert_eq!(section.content.trim(), "New storage content.");
        assert_eq!(section.title, "Storage (updated)");

        // AppendContent followed by PrependContent.
        server.patch_section(PatchSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            patch: SectionPatch {
                operations: vec![
                    PatchOp::AppendContent { content: "Appended.".to_owned() },
                    PatchOp::PrependContent { content: "Prepended.".to_owned() },
                ],
            },
            options: None,
        }).unwrap();

        let section2 = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        assert!(section2.content.starts_with("Prepended."), "expected prepend at start: {:?}", section2.content);
        assert!(section2.content.ends_with("Appended."), "expected append at end: {:?}", section2.content);

        // SetMetadata-only patch should not rewrite Markdown.
        let markdown_before = std::fs::read_to_string(
            workspace.0.join(
                server.get_document_location(GetDocumentLocationParams { document_id: document.id.clone() })
                    .unwrap().relative_path.replace('/', std::path::MAIN_SEPARATOR_STR)
            )
        ).unwrap();
        server.patch_section(PatchSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            patch: SectionPatch {
                operations: vec![
                    PatchOp::SetMetadata { metadata: SectionMetadata { anchor: None, tags: vec!["patched".to_owned()], summary: None, locked: false } },
                ],
            },
            options: None,
        }).unwrap();
        let markdown_after = std::fs::read_to_string(
            workspace.0.join(
                server.get_document_location(GetDocumentLocationParams { document_id: document.id.clone() })
                    .unwrap().relative_path.replace('/', std::path::MAIN_SEPARATOR_STR)
            )
        ).unwrap();
        assert_eq!(markdown_before, markdown_after, "metadata-only patch must not change the Markdown file");

        let section3 = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        assert_eq!(section3.metadata.tags, vec!["patched".to_owned()]);
    }

    #[test]
    fn split_section_divides_content_and_inserts_new_section_after() {
        use crate::mcp::SplitSectionParams;

        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let section_id = toc[0].children[0].section_id.clone(); // "Storage"

        let original_section = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        // Split at the first space to keep a clean word boundary.
        let split_at = original_section.content.find(' ').unwrap_or(original_section.content.len());

        let result = server.split_section(SplitSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            split_at,
            new_title: "Storage (continued)".to_owned(),
        }).unwrap();

        assert_eq!(result.original.section_id, section_id);
        assert_ne!(result.created.section_id, section_id);
        assert_eq!(result.created.title, "Storage (continued)");

        // The two halves should cover the original content.
        let first = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        let second = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: result.created.section_id.clone(),
        }).unwrap();
        let original_trimmed = original_section.content.trim();
        assert!(original_trimmed.starts_with(first.content.trim()), "first half should be prefix of original");
        assert!(original_trimmed.ends_with(second.content.trim()), "second half should be suffix of original");

        // The new section must appear immediately after the original in the TOC.
        let toc2 = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let storage_parent = &toc2[0];
        let positions: Vec<&SectionId> = storage_parent.children.iter()
            .map(|e| &e.section_id)
            .collect();
        let orig_pos = positions.iter().position(|id| *id == &section_id).unwrap();
        let new_pos = positions.iter().position(|id| *id == &result.created.section_id).unwrap();
        assert_eq!(new_pos, orig_pos + 1, "new section must be immediately after original");
    }

    #[test]
    fn section_versions_and_get_version_return_history() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let section_id = toc[0].section_id.clone();

        // Promote writes initial version; edit to produce a second.
        server.update_section(UpdateSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            content: "Updated content.".to_owned(),
            options: None,
        }).unwrap();

        let versions = server.section_versions(SectionVersionsParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        assert!(versions.len() >= 2, "initial + edited versions expected, got {}", versions.len());

        let first_version = server.get_section_version(GetSectionVersionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            version_id: versions[0].version_id.clone(),
        }).unwrap();
        assert_eq!(first_version.section_id, section_id);
        assert!(!first_version.content.is_empty(), "version must have content");
    }

    #[test]
    fn switch_section_version_restores_historical_content() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let section_id = toc[0].section_id.clone();
        let initial_section = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        let original_content = initial_section.content.clone();

        // Edit to produce a new version.
        server.update_section(UpdateSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            content: "Completely different content.".to_owned(),
            options: None,
        }).unwrap();

        // Find the initial version and switch back to it.
        let versions = server.section_versions(SectionVersionsParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        let initial_version_id = versions[0].version_id.clone();

        server.switch_section_version(SwitchSectionVersionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            version_id: initial_version_id,
            options: None,
        }).unwrap();

        let restored = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        assert_eq!(restored.content, original_content, "content must match initial version after switch");
    }

    #[test]
    fn diff_section_versions_returns_structured_hunks() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let section_id = toc[0].section_id.clone();

        server.update_section(UpdateSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            content: "New line one.\nNew line two.".to_owned(),
            options: None,
        }).unwrap();

        let versions = server.section_versions(SectionVersionsParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        assert!(versions.len() >= 2);

        let diff = server.diff_section_versions(DiffSectionVersionsParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            from_version: versions[0].version_id.clone(),
            to_version: versions[versions.len() - 1].version_id.clone(),
        }).unwrap();
        assert!(!diff.hunks.is_empty(), "diff between two versions must have at least one hunk");
    }

    #[test]
    fn create_and_list_and_restore_document_snapshot() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();
        let document = server.list_documents(ListDocumentsParams::default()).unwrap().remove(0);
        server.manage_document_file(ManageDocumentFileParams { document_id: document.id.clone() }).unwrap();

        // Take a snapshot of the current state.
        let snapshot = server.create_document_snapshot(CreateDocumentSnapshotParams {
            document_id: document.id.clone(),
            label: Some("baseline".to_owned()),
            change_summary: None,
        }).unwrap();
        assert_eq!(snapshot.document_id, document.id);

        // Mutate the document.
        let toc = server.table_of_contents(TableOfContentsParams { document_id: document.id.clone() }).unwrap();
        let section_id = toc[0].section_id.clone();
        server.update_section(UpdateSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
            content: "Post-snapshot edit.".to_owned(),
            options: None,
        }).unwrap();

        // List snapshots — should contain our baseline.
        let infos = server.document_snapshots(DocumentSnapshotsParams {
            document_id: document.id.clone(),
        }).unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].label.as_deref(), Some("baseline"));

        // Restore the snapshot and verify content is back.
        server.restore_document_snapshot(RestoreDocumentSnapshotParams {
            document_id: document.id.clone(),
            snapshot_id: snapshot.snapshot_id.clone(),
        }).unwrap();

        let restored_section = server.get_section(GetSectionParams {
            document_id: document.id.clone(),
            section_id: section_id.clone(),
        }).unwrap();
        assert_ne!(
            restored_section.content,
            "Post-snapshot edit.",
            "content must differ from post-snapshot state after restore"
        );
    }

    #[test]
    fn watcher_reloads_workspace_after_external_file_edit() {
        let workspace = TestWorkspace::new();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();

        // Manage the document so we can verify identity survives the reload.
        let docs = server.list_documents(ListDocumentsParams::default()).unwrap();
        let doc = &docs[0];
        server.manage_document_file(ManageDocumentFileParams { document_id: doc.id.clone() }).unwrap();

        let before_count = server.generation.read().unwrap().reload_count;

        // Externally overwrite the markdown file.
        fs::write(
            workspace.0.join("docs/architecture.md"),
            "# Architecture\n\nExternal update.\n\n## Storage\n\nStill filesystem.\n",
        ).unwrap();

        // Wait for the debounce window (300 ms) plus rebuild time.
        std::thread::sleep(std::time::Duration::from_millis(700));

        let after_count = server.generation.read().unwrap().reload_count;
        assert!(
            after_count > before_count,
            "reload_count should increment after external file change (was {before_count}, got {after_count})"
        );

        // The updated content should be visible through normal reads.
        let docs_after = server.list_documents(ListDocumentsParams::default()).unwrap();
        let doc_after = docs_after.iter().find(|d| d.id == doc.id).expect("managed document must survive reload");
        let toc = server.table_of_contents(TableOfContentsParams { document_id: doc_after.id.clone() }).unwrap();
        let root_section = toc.iter().find(|s| s.title == "Architecture").expect("root section present");
        let sections = server.get_section(GetSectionParams {
            document_id: doc_after.id.clone(),
            section_id: root_section.section_id.clone(),
        }).unwrap();
        assert!(sections.content.contains("External update"), "section content should reflect external edit");
    }

    #[test]
    fn watcher_reconciles_managed_identity_after_external_rename() {
        let workspace = TestWorkspace::new();
        // Add a second file that will be externally renamed.
        fs::create_dir_all(workspace.0.join("notes")).unwrap();
        fs::write(workspace.0.join("notes/todo.md"), "# Todo\n\nOriginal task list.\n").unwrap();
        let server = FilesystemVdsServer::open(&workspace.0).unwrap();

        let docs = server.list_documents(ListDocumentsParams::default()).unwrap();
        // Find the todo.md document by checking locations.
        let todo_doc = docs.iter().find(|d| {
            server.get_document_location(GetDocumentLocationParams { document_id: d.id.clone() })
                .ok()
                .map(|l| l.relative_path)
                .as_deref() == Some("notes/todo.md")
        }).expect("todo.md must be discovered").clone();
        server.manage_document_file(ManageDocumentFileParams { document_id: todo_doc.id.clone() }).unwrap();

        let managed_id = todo_doc.id.clone();

        // Externally rename notes/todo.md → notes/tasks.md while server is running.
        fs::rename(
            workspace.0.join("notes/todo.md"),
            workspace.0.join("notes/tasks.md"),
        ).unwrap();

        // Wait for debounce + rebuild.
        std::thread::sleep(std::time::Duration::from_millis(700));

        // The document should still be discoverable by its original managed ID.
        let docs_after = server.list_documents(ListDocumentsParams::default()).unwrap();
        let reconciled = docs_after.iter().find(|d| d.id == managed_id);
        assert!(
            reconciled.is_some(),
            "managed document ID must survive external rename; found IDs: {:?}",
            docs_after.iter().map(|d| &d.id).collect::<Vec<_>>()
        );
        let reconciled_location = server.get_document_location(GetDocumentLocationParams {
            document_id: managed_id.clone(),
        }).unwrap();
        assert_eq!(
            reconciled_location.relative_path.as_str(),
            "notes/tasks.md",
            "relative_path must reflect the new file name"
        );
    }
}
