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

//! MCP command surface for the Versioned Document Service.
//!
//! This module defines transport-neutral request and response types for every
//! MCP tool described in `docs/context2.md`. The rmcp adapter can implement
//! [`VdsMcpSurface`] and delegate each command to document and storage services.

use chrono::{DateTime, Utc};
use serde::de::Error as DeError;
use serde::{Deserialize, Serialize};

#[cfg(feature = "semantic-search")]
use crate::document::TextEmbedding;
use crate::document::{
    Document, DocumentId, DocumentSnapshot, EditOptions, Section, SectionId, SectionInfo,
    SectionMetadata, SectionPatch, SectionVersion, SnapshotId, TableOfContentsEntry,
    ValidationDiagnostic, VersionId,
};

/// Result type returned by MCP command handlers.
pub type McpResult<T> = std::result::Result<T, McpError>;

fn default_true() -> bool {
    true
}

macro_rules! unsupported_surface_method {
    ($name:ident, $params:ty, $result:ty) => {
        fn $name(&self, _params: $params) -> McpResult<$result> {
            Err(McpError {
                code: McpErrorCode::InvalidInput,
                message: format!(
                    "{} is not available for this VDS service mode",
                    stringify!($name)
                ),
            })
        }
    };
}

/// Transport-neutral interface covering the full VDS MCP command list.
#[allow(unused_doc_comments)]
pub trait VdsMcpSurface {
    /// Lists all documents available in the store.
    fn list_documents(&self, params: ListDocumentsParams) -> McpResult<Vec<DocumentInfo>>;
    /// Creates a new document, optionally seeded with initial Markdown content.
    unsupported_surface_method!(create_document, CreateDocumentParams, DocumentInfo);
    /// Imports a Markdown file into the internal section tree model.
    unsupported_surface_method!(import_document, ImportDocumentParams, DocumentInfo);
    /// Exports a document from the internal model to a Markdown file.
    unsupported_surface_method!(export_document, ExportDocumentParams, ExportResult);
    /// Reads the complete document record.
    fn get_document(&self, params: GetDocumentParams) -> McpResult<Document>;
    /// Deletes a document and its associated tree, versions, and snapshots.
    unsupported_surface_method!(delete_document, DeleteDocumentParams, DeleteResult);
    /// Renames a document while preserving its stable document ID.
    unsupported_surface_method!(rename_document, RenameDocumentParams, DocumentInfo);
    /// Returns the authoritative file location and content hash for a document.
    unsupported_surface_method!(
        get_document_location,
        GetDocumentLocationParams,
        DocumentLocation
    );
    /// Promotes a discovered Markdown file into durable VDS management.
    unsupported_surface_method!(
        manage_document_file,
        ManageDocumentFileParams,
        DocumentLocation
    );
    /// Moves a managed Markdown document to another workspace-relative path.
    unsupported_surface_method!(
        move_document_file,
        MoveDocumentFileParams,
        DocumentFileMutationResult
    );
    /// Renames a managed Markdown file without changing its parent folder.
    unsupported_surface_method!(
        rename_document_file,
        RenameDocumentFileParams,
        DocumentFileMutationResult
    );
    /// Soft-deletes a managed Markdown file and preserves recoverable history.
    unsupported_surface_method!(
        remove_document_file,
        RemoveDocumentFileParams,
        DocumentFileMutationResult
    );
    /// Stops managing a Markdown file while leaving the file in place.
    unsupported_surface_method!(
        unmanage_document_file,
        UnmanageDocumentFileParams,
        DocumentFileMutationResult
    );
    /// Restores a soft-deleted document from the inactive archive.
    unsupported_surface_method!(
        restore_document_file,
        RestoreDocumentFileParams,
        DocumentFileMutationResult
    );

    /// Builds a recursive table of contents for a document.
    fn table_of_contents(
        &self,
        params: TableOfContentsParams,
    ) -> McpResult<Vec<TableOfContentsEntry>>;
    /// Reads one section by stable section ID.
    fn get_section(&self, params: GetSectionParams) -> McpResult<Section>;
    /// Reads one section and its descendants up to an optional depth.
    fn get_section_tree(&self, params: GetSectionTreeParams) -> McpResult<SectionTree>;
    /// Reads multiple sections from one document.
    fn get_sections(&self, params: GetSectionsParams) -> McpResult<Vec<Section>>;
    /// Renders one section, optionally including descendants, as Markdown.
    fn render_section_markdown(&self, params: RenderSectionMarkdownParams) -> McpResult<String>;
    /// Renders an entire document tree as Markdown.
    fn render_document_markdown(&self, params: RenderDocumentMarkdownParams) -> McpResult<String>;

    /// Creates a new section under a parent or at the document root.
    unsupported_surface_method!(create_section, CreateSectionParams, SectionInfo);
    /// Inserts a section before an existing sibling.
    unsupported_surface_method!(
        insert_section_before,
        InsertSectionBeforeParams,
        SectionInfo
    );
    /// Inserts a section after an existing sibling.
    unsupported_surface_method!(insert_section_after, InsertSectionAfterParams, SectionInfo);
    /// Splits an existing section into two sections at a content offset.
    unsupported_surface_method!(split_section, SplitSectionParams, SplitSectionResult);

    /// Replaces a section body.
    unsupported_surface_method!(update_section, UpdateSectionParams, SectionInfo);
    /// Applies targeted patch operations to a section.
    unsupported_surface_method!(patch_section, PatchSectionParams, SectionInfo);
    /// Appends content to a section body.
    unsupported_surface_method!(append_to_section, AppendToSectionParams, SectionInfo);
    /// Renames a section heading.
    unsupported_surface_method!(rename_section, RenameSectionParams, SectionInfo);
    /// Replaces section metadata.
    unsupported_surface_method!(set_section_metadata, SetSectionMetadataParams, SectionInfo);

    /// Moves a section to a new parent or sibling position.
    unsupported_surface_method!(move_section, MoveSectionParams, SectionInfo);
    /// Removes a section, optionally including its descendants.
    unsupported_surface_method!(remove_section, RemoveSectionParams, RemovedSectionInfo);
    /// Replaces the child ordering under a parent.
    unsupported_surface_method!(reorder_sections, ReorderSectionsParams, Vec<SectionInfo>);
    /// Promotes a section one heading/tree level.
    unsupported_surface_method!(promote_section, PromoteSectionParams, SectionInfo);
    /// Demotes a section one heading/tree level.
    unsupported_surface_method!(demote_section, DemoteSectionParams, SectionInfo);

    /// Lists version summaries for a section.
    unsupported_surface_method!(
        section_versions,
        SectionVersionsParams,
        Vec<SectionVersionInfo>
    );
    /// Reads a historical section version.
    unsupported_surface_method!(get_section_version, GetSectionVersionParams, SectionVersion);
    /// Makes a historical section version current.
    unsupported_surface_method!(
        switch_section_version,
        SwitchSectionVersionParams,
        SectionInfo
    );
    /// Diffs two versions of one section.
    unsupported_surface_method!(diff_section_versions, DiffSectionVersionsParams, DiffResult);
    /// Creates a document-level snapshot.
    unsupported_surface_method!(
        create_document_snapshot,
        CreateDocumentSnapshotParams,
        DocumentSnapshot
    );
    /// Lists document snapshot summaries.
    unsupported_surface_method!(
        document_snapshots,
        DocumentSnapshotsParams,
        Vec<DocumentSnapshotInfo>
    );
    /// Restores a document from a snapshot.
    unsupported_surface_method!(
        restore_document_snapshot,
        RestoreDocumentSnapshotParams,
        DocumentInfo
    );
    /// Diffs two document snapshots.
    unsupported_surface_method!(
        diff_document_snapshots,
        DiffDocumentSnapshotsParams,
        DiffResult
    );

    /// Searches section titles and/or content.
    fn search_sections(&self, params: SearchSectionsParams) -> McpResult<Vec<SectionSearchResult>>;
    /// Searches current sections across the workspace using the lexical index.
    unsupported_surface_method!(
        full_text_search,
        FullTextSearchParams,
        Vec<FullTextSearchResult>
    );
    /// Searches sections by semantic embedding nearest neighbors.
    #[cfg(feature = "semantic-search")]
    unsupported_surface_method!(
        semantic_search_sections,
        SemanticSearchSectionsParams,
        Vec<SectionSearchResult>
    );
    /// Finds sections by title, optionally with fuzzy matching.
    fn find_by_title(&self, params: FindByTitleParams) -> McpResult<Vec<SectionSearchResult>>;
    /// Finds sections with a matching tag.
    fn find_by_tag(&self, params: FindByTagParams) -> McpResult<Vec<SectionInfo>>;
    /// Lists recent document or section changes.
    unsupported_surface_method!(
        list_recent_changes,
        ListRecentChangesParams,
        Vec<ChangeRecord>
    );

    /// Validates document tree and content integrity.
    fn validate_document(
        &self,
        params: ValidateDocumentParams,
    ) -> McpResult<Vec<ValidationDiagnostic>>;
    /// Normalizes document structure or formatting according to options.
    unsupported_surface_method!(normalize_document, NormalizeDocumentParams, NormalizeResult);
    /// Attempts to repair invalid document state.
    unsupported_surface_method!(repair_document, RepairDocumentParams, RepairResult);

    /// Acquires a cooperative edit lock for a section.
    unsupported_surface_method!(lock_section, LockSectionParams, LockInfo);
    /// Releases a cooperative edit lock for a section.
    unsupported_surface_method!(unlock_section, UnlockSectionParams, UnlockResult);
    /// Checks whether an expected version still matches the current section.
    unsupported_surface_method!(check_conflicts, CheckConflictsParams, ConflictCheckResult);

    /// Sets the workspace directory and uses <workspace>/.vds/vds.db as the database path.
    unsupported_surface_method!(set_workspace, SetWorkspaceParams, WorkspaceInfo);
    /// Gets the current workspace directory if the database is in a .vds folder.
    fn get_workspace(&self, params: GetWorkspaceParams) -> McpResult<WorkspaceInfo>;
    /// Sets an explicit database file path.
    unsupported_surface_method!(set_database, SetDatabaseParams, DatabaseInfo);
    /// Gets the current database file path.
    fn get_database(&self, params: GetDatabaseParams) -> McpResult<DatabaseInfo>;
}

/// Stable command names exposed as MCP tools.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VdsTool {
    /// Tool for [`VdsMcpSurface::list_documents`].
    ListDocuments,
    /// Tool for [`VdsMcpSurface::create_document`].
    CreateDocument,
    /// Tool for [`VdsMcpSurface::import_document`].
    ImportDocument,
    /// Tool for [`VdsMcpSurface::export_document`].
    ExportDocument,
    /// Tool for [`VdsMcpSurface::get_document`].
    GetDocument,
    /// Tool for [`VdsMcpSurface::delete_document`].
    DeleteDocument,
    /// Tool for [`VdsMcpSurface::rename_document`].
    RenameDocument,
    /// Tool for [`VdsMcpSurface::get_document_location`].
    GetDocumentLocation,
    /// Tool for [`VdsMcpSurface::manage_document_file`].
    ManageDocumentFile,
    /// Tool for [`VdsMcpSurface::move_document_file`].
    MoveDocumentFile,
    /// Tool for [`VdsMcpSurface::rename_document_file`].
    RenameDocumentFile,
    /// Tool for [`VdsMcpSurface::remove_document_file`].
    RemoveDocumentFile,
    /// Tool for [`VdsMcpSurface::unmanage_document_file`].
    UnmanageDocumentFile,
    /// Tool for [`VdsMcpSurface::restore_document_file`].
    RestoreDocumentFile,
    /// Tool for [`VdsMcpSurface::table_of_contents`].
    TableOfContents,
    /// Tool for [`VdsMcpSurface::get_section`].
    GetSection,
    /// Tool for [`VdsMcpSurface::get_section_tree`].
    GetSectionTree,
    /// Tool for [`VdsMcpSurface::get_sections`].
    GetSections,
    /// Tool for [`VdsMcpSurface::render_section_markdown`].
    RenderSectionMarkdown,
    /// Tool for [`VdsMcpSurface::render_document_markdown`].
    RenderDocumentMarkdown,
    /// Tool for [`VdsMcpSurface::create_section`].
    CreateSection,
    /// Tool for [`VdsMcpSurface::insert_section_before`].
    InsertSectionBefore,
    /// Tool for [`VdsMcpSurface::insert_section_after`].
    InsertSectionAfter,
    /// Tool for [`VdsMcpSurface::split_section`].
    SplitSection,
    /// Tool for [`VdsMcpSurface::update_section`].
    UpdateSection,
    /// Tool for [`VdsMcpSurface::patch_section`].
    PatchSection,
    /// Tool for [`VdsMcpSurface::append_to_section`].
    AppendToSection,
    /// Tool for [`VdsMcpSurface::rename_section`].
    RenameSection,
    /// Tool for [`VdsMcpSurface::set_section_metadata`].
    SetSectionMetadata,
    /// Tool for [`VdsMcpSurface::move_section`].
    MoveSection,
    /// Tool for [`VdsMcpSurface::remove_section`].
    RemoveSection,
    /// Tool for [`VdsMcpSurface::reorder_sections`].
    ReorderSections,
    /// Tool for [`VdsMcpSurface::promote_section`].
    PromoteSection,
    /// Tool for [`VdsMcpSurface::demote_section`].
    DemoteSection,
    /// Tool for [`VdsMcpSurface::section_versions`].
    SectionVersions,
    /// Tool for [`VdsMcpSurface::get_section_version`].
    GetSectionVersion,
    /// Tool for [`VdsMcpSurface::switch_section_version`].
    SwitchSectionVersion,
    /// Tool for [`VdsMcpSurface::diff_section_versions`].
    DiffSectionVersions,
    /// Tool for [`VdsMcpSurface::create_document_snapshot`].
    CreateDocumentSnapshot,
    /// Tool for [`VdsMcpSurface::document_snapshots`].
    DocumentSnapshots,
    /// Tool for [`VdsMcpSurface::restore_document_snapshot`].
    RestoreDocumentSnapshot,
    /// Tool for [`VdsMcpSurface::diff_document_snapshots`].
    DiffDocumentSnapshots,
    /// Tool for [`VdsMcpSurface::search_sections`].
    SearchSections,
    /// Tool for [`VdsMcpSurface::full_text_search`].
    FullTextSearch,
    /// Tool for [`VdsMcpSurface::semantic_search_sections`].
    #[cfg(feature = "semantic-search")]
    SemanticSearchSections,
    /// Tool for [`VdsMcpSurface::find_by_title`].
    FindByTitle,
    /// Tool for [`VdsMcpSurface::find_by_tag`].
    FindByTag,
    /// Tool for [`VdsMcpSurface::list_recent_changes`].
    ListRecentChanges,
    /// Tool for [`VdsMcpSurface::validate_document`].
    ValidateDocument,
    /// Tool for [`VdsMcpSurface::normalize_document`].
    NormalizeDocument,
    /// Tool for [`VdsMcpSurface::repair_document`].
    RepairDocument,
    /// Tool for [`VdsMcpSurface::lock_section`].
    LockSection,
    /// Tool for [`VdsMcpSurface::unlock_section`].
    UnlockSection,
    /// Tool for [`VdsMcpSurface::check_conflicts`].
    CheckConflicts,
    /// Tool for [`VdsMcpSurface::set_workspace`].
    SetWorkspace,
    /// Tool for [`VdsMcpSurface::get_workspace`].
    GetWorkspace,
    /// Tool for [`VdsMcpSurface::set_database`].
    SetDatabase,
    /// Tool for [`VdsMcpSurface::get_database`].
    GetDatabase,
}

impl VdsTool {
    /// Every VDS tool in the order it should usually be presented to clients.
    #[cfg(not(feature = "semantic-search"))]
    pub const ALL: [Self; 57] = [
        Self::ListDocuments,
        Self::CreateDocument,
        Self::ImportDocument,
        Self::ExportDocument,
        Self::GetDocument,
        Self::DeleteDocument,
        Self::RenameDocument,
        Self::GetDocumentLocation,
        Self::ManageDocumentFile,
        Self::MoveDocumentFile,
        Self::RenameDocumentFile,
        Self::RemoveDocumentFile,
        Self::UnmanageDocumentFile,
        Self::RestoreDocumentFile,
        Self::TableOfContents,
        Self::GetSection,
        Self::GetSectionTree,
        Self::GetSections,
        Self::RenderSectionMarkdown,
        Self::RenderDocumentMarkdown,
        Self::CreateSection,
        Self::InsertSectionBefore,
        Self::InsertSectionAfter,
        Self::SplitSection,
        Self::UpdateSection,
        Self::PatchSection,
        Self::AppendToSection,
        Self::RenameSection,
        Self::SetSectionMetadata,
        Self::MoveSection,
        Self::RemoveSection,
        Self::ReorderSections,
        Self::PromoteSection,
        Self::DemoteSection,
        Self::SectionVersions,
        Self::GetSectionVersion,
        Self::SwitchSectionVersion,
        Self::DiffSectionVersions,
        Self::CreateDocumentSnapshot,
        Self::DocumentSnapshots,
        Self::RestoreDocumentSnapshot,
        Self::DiffDocumentSnapshots,
        Self::SearchSections,
        Self::FullTextSearch,
        Self::FindByTitle,
        Self::FindByTag,
        Self::ListRecentChanges,
        Self::ValidateDocument,
        Self::NormalizeDocument,
        Self::RepairDocument,
        Self::LockSection,
        Self::UnlockSection,
        Self::CheckConflicts,
        Self::SetWorkspace,
        Self::GetWorkspace,
        Self::SetDatabase,
        Self::GetDatabase,
    ];

    /// Every VDS tool in the order it should usually be presented to clients.
    #[cfg(feature = "semantic-search")]
    pub const ALL: [Self; 58] = [
        Self::ListDocuments,
        Self::CreateDocument,
        Self::ImportDocument,
        Self::ExportDocument,
        Self::GetDocument,
        Self::DeleteDocument,
        Self::RenameDocument,
        Self::GetDocumentLocation,
        Self::ManageDocumentFile,
        Self::MoveDocumentFile,
        Self::RenameDocumentFile,
        Self::RemoveDocumentFile,
        Self::UnmanageDocumentFile,
        Self::RestoreDocumentFile,
        Self::TableOfContents,
        Self::GetSection,
        Self::GetSectionTree,
        Self::GetSections,
        Self::RenderSectionMarkdown,
        Self::RenderDocumentMarkdown,
        Self::CreateSection,
        Self::InsertSectionBefore,
        Self::InsertSectionAfter,
        Self::SplitSection,
        Self::UpdateSection,
        Self::PatchSection,
        Self::AppendToSection,
        Self::RenameSection,
        Self::SetSectionMetadata,
        Self::MoveSection,
        Self::RemoveSection,
        Self::ReorderSections,
        Self::PromoteSection,
        Self::DemoteSection,
        Self::SectionVersions,
        Self::GetSectionVersion,
        Self::SwitchSectionVersion,
        Self::DiffSectionVersions,
        Self::CreateDocumentSnapshot,
        Self::DocumentSnapshots,
        Self::RestoreDocumentSnapshot,
        Self::DiffDocumentSnapshots,
        Self::SearchSections,
        Self::FullTextSearch,
        Self::SemanticSearchSections,
        Self::FindByTitle,
        Self::FindByTag,
        Self::ListRecentChanges,
        Self::ValidateDocument,
        Self::NormalizeDocument,
        Self::RepairDocument,
        Self::LockSection,
        Self::UnlockSection,
        Self::CheckConflicts,
        Self::SetWorkspace,
        Self::GetWorkspace,
        Self::SetDatabase,
        Self::GetDatabase,
    ];

    /// Returns the stable MCP tool name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ListDocuments => "list_documents",
            Self::CreateDocument => "create_document",
            Self::ImportDocument => "import_document",
            Self::ExportDocument => "export_document",
            Self::GetDocument => "get_document",
            Self::DeleteDocument => "delete_document",
            Self::RenameDocument => "rename_document",
            Self::GetDocumentLocation => "get_document_location",
            Self::ManageDocumentFile => "manage_document_file",
            Self::MoveDocumentFile => "move_document_file",
            Self::RenameDocumentFile => "rename_document_file",
            Self::RemoveDocumentFile => "remove_document_file",
            Self::UnmanageDocumentFile => "unmanage_document_file",
            Self::RestoreDocumentFile => "restore_document_file",
            Self::TableOfContents => "table_of_contents",
            Self::GetSection => "get_section",
            Self::GetSectionTree => "get_section_tree",
            Self::GetSections => "get_sections",
            Self::RenderSectionMarkdown => "render_section_markdown",
            Self::RenderDocumentMarkdown => "render_document_markdown",
            Self::CreateSection => "create_section",
            Self::InsertSectionBefore => "insert_section_before",
            Self::InsertSectionAfter => "insert_section_after",
            Self::SplitSection => "split_section",
            Self::UpdateSection => "update_section",
            Self::PatchSection => "patch_section",
            Self::AppendToSection => "append_to_section",
            Self::RenameSection => "rename_section",
            Self::SetSectionMetadata => "set_section_metadata",
            Self::MoveSection => "move_section",
            Self::RemoveSection => "remove_section",
            Self::ReorderSections => "reorder_sections",
            Self::PromoteSection => "promote_section",
            Self::DemoteSection => "demote_section",
            Self::SectionVersions => "section_versions",
            Self::GetSectionVersion => "get_section_version",
            Self::SwitchSectionVersion => "switch_section_version",
            Self::DiffSectionVersions => "diff_section_versions",
            Self::CreateDocumentSnapshot => "create_document_snapshot",
            Self::DocumentSnapshots => "document_snapshots",
            Self::RestoreDocumentSnapshot => "restore_document_snapshot",
            Self::DiffDocumentSnapshots => "diff_document_snapshots",
            Self::SearchSections => "search_sections",
            Self::FullTextSearch => "full_text_search",
            #[cfg(feature = "semantic-search")]
            Self::SemanticSearchSections => "semantic_search_sections",
            Self::FindByTitle => "find_by_title",
            Self::FindByTag => "find_by_tag",
            Self::ListRecentChanges => "list_recent_changes",
            Self::ValidateDocument => "validate_document",
            Self::NormalizeDocument => "normalize_document",
            Self::RepairDocument => "repair_document",
            Self::LockSection => "lock_section",
            Self::UnlockSection => "unlock_section",
            Self::CheckConflicts => "check_conflicts",
            Self::SetWorkspace => "set_workspace",
            Self::GetWorkspace => "get_workspace",
            Self::SetDatabase => "set_database",
            Self::GetDatabase => "get_database",
        }
    }

    /// Returns documentation suitable for MCP tool descriptions.
    pub fn documentation(self) -> ToolDocumentation {
        let (title, description, usage) = match self {
            Self::ListDocuments => (
                "List documents",
                "Returns lightweight summaries for every document known to VDS.",
                "Use this first when you do not know a document ID. It takes no parameters.",
            ),
            Self::CreateDocument => (
                "Create document",
                "Creates a new versioned document and optional initial section tree from Markdown.",
                "Provide a stable human-readable name, an optional display title, and optional initial Markdown content.",
            ),
            Self::ImportDocument => (
                "Import document",
                "Imports Markdown from a filesystem path into the VDS section tree model.",
                "Use this when Markdown already exists on disk and should become addressable by stable section IDs.",
            ),
            Self::ExportDocument => (
                "Export document",
                "Renders a document tree back to Markdown and writes it to disk.",
                "Use after section-level edits when a Markdown artifact is needed outside VDS.",
            ),
            Self::GetDocument => (
                "Get document",
                "Reads the full document record, including root section ID, metadata, and current version.",
                "Use this when you need document metadata or the root ID before navigating sections.",
            ),
            Self::DeleteDocument => (
                "Delete document",
                "Deletes a document and its associated section, version, and snapshot data.",
                "Use only when the whole document should be removed from VDS.",
            ),
            Self::RenameDocument => (
                "Rename document",
                "Changes the human-readable document name while preserving the document ID.",
                "Use this for display or storage-name changes; section IDs and history remain stable.",
            ),
            Self::GetDocumentLocation => (
                "Get document location",
                "Returns a document's canonical workspace-relative path, folder, filename, management state, and content hash.",
                "Call this before path mutations so the returned content hash can guard against external edits.",
            ),
            Self::ManageDocumentFile => (
                "Manage document file",
                "Promotes discovered Markdown into durable .vds identity, metadata, and initial version history.",
                "Use before durable file or section mutations on an unmanaged document.",
            ),
            Self::MoveDocumentFile => (
                "Move document file",
                "Moves a managed Markdown document to a new canonical workspace-relative path.",
                "Provide the content hash returned by get_document_location; existing destinations are never overwritten by default.",
            ),
            Self::RenameDocumentFile => (
                "Rename document file",
                "Changes a managed Markdown filename while retaining its current parent folder.",
                "Use for filename-only changes and provide the current expected content hash.",
            ),
            Self::RemoveDocumentFile => (
                "Remove document file",
                "Soft-deletes a managed Markdown file while retaining recoverable metadata, history, and a tombstone.",
                "Use only after confirming the current content hash. Permanent history purge is intentionally separate.",
            ),
            Self::UnmanageDocumentFile => (
                "Unmanage document file",
                "Leaves Markdown in place while archiving or removing its active VDS management metadata.",
                "Use when a file should remain in the project but no longer retain active stable VDS identity.",
            ),
            Self::RestoreDocumentFile => (
                "Restore document file",
                "Restores a soft-deleted document from the inactive archive, recreating its Markdown and reactivating its metadata.",
                "Provide the document ID from a tombstone. Optionally supply a relative_path to restore to a different location.",
            ),
            Self::TableOfContents => (
                "Table of contents",
                "Returns a recursive outline of a document's section tree.",
                "Use this to discover section IDs before reading or editing targeted sections.",
            ),
            Self::GetSection => (
                "Get section",
                "Reads one section by stable section ID.",
                "Use this before editing so you have the current content, metadata, and version.",
            ),
            Self::GetSectionTree => (
                "Get section tree",
                "Reads a section and its descendants up to an optional depth.",
                "Use this for local context around a subtree without rendering the entire document.",
            ),
            Self::GetSections => (
                "Get sections",
                "Reads multiple sections from the same document in one call.",
                "Use this when an edit or analysis needs several non-adjacent sections.",
            ),
            Self::RenderSectionMarkdown => (
                "Render section Markdown",
                "Renders a section as Markdown, optionally including children.",
                "Use this when you need human-readable Markdown for a subsection or subtree.",
            ),
            Self::RenderDocumentMarkdown => (
                "Render document Markdown",
                "Renders the entire document tree as Markdown.",
                "Use this for whole-document review or export previews.",
            ),
            Self::CreateSection => (
                "Create section",
                "Creates a section under a parent or at the root with optional placement.",
                "Use stable parent IDs and InsertPosition to avoid rewriting surrounding content.",
            ),
            Self::InsertSectionBefore => (
                "Insert section before",
                "Creates a section immediately before an existing sibling.",
                "Use when relative placement is clearer than an ordinal index.",
            ),
            Self::InsertSectionAfter => (
                "Insert section after",
                "Creates a section immediately after an existing sibling.",
                "Use when adding follow-up content near a known section.",
            ),
            Self::SplitSection => (
                "Split section",
                "Splits one section into an updated original section and a new section.",
                "Use byte offsets from the current section content and provide the new section title.",
            ),
            Self::UpdateSection => (
                "Update section",
                "Replaces a section body and records edit metadata.",
                "Pass EditOptions.expected_version when editing from previously read context to avoid stale overwrites.",
            ),
            Self::PatchSection => (
                "Patch section",
                "Applies targeted content, title, or metadata operations to a section.",
                "Prefer this over full replacement for precise edits such as append, prepend, rename, or range replacement.",
            ),
            Self::AppendToSection => (
                "Append to section",
                "Appends content to an existing section body.",
                "Use for additive notes or follow-up paragraphs with optimistic concurrency options.",
            ),
            Self::RenameSection => (
                "Rename section",
                "Changes a section heading while preserving its stable section ID.",
                "Use this instead of delete-and-recreate when only the title changes.",
            ),
            Self::SetSectionMetadata => (
                "Set section metadata",
                "Replaces metadata such as anchor, tags, summary, and locked state.",
                "Use this to maintain navigation and discovery data without changing body content.",
            ),
            Self::MoveSection => (
                "Move section",
                "Moves a section to a new parent or sibling position.",
                "Use this for structural edits; the section ID and history remain stable.",
            ),
            Self::RemoveSection => (
                "Remove section",
                "Removes a section, optionally including descendants.",
                "Set remove_children intentionally so callers choose between subtree deletion and child preservation semantics.",
            ),
            Self::ReorderSections => (
                "Reorder sections",
                "Replaces the ordered child list under a parent.",
                "Use when multiple siblings need to be rearranged in one coherent operation.",
            ),
            Self::PromoteSection => (
                "Promote section",
                "Promotes a section one structural or heading level.",
                "Use for outline cleanup after imports or restructuring.",
            ),
            Self::DemoteSection => (
                "Demote section",
                "Demotes a section one structural or heading level.",
                "Use when a section should become subordinate to nearby content.",
            ),
            Self::SectionVersions => (
                "Section versions",
                "Lists historical version summaries for one section.",
                "Use before diffing, restoring, or auditing a section's edit history.",
            ),
            Self::GetSectionVersion => (
                "Get section version",
                "Reads the full content and metadata for a historical section version.",
                "Use when inspecting or preparing to restore an older section state.",
            ),
            Self::SwitchSectionVersion => (
                "Switch section version",
                "Makes a historical section version current.",
                "Use with EditOptions to record the restore action and guard against stale context.",
            ),
            Self::DiffSectionVersions => (
                "Diff section versions",
                "Compares two historical versions of one section.",
                "Use this to review what changed before restoring or explaining edits.",
            ),
            Self::CreateDocumentSnapshot => (
                "Create document snapshot",
                "Captures a document-level point in time.",
                "Use before broad structural edits or milestones so the full document can be restored later.",
            ),
            Self::DocumentSnapshots => (
                "Document snapshots",
                "Lists snapshot summaries for a document.",
                "Use to find snapshot IDs for restore or diff operations.",
            ),
            Self::RestoreDocumentSnapshot => (
                "Restore document snapshot",
                "Restores a document to a prior snapshot.",
                "Use when a whole-document rollback is needed rather than a section-level restore.",
            ),
            Self::DiffDocumentSnapshots => (
                "Diff document snapshots",
                "Compares two document-level snapshots.",
                "Use to review broad changes across the document tree between milestones.",
            ),
            Self::SearchSections => (
                "Search sections",
                "Searches section titles and/or content using configurable options.",
                "Use this when you know terms but not section IDs.",
            ),
            Self::FullTextSearch => (
                "Full-text search",
                "Searches current section titles and content across the materialized workspace.",
                "Use this for workspace-wide lexical discovery, optionally restricted by document or path prefix.",
            ),
            #[cfg(feature = "semantic-search")]
            Self::SemanticSearchSections => (
                "Semantic search sections",
                "Searches sections by semantic similarity using HNSW nearest-neighbor index.",
                "IMPORTANT: VDS does not generate embeddings. You must provide pre-computed embeddings via the query_embedding parameter. VDS caches embeddings by (section_id, content_hash, model) and maintains an HNSW index for fast approximate nearest neighbors. Build with --features semantic-search to enable this tool.",
            ),
            Self::FindByTitle => (
                "Find by title",
                "Finds sections by heading title with optional fuzzy matching.",
                "Use this for navigation when section names are known approximately.",
            ),
            Self::FindByTag => (
                "Find by tag",
                "Finds sections carrying a specific tag.",
                "Use this for topic-oriented discovery across a document.",
            ),
            Self::ListRecentChanges => (
                "List recent changes",
                "Returns recent audit records for document and section changes.",
                "Use this to understand recent edits before making further changes.",
            ),
            Self::ValidateDocument => (
                "Validate document",
                "Checks document tree and content integrity and returns diagnostics.",
                "Use after imports or structural edits to detect inconsistent state.",
            ),
            Self::NormalizeDocument => (
                "Normalize document",
                "Applies optional cleanup such as heading fixes, anchor regeneration, trimming, and empty-section removal.",
                "Use after imports or large edits when the document should be mechanically cleaned up.",
            ),
            Self::RepairDocument => (
                "Repair document",
                "Attempts to repair invalid document state and reports diagnostics.",
                "Use when validation reports errors that can be fixed automatically.",
            ),
            Self::LockSection => (
                "Lock section",
                "Creates a cooperative lock for a section.",
                "Use before long-running edits where another agent should avoid writing the same section.",
            ),
            Self::UnlockSection => (
                "Unlock section",
                "Releases a cooperative lock held by an owner.",
                "Use after finishing or abandoning a locked edit.",
            ),
            Self::CheckConflicts => (
                "Check conflicts",
                "Compares an expected section version with the current stored version.",
                "Use before writing if you need an explicit stale-context check separate from an edit command.",
            ),
            Self::SetWorkspace => (
                "Set workspace",
                "Sets the workspace directory. In filesystem mode (serve-v2), this controls which project root VDS materializes from Markdown. In legacy mode (serve), it sets <workspace>/.vds/vds.db as the database path.",
                "Use this when the MCP server starts in a different directory than your project. The workspace will be reloaded at the new root.",
            ),
            Self::GetWorkspace => (
                "Get workspace",
                "Returns the current workspace root, storage backend, watcher status, and reload count.",
                "Use this to verify the current workspace and detect whether cached document data may be stale (reload_count increments on every live reload).",
            ),
            Self::SetDatabase => (
                "Set database",
                "Sets an explicit database file path (legacy VDS 1 mode only). Not applicable in filesystem-authoritative serve-v2 mode.",
                "Use this in legacy mode for custom database locations. In serve-v2 mode this operation is not supported.",
            ),
            Self::GetDatabase => (
                "Get database",
                "Returns the current storage backend identifier. Returns 'filesystem' in serve-v2 mode and the database file path in legacy mode.",
                "Use this to confirm which storage backend is active.",
            ),
        };

        ToolDocumentation {
            tool: self,
            name: self.name(),
            title,
            description,
            usage,
        }
    }
}

/// Human-readable endpoint documentation that can be exposed through MCP metadata.
#[derive(Clone, Debug, Serialize)]
pub struct EndpointDocumentation {
    /// Service display name.
    pub name: &'static str,
    /// Short summary of what the endpoint provides.
    pub description: &'static str,
    /// General usage guidance for agents.
    pub usage: &'static str,
    /// Documented tools exposed by the endpoint.
    pub tools: Vec<ToolDocumentation>,
}

/// Human-readable documentation for one MCP tool.
#[derive(Clone, Debug, Serialize)]
pub struct ToolDocumentation {
    /// Tool enum variant.
    pub tool: VdsTool,
    /// Stable MCP tool name.
    pub name: &'static str,
    /// Short display title.
    pub title: &'static str,
    /// Concise description of the tool's behavior.
    pub description: &'static str,
    /// Guidance on when and how agents should call the tool.
    pub usage: &'static str,
}

/// Returns endpoint-level documentation for clients and tool registries.
pub fn endpoint_documentation() -> EndpointDocumentation {
    EndpointDocumentation {
        name: "Versioned Document Service",
        description: "VDS 2.0 manages long-form Markdown documents as stable, versioned section trees. Filesystem-authoritative: Markdown files are the source of truth, and .vds/ metadata stores IDs, version history, and snapshots as Git-friendly JSON.",
        usage: "Start with list_documents or manage_document_file, use table_of_contents to discover stable section IDs, read sections before editing, pass expected_content_hash for conflict detection, use full_text_search for lexical discovery, and create snapshots before broad structural changes.",
        tools: tool_documentation(),
    }
}

/// Returns documentation for every VDS MCP tool.
pub fn tool_documentation() -> Vec<ToolDocumentation> {
    VdsTool::ALL
        .iter()
        .map(|tool| tool.documentation())
        .collect()
}

/// Parameters for listing documents.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ListDocumentsParams {}

/// Parameters for creating a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateDocumentParams {
    /// Workspace-relative path for the new Markdown file (required in filesystem mode).
    #[serde(default)]
    pub relative_path: Option<String>,
    /// Human-readable document name (defaults to filename stem when absent).
    #[serde(default)]
    pub name: Option<String>,
    /// Optional display title.
    pub title: Option<String>,
    /// Optional initial Markdown content to parse into sections.
    #[serde(alias = "markdown", default)]
    pub initial_content: Option<String>,
}

/// Parameters for importing a Markdown document from disk.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportDocumentParams {
    /// Human-readable document name to assign after import.
    pub name: String,
    /// Filesystem path to the source Markdown file.
    pub path: String,
}

/// Parameters for exporting a document to disk.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportDocumentParams {
    /// Document to export.
    pub document_id: DocumentId,
    /// Destination filesystem path.
    pub path: String,
}

/// Parameters for reading a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetDocumentParams {
    /// Document to read.
    pub document_id: DocumentId,
}

/// Parameters for deleting a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteDocumentParams {
    /// Document to delete.
    pub document_id: DocumentId,
}

/// Parameters for renaming a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameDocumentParams {
    /// Document to rename.
    pub document_id: DocumentId,
    /// New human-readable name.
    pub name: String,
}

/// Parameters for reading a document's authoritative filesystem location.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetDocumentLocationParams {
    pub document_id: DocumentId,
}

/// Parameters for promoting a discovered Markdown file into managed metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManageDocumentFileParams {
    pub document_id: DocumentId,
}

/// Parameters for moving a managed Markdown file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoveDocumentFileParams {
    pub document_id: DocumentId,
    /// Canonical destination relative to the workspace root.
    pub new_relative_path: String,
    /// Hash returned by `get_document_location` before the move.
    pub expected_content_hash: String,
    /// Whether missing destination parent directories may be created.
    #[serde(default)]
    pub create_parent_directories: bool,
}

/// Parameters for a filename-only rename in the current parent folder.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameDocumentFileParams {
    pub document_id: DocumentId,
    /// New filename, including the Markdown extension and no directory components.
    pub new_filename: String,
    pub expected_content_hash: String,
}

/// Parameters for soft deletion of a managed Markdown file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveDocumentFileParams {
    pub document_id: DocumentId,
    pub expected_content_hash: String,
}

/// Parameters for leaving Markdown in place while ending active management.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnmanageDocumentFileParams {
    pub document_id: DocumentId,
    pub expected_content_hash: String,
    /// Preserve metadata and history in an inactive archive. Defaults to true.
    #[serde(default = "default_true")]
    pub archive_history: bool,
}

/// Parameters for restoring a soft-deleted document from the inactive archive.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreDocumentFileParams {
    /// Archived document to restore.
    pub document_id: DocumentId,
    /// Optional workspace-relative path to restore to. Defaults to original path.
    #[serde(default)]
    pub relative_path: Option<String>,
}

/// Parameters for generating a document table of contents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TableOfContentsParams {
    /// Document to summarize.
    pub document_id: DocumentId,
}

/// Parameters for reading a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to read.
    pub section_id: SectionId,
}

/// Parameters for reading a section subtree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetSectionTreeParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Root section of the subtree.
    pub section_id: SectionId,
    /// Maximum descendant depth to include, or all descendants when absent.
    pub depth: Option<u32>,
}

/// Parameters for reading multiple sections.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetSectionsParams {
    /// Document that owns the sections.
    pub document_id: DocumentId,
    /// Sections to read.
    pub section_ids: Vec<SectionId>,
}

/// Parameters for rendering one section as Markdown.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderSectionMarkdownParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to render.
    pub section_id: SectionId,
    /// Whether descendant sections should be included.
    pub include_children: bool,
}

/// Parameters for rendering an entire document as Markdown.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderDocumentMarkdownParams {
    /// Document to render.
    pub document_id: DocumentId,
}

/// Parameters for creating a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateSectionParams {
    /// Document that will own the section.
    pub document_id: DocumentId,
    /// Parent section, or root insertion when absent.
    #[serde(alias = "parent", alias = "parent_section_id")]
    pub parent_id: Option<SectionId>,
    /// New section heading.
    pub title: String,
    /// New section body.
    pub content: String,
    /// Optional insertion position among siblings.
    pub position: Option<InsertPosition>,
}

/// Parameters for inserting a section before a sibling.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InsertSectionBeforeParams {
    /// Document that owns the sibling.
    pub document_id: DocumentId,
    /// Existing sibling section.
    pub sibling_section_id: SectionId,
    /// New section heading.
    pub title: String,
    /// New section body.
    pub content: String,
}

/// Parameters for inserting a section after a sibling.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InsertSectionAfterParams {
    /// Document that owns the sibling.
    pub document_id: DocumentId,
    /// Existing sibling section.
    pub sibling_section_id: SectionId,
    /// New section heading.
    pub title: String,
    /// New section body.
    pub content: String,
}

/// Parameters for splitting a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SplitSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to split.
    pub section_id: SectionId,
    /// Byte offset in the section body where the split occurs.
    pub split_at: usize,
    /// Heading for the newly created section.
    pub new_title: String,
}

/// Parameters for replacing section content.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to update.
    pub section_id: SectionId,
    /// Replacement section body.
    pub content: String,
    /// Edit metadata and optimistic concurrency options.
    pub options: Option<EditOptions>,
}

/// Parameters for applying a section patch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to patch.
    pub section_id: SectionId,
    /// Ordered patch operations.
    pub patch: SectionPatch,
    /// Edit metadata and optimistic concurrency options.
    pub options: Option<EditOptions>,
}

/// Parameters for appending content to a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppendToSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to append to.
    pub section_id: SectionId,
    /// Content to append.
    pub content: String,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for renaming a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to rename.
    pub section_id: SectionId,
    /// New heading title.
    pub new_title: String,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for replacing section metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetSectionMetadataParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section whose metadata should change.
    pub section_id: SectionId,
    /// Replacement metadata.
    pub metadata: SectionMetadata,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for moving a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoveSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to move.
    pub section_id: SectionId,
    /// New parent section, or root placement when absent.
    #[serde(alias = "new_parent")]
    pub new_parent_id: Option<SectionId>,
    /// Optional placement among the new siblings.
    pub position: Option<InsertPosition>,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for removing a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to remove.
    pub section_id: SectionId,
    /// Whether descendants should also be removed.
    pub remove_children: bool,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for reordering child sections.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReorderSectionsParams {
    /// Document that owns the parent.
    pub document_id: DocumentId,
    /// Parent whose children are reordered, or root when absent.
    #[serde(alias = "parent")]
    pub parent_id: Option<SectionId>,
    /// Complete ordered child list after reordering.
    pub ordered_children: Vec<SectionId>,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for promoting a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromoteSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to promote.
    pub section_id: SectionId,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for demoting a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DemoteSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to demote.
    pub section_id: SectionId,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for listing section versions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionVersionsParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section whose history should be listed.
    pub section_id: SectionId,
}

/// Parameters for reading a historical section version.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetSectionVersionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section that owns the version.
    pub section_id: SectionId,
    /// Version to read.
    pub version_id: VersionId,
}

/// Parameters for switching a section to a historical version.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwitchSectionVersionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to update.
    pub section_id: SectionId,
    /// Historical version to make current.
    pub version_id: VersionId,
    /// Edit metadata and optimistic concurrency options.
    #[serde(default)]
    pub options: Option<EditOptions>,
}

/// Parameters for diffing two section versions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffSectionVersionsParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section whose versions should be compared.
    pub section_id: SectionId,
    /// Earlier version to compare from.
    pub from_version: VersionId,
    /// Later version to compare to.
    pub to_version: VersionId,
}

/// Parameters for creating a document snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateDocumentSnapshotParams {
    /// Document to snapshot.
    pub document_id: DocumentId,
    /// Optional display label for the snapshot.
    #[serde(default, alias = "name")]
    pub label: Option<String>,
    /// Optional human-readable description of the snapshot.
    #[serde(default, alias = "description")]
    pub change_summary: Option<String>,
}

/// Parameters for listing document snapshots.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentSnapshotsParams {
    /// Document whose snapshots should be listed.
    pub document_id: DocumentId,
}

/// Parameters for restoring a document snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreDocumentSnapshotParams {
    /// Document to restore.
    pub document_id: DocumentId,
    /// Snapshot to restore from.
    pub snapshot_id: SnapshotId,
}

/// Parameters for diffing two document snapshots.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffDocumentSnapshotsParams {
    /// Document whose snapshots should be compared.
    pub document_id: DocumentId,
    /// Earlier snapshot to compare from.
    pub from_snapshot: SnapshotId,
    /// Later snapshot to compare to.
    pub to_snapshot: SnapshotId,
}

/// Parameters for searching sections.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchSectionsParams {
    /// Document to search.
    pub document_id: DocumentId,
    /// Search query.
    pub query: String,
    /// Search behavior options.
    #[serde(default)]
    pub options: Option<SearchOptions>,
}

/// Parameters for workspace-wide indexed lexical search.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FullTextSearchParams {
    /// Lexical query. A term ending in `*` performs prefix expansion.
    pub query: String,
    /// Optional document restriction.
    #[serde(default)]
    pub document_id: Option<DocumentId>,
    /// Optional canonical workspace-relative path prefix.
    #[serde(default)]
    pub path_prefix: Option<String>,
    /// Whether every query atom must match. Defaults to true.
    #[serde(default = "default_true")]
    pub require_all_terms: bool,
    /// Maximum number of results. Defaults to 50.
    #[serde(default, alias = "limit")]
    pub max_results: Option<u32>,
}

/// Parameters for semantic nearest-neighbor section search.
#[cfg(feature = "semantic-search")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticSearchSectionsParams {
    /// Document to search.
    pub document_id: DocumentId,
    /// Natural-language query to embed when model paths are provided.
    pub query: Option<String>,
    /// Precomputed query embedding. Takes precedence over query/model paths.
    pub query_embedding: Option<TextEmbedding>,
    /// Optional local ONNX model configuration for query embedding.
    pub model: Option<EmbeddingModelConfig>,
    /// Search behavior options.
    #[serde(default)]
    pub options: Option<SemanticSearchOptions>,
}

/// Local embedding model files used for semantic search queries.
#[cfg(feature = "semantic-search")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbeddingModelConfig {
    /// Path to an ONNX embedding model.
    pub model_path: String,
    /// Path to the tokenizer JSON for the embedding model.
    pub tokenizer_path: String,
}

/// Parameters for title-based section search.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FindByTitleParams {
    /// Document to search.
    pub document_id: DocumentId,
    /// Title query.
    pub title: String,
    /// Whether fuzzy title matching should be used (default: false).
    #[serde(default)]
    pub fuzzy: bool,
}

/// Parameters for tag-based section search.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FindByTagParams {
    /// Document to search.
    pub document_id: DocumentId,
    /// Tag to match.
    pub tag: String,
}

/// Parameters for listing recent changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListRecentChangesParams {
    /// Document whose changes should be listed.
    pub document_id: DocumentId,
    /// Maximum number of changes to return.
    pub limit: Option<u32>,
}

/// Parameters for validating a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidateDocumentParams {
    /// Document to validate.
    pub document_id: DocumentId,
}

/// Parameters for normalizing a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormalizeDocumentParams {
    /// Document to normalize.
    pub document_id: DocumentId,
    /// Normalization behavior options.
    #[serde(default)]
    pub options: Option<NormalizeOptions>,
}

/// Parameters for repairing a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepairDocumentParams {
    /// Document to repair.
    pub document_id: DocumentId,
}

/// Parameters for locking a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to lock.
    pub section_id: SectionId,
    /// Lock owner identifier.
    pub owner: String,
    /// Optional time-to-live for the lock.
    pub ttl_seconds: Option<u64>,
}

/// Parameters for unlocking a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnlockSectionParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to unlock.
    pub section_id: SectionId,
    /// Lock owner identifier.
    pub owner: String,
}

/// Parameters for optimistic concurrency conflict checks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckConflictsParams {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section to check.
    pub section_id: SectionId,
    /// Version the caller expects to still be current.
    pub expected_version: VersionId,
}

/// Parameters for setting the workspace directory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetWorkspaceParams {
    /// Workspace directory path.
    pub workspace: String,
}

/// Parameters for getting the workspace directory.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GetWorkspaceParams {}

/// Parameters for setting the database file path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetDatabaseParams {
    /// Database file path.
    pub database: String,
}

/// Parameters for getting the database file path.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GetDatabaseParams {}

/// Workspace information result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    /// Workspace directory path, if set and database is in .vds folder.
    pub workspace: Option<String>,
    /// Current database file path.
    pub database: String,
    /// Whether a filesystem watcher is actively monitoring the workspace for external changes.
    #[serde(default)]
    pub watcher_active: bool,
    /// Number of live reloads that have completed since the server started.
    /// Zero on initial load; increments each time an external change or VDS
    /// mutation triggers a workspace rebuild.  Clients can compare successive
    /// values to detect that cached document or section data may be stale.
    #[serde(default)]
    pub reload_count: u64,
}

/// Database information result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseInfo {
    /// Current database file path.
    pub database: String,
}

/// Lightweight document summary returned by lifecycle commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentInfo {
    /// Stable document identifier.
    pub id: DocumentId,
    /// Human-readable document name.
    pub name: String,
    /// Optional display title.
    pub title: Option<String>,
    /// Root section identifier.
    pub root: SectionId,
    /// Current document-level version.
    pub current_version: VersionId,
    /// Last document update time.
    pub updated_at: DateTime<Utc>,
}

/// Authoritative location and change-detection state for one Markdown file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentLocation {
    pub document_id: DocumentId,
    pub relative_path: String,
    pub folder: String,
    pub filename: String,
    pub managed: bool,
    pub source_matches_metadata: bool,
    pub content_hash: String,
}

/// Common result for document filesystem lifecycle mutations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentFileMutationResult {
    pub document_id: DocumentId,
    pub previous_relative_path: String,
    /// Current path, or `None` when the active Markdown file was removed.
    pub relative_path: Option<String>,
    pub content_hash: Option<String>,
    pub managed: bool,
}

/// Result returned after exporting a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportResult {
    /// Exported document.
    pub document_id: DocumentId,
    /// Destination path written by the export.
    pub path: String,
    /// Number of bytes written.
    pub bytes_written: u64,
}

/// Result returned after deleting a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteResult {
    /// Deleted document.
    pub document_id: DocumentId,
    /// Whether a document was actually deleted.
    pub deleted: bool,
    /// Number of section records deleted.
    pub sections_deleted: usize,
    /// Number of section version records deleted.
    pub versions_deleted: usize,
    /// Number of document snapshot records deleted.
    pub snapshots_deleted: usize,
}

/// Recursive section tree result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionTree {
    /// Section at this tree node.
    pub section: Section,
    /// Child section subtrees in document order.
    pub children: Vec<SectionTree>,
}

/// Result returned after splitting a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SplitSectionResult {
    /// Updated original section.
    pub original: SectionInfo,
    /// Newly created section.
    pub created: SectionInfo,
}

/// Summary of a removed section and affected descendants.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemovedSectionInfo {
    /// Removed section.
    pub section_id: SectionId,
    /// Former parent section, if any.
    pub parent_id: Option<SectionId>,
    /// Descendant sections removed along with the target.
    pub removed_children: Vec<SectionId>,
}

/// Lightweight section version summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionVersionInfo {
    /// Version identifier.
    pub version_id: VersionId,
    /// Section that owns the version.
    pub section_id: SectionId,
    /// Time when the version was created.
    pub created_at: DateTime<Utc>,
    /// Optional actor responsible for the version.
    pub author: Option<String>,
    /// Optional human-readable change summary.
    pub change_summary: Option<String>,
}

/// Lightweight document snapshot summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentSnapshotInfo {
    /// Snapshot identifier.
    pub snapshot_id: SnapshotId,
    /// Document captured by the snapshot.
    pub document_id: DocumentId,
    /// Optional snapshot label.
    pub label: Option<String>,
    /// Time when the snapshot was created.
    pub created_at: DateTime<Utc>,
    /// Optional actor responsible for the snapshot.
    pub author: Option<String>,
    /// Optional human-readable snapshot summary.
    pub change_summary: Option<String>,
}

/// Structured diff result for section versions or document snapshots.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffResult {
    /// Left-hand compared object identifier or label.
    pub left: String,
    /// Right-hand compared object identifier or label.
    pub right: String,
    /// Diff representation format.
    pub format: DiffFormat,
    /// Diff hunks in display order.
    pub hunks: Vec<DiffHunk>,
}

/// One hunk in a structured diff.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Starting line in the old content.
    pub old_start: usize,
    /// Number of old-content lines covered.
    pub old_lines: usize,
    /// Starting line in the new content.
    pub new_start: usize,
    /// Number of new-content lines covered.
    pub new_lines: usize,
    /// Lines contained in this hunk.
    pub lines: Vec<DiffLine>,
}

/// One line in a structured diff hunk.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffLine {
    /// Whether the line is context, added, or removed.
    pub kind: DiffLineKind,
    /// Line text without a diff prefix.
    pub text: String,
}

/// Search result for a matching section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionSearchResult {
    /// Matching section summary.
    pub section: SectionInfo,
    /// Search relevance score.
    pub score: f32,
    /// Whether the title matched.
    pub title_match: bool,
    /// Content match snippets.
    pub content_matches: Vec<TextMatch>,
}

/// Workspace-aware result returned by indexed lexical search.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FullTextSearchResult {
    /// Document containing the matching section.
    pub document_id: DocumentId,
    /// Canonical workspace-relative Markdown path.
    pub relative_path: String,
    /// Matching section summary.
    pub section: SectionInfo,
    /// Heading titles from the document root to the section parent.
    pub heading_ancestry: Vec<String>,
    /// BM25-style relevance score.
    pub score: f32,
    /// Whether the section title matched.
    pub title_match: bool,
    /// Content match snippets and byte offsets.
    pub content_matches: Vec<TextMatch>,
}

/// Text match location and snippet.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextMatch {
    /// Start byte offset of the match.
    pub start: usize,
    /// End byte offset of the match.
    pub end: usize,
    /// Human-readable snippet around the match.
    pub snippet: String,
}

/// Audit-style record for a document or section change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeRecord {
    /// Stable revision identifier.
    pub revision_id: String,
    /// Document affected by the change.
    pub document_id: DocumentId,
    /// Section affected by the change, if applicable.
    pub section_id: Option<SectionId>,
    /// Section version created or referenced by the change, if applicable.
    pub version_id: Option<VersionId>,
    /// Snapshot created or referenced by the change, if applicable.
    pub snapshot_id: Option<SnapshotId>,
    /// Type of change.
    pub change_kind: ChangeKind,
    /// Time when the change occurred.
    pub created_at: DateTime<Utc>,
    /// Optional actor responsible for the change.
    pub author: Option<String>,
    /// Optional human-readable change summary.
    pub change_summary: Option<String>,
}

/// Result returned after document normalization.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormalizeResult {
    /// Normalized document.
    pub document_id: DocumentId,
    /// Whether normalization changed stored state.
    pub changed: bool,
    /// Diagnostics produced during normalization.
    pub diagnostics: Vec<ValidationDiagnostic>,
}

/// Result returned after document repair.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepairResult {
    /// Repaired document.
    pub document_id: DocumentId,
    /// Whether repair changed stored state.
    pub repaired: bool,
    /// Diagnostics produced during repair.
    pub diagnostics: Vec<ValidationDiagnostic>,
}

/// Active cooperative section lock information.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockInfo {
    /// Locked document.
    pub document_id: DocumentId,
    /// Locked section.
    pub section_id: SectionId,
    /// Lock owner identifier.
    pub owner: String,
    /// Optional lock expiration time.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Result returned after unlocking a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnlockResult {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section that was unlocked.
    pub section_id: SectionId,
    /// Whether an active lock was released.
    pub unlocked: bool,
}

/// Result of an optimistic concurrency check.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConflictCheckResult {
    /// Document that owns the section.
    pub document_id: DocumentId,
    /// Section that was checked.
    pub section_id: SectionId,
    /// Version supplied by the caller.
    pub expected_version: VersionId,
    /// Current stored section version.
    pub current_version: VersionId,
    /// Whether the expected version differs from the current version.
    pub conflicted: bool,
}

/// Insertion location for section creation and movement.
///
/// When calling MCP tools, this enum must be serialized as either a string or a map:
/// - `"First"` or `{"First": null}` - Insert as the first child
/// - `"Last"` or `{"Last": null}` - Insert as the last child
/// - `{"Before": "section-id"}` - Insert before the given sibling
/// - `{"After": "section-id"}` - Insert after the given sibling
/// - `{"Index": 0}` - Insert at a zero-based sibling index
///
/// # Examples
///
/// ```json
/// // Insert as first child
/// {"position": "First"}
///
/// // Insert after a specific section
/// {"position": {"After": "sec-033OxhUqzh8GePdzFZ4yvm"}}
///
/// // Insert at index 2
/// {"position": {"Index": 2}}
/// ```
/// Position specification for inserting sections relative to siblings.
///
/// # JSON Format Examples
/// ```json
/// "First"                          // Insert as first child
/// "Last"                           // Insert as last child
/// {"Before": "sec-abc123"}         // Insert before section
/// {"After": "sec-abc123"}          // Insert after section
/// {"Index": 2}                     // Insert at index 2
/// ```
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum InsertPosition {
    /// Insert as the first child.
    First,
    /// Insert as the last child.
    Last,
    /// Insert before the given sibling section.
    Before(SectionId),
    /// Insert after the given sibling section.
    After(SectionId),
    /// Insert at a zero-based sibling index.
    Index(u32),
}

impl<'de> Deserialize<'de> for InsertPosition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(kind) => match normalize_variant_name(&kind).as_str() {
                "first" => Ok(Self::First),
                "last" => Ok(Self::Last),
                _ => Err(D::Error::custom(format!(
                    "unknown insert position variant: {kind}"
                ))),
            },
            serde_json::Value::Number(number) => number
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .map(Self::Index)
                .ok_or_else(|| D::Error::custom("insert position index must fit in u32")),
            serde_json::Value::Object(object) if object.len() == 1 => {
                let (kind, payload) = object.into_iter().next().expect("checked len");
                match normalize_variant_name(&kind).as_str() {
                    "before" => section_id_payload(payload)
                        .map(Self::Before)
                        .map_err(D::Error::custom),
                    "after" => section_id_payload(payload)
                        .map(Self::After)
                        .map_err(D::Error::custom),
                    "index" => index_payload(payload)
                        .map(Self::Index)
                        .map_err(D::Error::custom),
                    _ => Err(D::Error::custom(format!(
                        "unknown insert position variant: {kind}"
                    ))),
                }
            }
            _ => Err(D::Error::custom(
                "insert position must be a variant string, index number, or single-key variant map",
            )),
        }
    }
}

fn normalize_variant_name(kind: &str) -> String {
    kind.chars()
        .filter(|ch| *ch != '_' && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

fn section_id_payload(payload: serde_json::Value) -> Result<SectionId, serde_json::Error> {
    serde_json::from_value::<SectionId>(payload)
}

fn index_payload(payload: serde_json::Value) -> Result<u32, serde_json::Error> {
    serde_json::from_value::<u32>(payload)
}

/// Options controlling section search behavior.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchOptions {
    /// Whether section body content should be searched.
    pub include_content: bool,
    /// Whether section titles should be searched.
    pub include_titles: bool,
    /// Whether title matching may use fuzzy matching.
    pub fuzzy_titles: bool,
    /// Maximum number of results to return.
    #[serde(alias = "limit")]
    pub max_results: Option<u32>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            include_content: true,
            include_titles: true,
            fuzzy_titles: false,
            max_results: Some(50),
        }
    }
}

/// Options controlling semantic section search behavior.
#[cfg(feature = "semantic-search")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SemanticSearchOptions {
    /// Maximum number of nearest sections to return.
    pub max_results: Option<u32>,
    /// HNSW search beam width. Higher values improve recall at higher cost.
    pub ef: Option<usize>,
    /// HNSW maximum connections per graph layer.
    pub m: Option<usize>,
    /// HNSW construction candidate list size.
    pub ef_construction: Option<usize>,
    /// Only search section embeddings from the same model name as the query.
    pub require_same_model: bool,
}

#[cfg(feature = "semantic-search")]
impl Default for SemanticSearchOptions {
    fn default() -> Self {
        Self {
            max_results: Some(10),
            ef: Some(50),
            m: Some(16),
            ef_construction: Some(200),
            require_same_model: false,
        }
    }
}

/// Options controlling document normalization.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NormalizeOptions {
    /// Whether heading levels should be made structurally consistent.
    pub fix_heading_levels: bool,
    /// Whether section anchors should be regenerated.
    pub regenerate_anchors: bool,
    /// Whether leading and trailing whitespace should be trimmed.
    pub trim_whitespace: bool,
    /// Whether empty sections should be removed.
    pub remove_empty_sections: bool,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        Self {
            fix_heading_levels: true,
            regenerate_anchors: true,
            trim_whitespace: true,
            remove_empty_sections: false,
        }
    }
}

/// Format used for diff results.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DiffFormat {
    /// Unified textual diff shape.
    Unified,
    /// Structured hunk and line diff shape.
    Structured,
}

/// Classification for a diff line.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DiffLineKind {
    /// Unchanged context line.
    Context,
    /// Added line.
    Added,
    /// Removed line.
    Removed,
}

/// Kinds of changes that can appear in recent-change results.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChangeKind {
    /// A document was created.
    DocumentCreated,
    /// A document was renamed.
    DocumentRenamed,
    /// A document was deleted.
    DocumentDeleted,
    /// A document was imported.
    DocumentImported,
    /// A document was exported.
    DocumentExported,
    /// A section was created.
    SectionCreated,
    /// A section body was replaced.
    SectionUpdated,
    /// A section was patched.
    SectionPatched,
    /// A section was moved.
    SectionMoved,
    /// A section was removed.
    SectionRemoved,
    /// A section was renamed.
    SectionRenamed,
    /// A historical section version was made current.
    SectionVersionSwitched,
    /// A document snapshot was created.
    SnapshotCreated,
    /// A document snapshot was restored.
    SnapshotRestored,
    /// A document was normalized.
    DocumentNormalized,
    /// A document was repaired.
    DocumentRepaired,
}

/// Error returned by an MCP command handler.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpError {
    /// Machine-readable error category.
    pub code: McpErrorCode,
    /// Human-readable error message.
    pub message: String,
}

/// Machine-readable MCP error category.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum McpErrorCode {
    /// Requested document, section, version, or snapshot was not found.
    NotFound,
    /// Optimistic concurrency or version conflict.
    Conflict,
    /// Request parameters were invalid.
    InvalidInput,
    /// Section is locked by another owner.
    Locked,
    /// Storage layer failed.
    Storage,
    /// Unexpected internal failure.
    Internal,
}
