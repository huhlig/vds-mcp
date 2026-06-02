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

//! Runtime MCP service adapter for VDS.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, JsonObject, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as RmcpError, ServiceExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::document::{
    Document, DocumentId, DocumentSnapshot, EditOptions, PatchOp, Section, SectionId, SectionInfo,
    SectionVersion, TableOfContentsEntry, ValidationDiagnostic, VersionId,
};
use crate::markdown::{
    export_markdown_file, export_markdown_string, import_markdown_file, import_markdown_str,
    render_section_markdown_string,
};
use crate::mcp::*;
use crate::storage::{DocumentStore, StorageError};

/// VDS MCP server backed by a redb document store.
pub struct VdsServer {
    store: std::sync::RwLock<DocumentStore>,
    database_path: std::sync::RwLock<PathBuf>,
}

impl VdsServer {
    /// Opens the MCP service against the given database path.
    pub fn open(database: impl Into<PathBuf>) -> Result<Self, ServiceError> {
        let database = database.into();
        if let Some(parent) = database.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let store = DocumentStore::open(&database)?;
        Ok(Self {
            store: std::sync::RwLock::new(store),
            database_path: std::sync::RwLock::new(database),
        })
    }

    /// Reopens the database at a new path.
    /// This closes the current database and opens/creates a new one.
    pub fn reopen_database(&self, new_path: PathBuf) -> Result<(), ServiceError> {
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let new_store = DocumentStore::open(&new_path)?;
        
        *self.store.write().unwrap() = new_store;
        *self.database_path.write().unwrap() = new_path;
        
        Ok(())
    }

    /// Gets the current database path.
    pub fn get_database_path(&self) -> PathBuf {
        self.database_path.read().unwrap().clone()
    }

    /// Sets the workspace directory and uses <workspace>/.vds/vds.db as the database path.
    pub fn set_workspace_path(&self, workspace: PathBuf) -> Result<(), ServiceError> {
        let database_path = workspace.join(".vds").join("vds.db");
        self.reopen_database(database_path)
    }

    /// Gets the workspace directory (parent of .vds directory) if the database is in a .vds folder.
    pub fn get_workspace_path(&self) -> Option<PathBuf> {
        let db_path = self.database_path.read().unwrap();
        db_path.parent()
            .and_then(|parent| {
                if parent.file_name()?.to_str()? == ".vds" {
                    parent.parent().map(|p| p.to_path_buf())
                } else {
                    None
                }
            })
    }

    /// Helper to access the store - returns a guard that derefs to &DocumentStore
    fn store(&self) -> std::sync::RwLockReadGuard<'_, DocumentStore> {
        self.store.read().unwrap()
    }

    fn call(&self, name: &str, arguments: Option<JsonObject>) -> Result<Value, McpError> {
        match name {
            "list_documents" => self.to_value(self.list_documents(parse(arguments)?)),
            "create_document" => self.to_value(self.create_document(parse(arguments)?)),
            "import_document" => self.to_value(self.import_document(parse(arguments)?)),
            "export_document" => self.to_value(self.export_document(parse(arguments)?)),
            "get_document" => self.to_value(self.get_document(parse(arguments)?)),
            "delete_document" => self.to_value(self.delete_document(parse(arguments)?)),
            "rename_document" => self.to_value(self.rename_document(parse(arguments)?)),
            "table_of_contents" => self.to_value(self.table_of_contents(parse(arguments)?)),
            "get_section" => self.to_value(self.get_section(parse(arguments)?)),
            "get_section_tree" => self.to_value(self.get_section_tree(parse(arguments)?)),
            "get_sections" => self.to_value(self.get_sections(parse(arguments)?)),
            "render_section_markdown" => {
                self.to_value(self.render_section_markdown(parse(arguments)?))
            }
            "render_document_markdown" => {
                self.to_value(self.render_document_markdown(parse(arguments)?))
            }
            "create_section" => self.to_value(self.create_section(parse(arguments)?)),
            "insert_section_before" => self.to_value(self.insert_section_before(parse(arguments)?)),
            "insert_section_after" => self.to_value(self.insert_section_after(parse(arguments)?)),
            "split_section" => self.to_value(self.split_section(parse(arguments)?)),
            "update_section" => self.to_value(self.update_section(parse(arguments)?)),
            "patch_section" => self.to_value(self.patch_section(parse(arguments)?)),
            "append_to_section" => self.to_value(self.append_to_section(parse(arguments)?)),
            "rename_section" => self.to_value(self.rename_section(parse(arguments)?)),
            "set_section_metadata" => self.to_value(self.set_section_metadata(parse(arguments)?)),
            "move_section" => self.to_value(self.move_section(parse(arguments)?)),
            "remove_section" => self.to_value(self.remove_section(parse(arguments)?)),
            "reorder_sections" => self.to_value(self.reorder_sections(parse(arguments)?)),
            "promote_section" => self.to_value(self.promote_section(parse(arguments)?)),
            "demote_section" => self.to_value(self.demote_section(parse(arguments)?)),
            "section_versions" => self.to_value(self.section_versions(parse(arguments)?)),
            "get_section_version" => self.to_value(self.get_section_version(parse(arguments)?)),
            "switch_section_version" => {
                self.to_value(self.switch_section_version(parse(arguments)?))
            }
            "diff_section_versions" => self.to_value(self.diff_section_versions(parse(arguments)?)),
            "create_document_snapshot" => {
                self.to_value(self.create_document_snapshot(parse(arguments)?))
            }
            "document_snapshots" => self.to_value(self.document_snapshots(parse(arguments)?)),
            "restore_document_snapshot" => {
                self.to_value(self.restore_document_snapshot(parse(arguments)?))
            }
            "diff_document_snapshots" => {
                self.to_value(self.diff_document_snapshots(parse(arguments)?))
            }
            "search_sections" => self.to_value(self.search_sections(parse(arguments)?)),
            #[cfg(feature = "semantic-search")]
            "semantic_search_sections" => {
                self.to_value(self.semantic_search_sections(parse(arguments)?))
            }
            "find_by_title" => self.to_value(self.find_by_title(parse(arguments)?)),
            "find_by_tag" => self.to_value(self.find_by_tag(parse(arguments)?)),
            "list_recent_changes" => self.to_value(self.list_recent_changes(parse(arguments)?)),
            "validate_document" => self.to_value(self.validate_document(parse(arguments)?)),
            "normalize_document" => self.to_value(self.normalize_document(parse(arguments)?)),
            "repair_document" => self.to_value(self.repair_document(parse(arguments)?)),
            "lock_section" => self.to_value(self.lock_section(parse(arguments)?)),
            "unlock_section" => self.to_value(self.unlock_section(parse(arguments)?)),
            "check_conflicts" => self.to_value(self.check_conflicts(parse(arguments)?)),
            "set_workspace" => self.to_value(self.set_workspace(parse(arguments)?)),
            "get_workspace" => self.to_value(self.get_workspace(parse(arguments)?)),
            "set_database" => self.to_value(self.set_database(parse(arguments)?)),
            "get_database" => self.to_value(self.get_database(parse(arguments)?)),
            _ => Err(mcp_error(
                McpErrorCode::InvalidInput,
                format!("unknown tool: {name}"),
            )),
        }
    }

    fn to_value<T: Serialize>(&self, result: McpResult<T>) -> Result<Value, McpError> {
        result.and_then(|value| {
            let json_value = serde_json::to_value(value)
                .map_err(|error| mcp_error(McpErrorCode::Internal, error.to_string()))?;

            // MCP protocol requires structured content to be a JSON object, not an array or string.
            // Wrap arrays and strings in an object to comply with the protocol specification.
            if json_value.is_array() {
                Ok(serde_json::json!({ "items": json_value }))
            } else if json_value.is_string() {
                Ok(serde_json::json!({ "content": json_value }))
            } else {
                Ok(json_value)
            }
        })
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

    fn section_info(&self, section: Section) -> SectionInfo {
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

    fn require_document(&self, document_id: &DocumentId) -> McpResult<Document> {
        self.store().get_document(document_id)?.ok_or_else(|| {
            mcp_error(
                McpErrorCode::NotFound,
                format!("document not found: {}", document_id.as_str()),
            )
        })
    }

    fn require_section(
        &self,
        document_id: &DocumentId,
        section_id: &SectionId,
    ) -> McpResult<Section> {
        let section = self.store().get_section(section_id)?.ok_or_else(|| {
            mcp_error(
                McpErrorCode::NotFound,
                format!("section not found: {}", section_id.as_str()),
            )
        })?;
        if &section.document_id != document_id {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                format!(
                    "section {} does not belong to document {}",
                    section_id.as_str(),
                    document_id.as_str()
                ),
            ));
        }
        Ok(section)
    }

    fn build_toc(
        &self,
        document_id: &DocumentId,
        parent: &SectionId,
    ) -> McpResult<Vec<TableOfContentsEntry>> {
        let children = self.store().list_child_sections(document_id, Some(parent))?;
        children
            .into_iter()
            .map(|section| {
                let children = self.build_toc(document_id, &section.section_id)?;
                Ok(TableOfContentsEntry {
                    section_id: section.section_id,
                    title: section.title,
                    level: section.level,
                    ordinal: section.ordinal,
                    children,
                })
            })
            .collect()
    }

    fn build_tree(
        &self,
        document_id: &DocumentId,
        section: Section,
        depth: Option<u32>,
    ) -> McpResult<SectionTree> {
        let children = if depth == Some(0) {
            Vec::new()
        } else {
            let next_depth = depth.map(|value| value.saturating_sub(1));
            self.store()
                .list_child_sections(document_id, Some(&section.section_id))?
                .into_iter()
                .map(|child| self.build_tree(document_id, child, next_depth))
                .collect::<McpResult<Vec<_>>>()?
        };
        Ok(SectionTree { section, children })
    }

    fn touch_document(&self, document: &mut Document) -> McpResult<()> {
        document.current_version = VersionId::new_v7();
        document.updated_at = Utc::now();
        self.store().put_document(document)?;
        Ok(())
    }

    fn ensure_editable(&self, section: &Section, options: &Option<EditOptions>) -> McpResult<()> {
        if section.metadata.locked {
            return Err(mcp_error(
                McpErrorCode::Locked,
                format!("section is locked: {}", section.section_id.as_str()),
            ));
        }
        if let Some(opts) = options {
            if opts
                .expected_version
                .as_ref()
                .is_some_and(|expected| expected != &section.current_version)
            {
                return Err(mcp_error(
                    McpErrorCode::Conflict,
                    "section version does not match expected version",
                ));
            }
        }
        Ok(())
    }

    fn save_section_version(
        &self,
        section: &mut Section,
        options: &Option<EditOptions>,
    ) -> McpResult<SectionVersion> {
        let now = Utc::now();
        section.current_version = VersionId::new_v7();
        section.updated_at = now;
        let default_opts = EditOptions::default();
        let opts = options.as_ref().unwrap_or(&default_opts);
        let version = SectionVersion {
            version_id: section.current_version.clone(),
            section_id: section.section_id.clone(),
            title: section.title.clone(),
            content: section.content.clone(),
            metadata: section.metadata.clone(),
            embedding: section.embedding.clone(),
            created_at: now,
            author: opts.author.clone(),
            change_summary: opts.change_summary.clone(),
        };
        self.store().put_section(section)?;
        self.store().put_section_version(&version)?;
        Ok(version)
    }

    fn default_options(summary: impl Into<String>) -> Option<EditOptions> {
        Some(EditOptions {
            expected_version: None,
            author: None,
            change_summary: Some(summary.into()),
        })
    }

    fn child_parent(&self, document: &Document, parent: Option<SectionId>) -> McpResult<Section> {
        self.require_section(&document.id, parent.as_ref().unwrap_or(&document.root))
    }

    fn ordered_insert_index(
        &self,
        siblings: &[Section],
        position: Option<&InsertPosition>,
    ) -> McpResult<usize> {
        let index = match position.unwrap_or(&InsertPosition::Last) {
            InsertPosition::First => 0,
            InsertPosition::Last => siblings.len(),
            InsertPosition::Index(index) => (*index as usize).min(siblings.len()),
            InsertPosition::Before(section_id) => siblings
                .iter()
                .position(|section| &section.section_id == section_id)
                .ok_or_else(|| mcp_error(McpErrorCode::InvalidInput, "sibling not found"))?,
            InsertPosition::After(section_id) => siblings
                .iter()
                .position(|section| &section.section_id == section_id)
                .map(|index| index + 1)
                .ok_or_else(|| mcp_error(McpErrorCode::InvalidInput, "sibling not found"))?,
        };
        Ok(index)
    }

    fn renumber_siblings(&self, siblings: &mut [Section]) -> McpResult<()> {
        for (index, sibling) in siblings.iter_mut().enumerate() {
            sibling.ordinal = index as u32;
            self.store().put_section(sibling)?;
        }
        Ok(())
    }

    fn create_child_section(
        &self,
        document: &mut Document,
        mut parent: Section,
        title: String,
        content: String,
        position: Option<InsertPosition>,
    ) -> McpResult<SectionInfo> {
        let mut siblings = self.store().list_child_sections(&document.id, Some(&parent.section_id))?;
        let index = self.ordered_insert_index(&siblings, position.as_ref())?;
        let now = Utc::now();
        let new_section_id = SectionId::new_v7();
        siblings.insert(
            index,
            Section {
                section_id: new_section_id.clone(),
                document_id: document.id.clone(),
                parent_id: Some(parent.section_id.clone()),
                children: Vec::new(),
                title,
                level: parent.level.saturating_add(1).clamp(1, 6),
                content,
                ordinal: index as u32,
                current_version: VersionId::new_v7(),
                metadata: crate::document::SectionMetadata {
                    anchor: None,
                    tags: Vec::new(),
                    summary: None,
                    locked: false,
                },
                embedding: None,
                created_at: now,
                updated_at: now,
            },
        );
        parent.children = siblings
            .iter()
            .map(|sibling| sibling.section_id.clone())
            .collect();
        self.renumber_siblings(&mut siblings)?;
        self.store().put_section(&parent)?;
        let mut section = siblings
            .into_iter()
            .find(|sibling| sibling.section_id == new_section_id)
            .ok_or_else(|| mcp_error(McpErrorCode::Internal, "new section missing after insert"))?;
        self.save_section_version(&mut section, &Self::default_options("Created section"))?;
        self.touch_document(document)?;
        Ok(self.section_info(section))
    }

    fn descendant_ids(
        &self,
        document_id: &DocumentId,
        section: &Section,
    ) -> McpResult<Vec<SectionId>> {
        let mut ids = Vec::new();
        for child in self.store().list_child_sections(document_id, Some(&section.section_id))?
        {
            ids.push(child.section_id.clone());
            ids.extend(self.descendant_ids(document_id, &child)?);
        }
        Ok(ids)
    }

    fn find_word_boundary(&self, content: &str, position: usize) -> usize {
        // If position is at the end or start, return as-is
        if position == 0 || position >= content.len() {
            return position;
        }

        let bytes = content.as_bytes();

        // First, check if we're near a markdown heading (## or ###, etc.)
        // Search backward to find the start of the line
        let mut line_start = position;
        while line_start > 0 && bytes[line_start - 1] != b'\n' {
            line_start -= 1;
        }

        // Check if this line or nearby lines start with markdown heading
        // Search forward for the next heading (up to 200 chars)
        let search_end = (position + 200).min(content.len());
        for i in position..search_end {
            if i == 0 || (i > 0 && bytes[i - 1] == b'\n') {
                // At start of a line, check if it's a heading
                if i + 1 < bytes.len() && bytes[i] == b'#' {
                    // Found a heading, split here
                    return i;
                }
            }
        }

        // If no heading found forward, search backward (up to 200 chars)
        let search_start = position.saturating_sub(200);
        for i in (search_start..position).rev() {
            if i == 0 || (i > 0 && bytes[i - 1] == b'\n') {
                // At start of a line, check if it's a heading
                if i + 1 < bytes.len() && bytes[i] == b'#' {
                    // Found a heading, split here
                    return i;
                }
            }
        }

        // No markdown heading found, fall back to word boundary logic
        let is_in_word = position > 0
            && position < bytes.len()
            && !bytes[position].is_ascii_whitespace()
            && !bytes[position - 1].is_ascii_whitespace();

        if !is_in_word {
            // Already at a boundary
            return position;
        }

        // Search forward for the next whitespace (up to 50 chars)
        let search_end = (position + 50).min(content.len());
        for i in position..search_end {
            if bytes[i].is_ascii_whitespace() {
                // Return position after the whitespace
                let mut pos = i + 1;
                // Skip multiple whitespaces
                while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                return pos;
            }
        }

        // If no boundary found forward, search backward (up to 50 chars)
        let search_start = position.saturating_sub(50);
        for i in (search_start..position).rev() {
            if bytes[i].is_ascii_whitespace() {
                // Return position after the whitespace
                let mut pos = i + 1;
                // Skip multiple whitespaces
                while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                return pos;
            }
        }

        // If still no boundary found, return original position
        position
    }

    fn apply_level_delta(
        &self,
        document_id: &DocumentId,
        section: &mut Section,
        delta: i16,
    ) -> McpResult<()> {
        let level = (section.level as i16 + delta).clamp(1, 6) as u8;
        section.level = level;
        self.store().put_section(section)?;
        for mut child in self
            .store()
            .list_child_sections(document_id, Some(&section.section_id))?
        {
            self.apply_level_delta(document_id, &mut child, delta)?;
        }
        Ok(())
    }

    fn structured_diff(&self, left: String, right: String, old: &str, new: &str) -> DiffResult {
        let lines = if old == new {
            old.lines()
                .map(|text| DiffLine {
                    kind: DiffLineKind::Context,
                    text: text.to_owned(),
                })
                .collect()
        } else {
            old.lines()
                .map(|text| DiffLine {
                    kind: DiffLineKind::Removed,
                    text: text.to_owned(),
                })
                .chain(new.lines().map(|text| DiffLine {
                    kind: DiffLineKind::Added,
                    text: text.to_owned(),
                }))
                .collect()
        };
        DiffResult {
            left,
            right,
            format: DiffFormat::Structured,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: old.lines().count(),
                new_start: 1,
                new_lines: new.lines().count(),
                lines,
            }],
        }
    }

    fn snapshot_markdown(&self, snapshot: &DocumentSnapshot) -> String {
        let Some(root) = snapshot
            .sections
            .iter()
            .find(|section| section.parent_id.is_none())
        else {
            return String::new();
        };
        let mut markdown = String::new();
        if !root.content.trim().is_empty() {
            markdown.push_str(root.content.trim_end());
            markdown.push('\n');
        }
        self.append_snapshot_children(snapshot, Some(&root.section_id), &mut markdown);
        markdown
    }

    fn append_snapshot_children(
        &self,
        snapshot: &DocumentSnapshot,
        parent: Option<&SectionId>,
        markdown: &mut String,
    ) {
        let mut children = snapshot
            .sections
            .iter()
            .filter(|section| section.parent_id.as_ref() == parent)
            .collect::<Vec<_>>();
        children.sort_by_key(|section| section.ordinal);
        for section in children {
            if !markdown.is_empty() && !markdown.ends_with("\n\n") {
                if !markdown.ends_with('\n') {
                    markdown.push('\n');
                }
                markdown.push('\n');
            }
            markdown.push_str(&"#".repeat(section.level.clamp(1, 6) as usize));
            markdown.push(' ');
            markdown.push_str(&section.title);
            markdown.push_str("\n\n");
            if !section.content.trim().is_empty() {
                markdown.push_str(section.content.trim_end());
                markdown.push('\n');
            }
            self.append_snapshot_children(snapshot, Some(&section.section_id), markdown);
        }
    }
}

impl VdsMcpSurface for VdsServer {
    fn list_documents(&self, _params: ListDocumentsParams) -> McpResult<Vec<DocumentInfo>> {
        Ok(self.store().list_documents()?
            .into_iter()
            .map(Self::document_info)
            .collect())
    }

    fn create_document(&self, params: CreateDocumentParams) -> McpResult<DocumentInfo> {
        let CreateDocumentParams {
            name,
            title,
            initial_content,
        } = params;
        let import_name = title.clone().unwrap_or_else(|| name.clone());
        let mut document = import_markdown_str(
            &*self.store(),
            import_name,
            None,
            initial_content.as_deref().unwrap_or_default(),
        )?;
        document.name = name;
        document.metadata.title = title;
        self.store().put_document(&document)?;
        Ok(Self::document_info(document))
    }

    fn import_document(&self, params: ImportDocumentParams) -> McpResult<DocumentInfo> {
        Ok(Self::document_info(import_markdown_file(
            &*self.store(),
            params.name,
            params.path,
        )?))
    }

    fn export_document(&self, params: ExportDocumentParams) -> McpResult<ExportResult> {
        let bytes_written = export_markdown_file(&*self.store(), &params.document_id, &params.path)?;
        Ok(ExportResult {
            document_id: params.document_id,
            path: params.path,
            bytes_written,
        })
    }

    fn get_document(&self, params: GetDocumentParams) -> McpResult<Document> {
        self.require_document(&params.document_id)
    }

    fn delete_document(&self, params: DeleteDocumentParams) -> McpResult<DeleteResult> {
        let deleted = self.store().delete_document(&params.document_id)?;
        let (sections_deleted, versions_deleted, snapshots_deleted) = deleted.unwrap_or((0, 0, 0));
        Ok(DeleteResult {
            document_id: params.document_id,
            deleted: deleted.is_some(),
            sections_deleted,
            versions_deleted,
            snapshots_deleted,
        })
    }

    fn rename_document(&self, params: RenameDocumentParams) -> McpResult<DocumentInfo> {
        let mut document = self.require_document(&params.document_id)?;
        document.name = params.name;
        document.updated_at = Utc::now();
        self.store().put_document(&document)?;
        Ok(Self::document_info(document))
    }

    fn table_of_contents(
        &self,
        params: TableOfContentsParams,
    ) -> McpResult<Vec<TableOfContentsEntry>> {
        let document = self.require_document(&params.document_id)?;
        self.build_toc(&params.document_id, &document.root)
    }

    fn get_section(&self, params: GetSectionParams) -> McpResult<Section> {
        self.require_section(&params.document_id, &params.section_id)
    }

    fn get_section_tree(&self, params: GetSectionTreeParams) -> McpResult<SectionTree> {
        let section = self.require_section(&params.document_id, &params.section_id)?;
        self.build_tree(&params.document_id, section, params.depth)
    }

    fn get_sections(&self, params: GetSectionsParams) -> McpResult<Vec<Section>> {
        params
            .section_ids
            .iter()
            .map(|section_id| self.require_section(&params.document_id, section_id))
            .collect()
    }

    fn render_section_markdown(&self, params: RenderSectionMarkdownParams) -> McpResult<String> {
        Ok(render_section_markdown_string(
            &*self.store(),
            &params.document_id,
            &params.section_id,
            params.include_children,
        )?)
    }

    fn render_document_markdown(&self, params: RenderDocumentMarkdownParams) -> McpResult<String> {
        Ok(export_markdown_string(&*self.store(), &params.document_id)?)
    }

    fn create_section(&self, params: CreateSectionParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let parent = self.child_parent(&document, params.parent_id)?;
        self.create_child_section(
            &mut document,
            parent,
            params.title,
            params.content,
            params.position,
        )
    }

    fn insert_section_before(&self, params: InsertSectionBeforeParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let sibling = self.require_section(&params.document_id, &params.sibling_section_id)?;
        let parent = self.child_parent(&document, sibling.parent_id.clone())?;
        self.create_child_section(
            &mut document,
            parent,
            params.title,
            params.content,
            Some(InsertPosition::Before(params.sibling_section_id)),
        )
    }

    fn insert_section_after(&self, params: InsertSectionAfterParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let sibling = self.require_section(&params.document_id, &params.sibling_section_id)?;
        let parent = self.child_parent(&document, sibling.parent_id.clone())?;
        self.create_child_section(
            &mut document,
            parent,
            params.title,
            params.content,
            Some(InsertPosition::After(params.sibling_section_id)),
        )
    }

    fn split_section(&self, params: SplitSectionParams) -> McpResult<SplitSectionResult> {
        let mut document = self.require_document(&params.document_id)?;
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        if section.metadata.locked {
            return Err(mcp_error(McpErrorCode::Locked, "section is locked"));
        }
        if !section.content.is_char_boundary(params.split_at) {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "split_at must be a valid UTF-8 boundary",
            ));
        }

        // Find the nearest word boundary to avoid splitting mid-word
        let split_pos = self.find_word_boundary(&section.content, params.split_at);
        let tail = section.content[split_pos..].to_owned();
        section.content.truncate(split_pos);
        self.save_section_version(&mut section, &Self::default_options("Split section"))?;
        let parent = self.child_parent(&document, section.parent_id.clone())?;
        let created = self.create_child_section(
            &mut document,
            parent,
            params.new_title,
            tail,
            Some(InsertPosition::After(section.section_id.clone())),
        )?;
        Ok(SplitSectionResult {
            original: self.section_info(section),
            created,
        })
    }

    fn update_section(&self, params: UpdateSectionParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        self.ensure_editable(&section, &params.options)?;
        section.content = params.content;
        self.save_section_version(&mut section, &params.options)?;
        self.touch_document(&mut document)?;
        Ok(self.section_info(section))
    }

    fn patch_section(&self, params: PatchSectionParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        self.ensure_editable(&section, &params.options)?;
        for operation in params.patch.operations {
            match operation {
                PatchOp::ReplaceContent { content } => section.content = content,
                PatchOp::AppendContent { content } => section.content.push_str(&content),
                PatchOp::PrependContent { content } => section.content.insert_str(0, &content),
                PatchOp::ReplaceRange {
                    start,
                    end,
                    content,
                } => {
                    if start > end
                        || end > section.content.len()
                        || !section.content.is_char_boundary(start)
                        || !section.content.is_char_boundary(end)
                    {
                        return Err(mcp_error(
                            McpErrorCode::InvalidInput,
                            "patch range must be valid UTF-8 byte offsets",
                        ));
                    }
                    section.content.replace_range(start..end, &content);
                }
                PatchOp::Rename { title } => section.title = title,
                PatchOp::SetMetadata { metadata } => section.metadata = metadata,
            }
        }
        self.save_section_version(&mut section, &params.options)?;
        self.touch_document(&mut document)?;
        Ok(self.section_info(section))
    }

    fn append_to_section(&self, params: AppendToSectionParams) -> McpResult<SectionInfo> {
        self.patch_section(PatchSectionParams {
            document_id: params.document_id,
            section_id: params.section_id,
            patch: crate::document::SectionPatch {
                operations: vec![PatchOp::AppendContent {
                    content: params.content,
                }],
            },
            options: params.options,
        })
    }

    fn rename_section(&self, params: RenameSectionParams) -> McpResult<SectionInfo> {
        self.patch_section(PatchSectionParams {
            document_id: params.document_id,
            section_id: params.section_id,
            patch: crate::document::SectionPatch {
                operations: vec![PatchOp::Rename {
                    title: params.new_title,
                }],
            },
            options: params.options,
        })
    }

    fn set_section_metadata(&self, params: SetSectionMetadataParams) -> McpResult<SectionInfo> {
        self.patch_section(PatchSectionParams {
            document_id: params.document_id,
            section_id: params.section_id,
            patch: crate::document::SectionPatch {
                operations: vec![PatchOp::SetMetadata {
                    metadata: params.metadata,
                }],
            },
            options: params.options,
        })
    }

    fn move_section(&self, params: MoveSectionParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        self.ensure_editable(&section, &params.options)?;
        if section.parent_id.is_none() {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "root section cannot be moved",
            ));
        }
        let mut new_parent = self.child_parent(&document, params.new_parent_id)?;
        if new_parent.section_id == section.section_id
            || self
                .descendant_ids(&params.document_id, &section)?
                .iter()
                .any(|id| id == &new_parent.section_id)
        {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "section cannot be moved under itself or its descendants",
            ));
        }
        let mut old_parent = self.child_parent(&document, section.parent_id.clone())?;
        let mut old_siblings = self.store().list_child_sections(&params.document_id, Some(&old_parent.section_id))?
            .into_iter()
            .filter(|sibling| sibling.section_id != section.section_id)
            .collect::<Vec<_>>();
        old_parent.children = old_siblings
            .iter()
            .map(|sibling| sibling.section_id.clone())
            .collect();
        self.renumber_siblings(&mut old_siblings)?;
        self.store().put_section(&old_parent)?;

        let mut new_siblings = self.store().list_child_sections(&params.document_id, Some(&new_parent.section_id))?
            .into_iter()
            .filter(|sibling| sibling.section_id != section.section_id)
            .collect::<Vec<_>>();
        let index = self.ordered_insert_index(&new_siblings, params.position.as_ref())?;
        let old_level = section.level;
        section.parent_id = Some(new_parent.section_id.clone());
        section.level = new_parent.level.saturating_add(1).clamp(1, 6);
        new_siblings.insert(index, section.clone());
        new_parent.children = new_siblings
            .iter()
            .map(|sibling| sibling.section_id.clone())
            .collect();
        self.renumber_siblings(&mut new_siblings)?;
        self.store().put_section(&new_parent)?;
        let delta = section.level as i16 - old_level as i16;
        self.apply_level_delta(&params.document_id, &mut section, delta)?;
        self.save_section_version(&mut section, &params.options)?;
        self.touch_document(&mut document)?;
        Ok(self.section_info(section))
    }

    fn remove_section(&self, params: RemoveSectionParams) -> McpResult<RemovedSectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let section = self.require_section(&params.document_id, &params.section_id)?;
        self.ensure_editable(&section, &params.options)?;
        if section.parent_id.is_none() {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "root section cannot be removed",
            ));
        }
        if !params.remove_children && !section.children.is_empty() {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "section has children; set remove_children to true",
            ));
        }
        let mut parent = self.child_parent(&document, section.parent_id.clone())?;
        let mut siblings = self.store().list_child_sections(&params.document_id, Some(&parent.section_id))?
            .into_iter()
            .filter(|sibling| sibling.section_id != section.section_id)
            .collect::<Vec<_>>();
        parent.children = siblings
            .iter()
            .map(|sibling| sibling.section_id.clone())
            .collect();
        self.renumber_siblings(&mut siblings)?;
        self.store().put_section(&parent)?;

        let removed_children = self.descendant_ids(&params.document_id, &section)?;
        let mut to_remove = vec![section.section_id.clone()];
        to_remove.extend(removed_children.clone());
        self.store().delete_sections(&to_remove)?;
        self.touch_document(&mut document)?;
        Ok(RemovedSectionInfo {
            section_id: params.section_id,
            parent_id: section.parent_id,
            removed_children,
        })
    }

    fn reorder_sections(&self, params: ReorderSectionsParams) -> McpResult<Vec<SectionInfo>> {
        let mut document = self.require_document(&params.document_id)?;
        let mut parent = self.child_parent(&document, params.parent_id)?;
        let mut current = self.store().list_child_sections(&params.document_id, Some(&parent.section_id))?;
        let mut current_ids = current
            .iter()
            .map(|section| section.section_id.clone())
            .collect::<Vec<_>>();
        let mut ordered = params.ordered_children.clone();
        current_ids.sort();
        ordered.sort();
        if current_ids != ordered {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "ordered_children must exactly match the parent's current children",
            ));
        }
        let mut reordered = Vec::new();
        for section_id in params.ordered_children {
            let index = current
                .iter()
                .position(|section| section.section_id == section_id)
                .unwrap();
            reordered.push(current.remove(index));
        }
        parent.children = reordered
            .iter()
            .map(|section| section.section_id.clone())
            .collect();
        self.renumber_siblings(&mut reordered)?;
        self.store().put_section(&parent)?;
        self.touch_document(&mut document)?;
        Ok(reordered
            .into_iter()
            .map(|section| self.section_info(section))
            .collect())
    }

    fn promote_section(&self, params: PromoteSectionParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        self.ensure_editable(&section, &params.options)?;
        if section.level <= 1 {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "section is already level 1",
            ));
        }

        // When promoting, move the section to be a sibling of its current parent
        if let Some(parent_id) = section.parent_id.clone() {
            let parent = self.require_section(&params.document_id, &parent_id)?;

            if let Some(grandparent_id) = parent.parent_id.clone() {
                // Remove from current parent's children
                let mut old_parent = parent.clone();
                old_parent.children.retain(|id| id != &section.section_id);
                self.store().put_section(&old_parent)?;

                // Add to grandparent's children (after the parent)
                let mut grandparent = self.require_section(&params.document_id, &grandparent_id)?;
                let parent_index = grandparent
                    .children
                    .iter()
                    .position(|id| id == &parent_id)
                    .unwrap_or(grandparent.children.len());
                grandparent
                    .children
                    .insert(parent_index + 1, section.section_id.clone());
                self.store().put_section(&grandparent)?;

                // Update section's parent and level
                section.parent_id = Some(grandparent_id.clone());
                section.level = parent.level;

                // Update all descendants' levels
                self.apply_level_delta(&params.document_id, &mut section, -1)?;

                // Renumber siblings in old parent
                let mut old_siblings = self.store().list_child_sections(&params.document_id, Some(&parent_id))?;
                self.renumber_siblings(&mut old_siblings)?;

                // Renumber siblings in grandparent
                let mut new_siblings = self.store().list_child_sections(&params.document_id, Some(&grandparent_id))?;
                self.renumber_siblings(&mut new_siblings)?;
            } else {
                // Parent is root, just decrease level without changing parent
                self.apply_level_delta(&params.document_id, &mut section, -1)?;
            }
        } else {
            // No parent (shouldn't happen if level > 1), just decrease level
            self.apply_level_delta(&params.document_id, &mut section, -1)?;
        }

        self.save_section_version(&mut section, &params.options)?;
        self.touch_document(&mut document)?;
        Ok(self.section_info(section))
    }

    fn demote_section(&self, params: DemoteSectionParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        self.ensure_editable(&section, &params.options)?;
        if section.level >= 6 {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "section is already level 6",
            ));
        }

        // When demoting, we need to update parent-child relationships
        // The section should become a child of its previous sibling if one exists
        if let Some(parent_id) = section.parent_id.clone() {
            let siblings = self.store().list_child_sections(&params.document_id, Some(&parent_id))?;

            // Find the previous sibling (one with ordinal just before this section)
            if let Some(prev_sibling) = siblings
                .iter()
                .filter(|s| s.ordinal < section.ordinal && s.section_id != section.section_id)
                .max_by_key(|s| s.ordinal)
            {
                // Move this section to be a child of the previous sibling
                let mut prev_sibling = prev_sibling.clone();
                let mut old_parent = self.require_section(&params.document_id, &parent_id)?;

                // Remove from old parent's children
                old_parent.children.retain(|id| id != &section.section_id);
                self.store().put_section(&old_parent)?;

                // Add to new parent's children
                prev_sibling.children.push(section.section_id.clone());
                self.store().put_section(&prev_sibling)?;

                // Update section's parent and level
                let old_level = section.level;
                section.parent_id = Some(prev_sibling.section_id.clone());
                section.level = prev_sibling.level.saturating_add(1).clamp(1, 6);

                // Calculate the actual level change and apply to descendants
                let level_delta = section.level as i16 - old_level as i16;
                if level_delta != 0 {
                    self.store().put_section(&section)?;
                    for mut child in self.store().list_child_sections(&params.document_id, Some(&section.section_id))?
                    {
                        self.apply_level_delta(&params.document_id, &mut child, level_delta)?;
                    }
                }

                // Renumber siblings in old parent
                let mut remaining_siblings = self.store().list_child_sections(&params.document_id, Some(&parent_id))?;
                self.renumber_siblings(&mut remaining_siblings)?;
            } else {
                // No previous sibling, just increase level without changing parent
                self.apply_level_delta(&params.document_id, &mut section, 1)?;
            }
        } else {
            // Root section, just increase level
            self.apply_level_delta(&params.document_id, &mut section, 1)?;
        }

        self.save_section_version(&mut section, &params.options)?;
        self.touch_document(&mut document)?;
        Ok(self.section_info(section))
    }

    fn section_versions(
        &self,
        params: SectionVersionsParams,
    ) -> McpResult<Vec<SectionVersionInfo>> {
        self.require_section(&params.document_id, &params.section_id)?;
        Ok(self.store().list_section_versions(&params.section_id)?
            .into_iter()
            .map(|version| SectionVersionInfo {
                version_id: version.version_id,
                section_id: version.section_id,
                created_at: version.created_at,
                author: version.author,
                change_summary: version.change_summary,
            })
            .collect())
    }

    fn get_section_version(&self, params: GetSectionVersionParams) -> McpResult<SectionVersion> {
        self.require_section(&params.document_id, &params.section_id)?;
        let version = self.store().get_section_version(&params.version_id)?
            .ok_or_else(|| {
                mcp_error(
                    McpErrorCode::NotFound,
                    format!("section version not found: {}", params.version_id.as_str()),
                )
            })?;
        if version.section_id != params.section_id {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "version does not belong to section",
            ));
        }
        Ok(version)
    }

    fn switch_section_version(&self, params: SwitchSectionVersionParams) -> McpResult<SectionInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        self.ensure_editable(&section, &params.options)?;
        let version = self.get_section_version(GetSectionVersionParams {
            document_id: params.document_id.clone(),
            section_id: params.section_id,
            version_id: params.version_id,
        })?;
        section.title = version.title;
        section.content = version.content;
        section.metadata = version.metadata;
        section.embedding = version.embedding;
        self.save_section_version(&mut section, &params.options)?;
        self.touch_document(&mut document)?;
        Ok(self.section_info(section))
    }

    fn diff_section_versions(&self, params: DiffSectionVersionsParams) -> McpResult<DiffResult> {
        self.require_section(&params.document_id, &params.section_id)?;
        let from = self.get_section_version(GetSectionVersionParams {
            document_id: params.document_id.clone(),
            section_id: params.section_id.clone(),
            version_id: params.from_version.clone(),
        })?;
        let to = self.get_section_version(GetSectionVersionParams {
            document_id: params.document_id,
            section_id: params.section_id,
            version_id: params.to_version.clone(),
        })?;
        Ok(self.structured_diff(
            params.from_version.as_str().to_owned(),
            params.to_version.as_str().to_owned(),
            &format!("{}\n{}", from.title, from.content),
            &format!("{}\n{}", to.title, to.content),
        ))
    }

    fn create_document_snapshot(
        &self,
        params: CreateDocumentSnapshotParams,
    ) -> McpResult<crate::document::DocumentSnapshot> {
        let document = self.require_document(&params.document_id)?;
        let snapshot = DocumentSnapshot {
            snapshot_id: crate::document::SnapshotId::new_v7(),
            document_id: params.document_id.clone(),
            root_version: document.current_version,
            sections: self.store().list_document_sections(&params.document_id)?,
            label: params.label,
            created_at: Utc::now(),
            author: None,
            change_summary: params.change_summary,
        };
        self.store().put_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    fn document_snapshots(
        &self,
        params: DocumentSnapshotsParams,
    ) -> McpResult<Vec<DocumentSnapshotInfo>> {
        self.require_document(&params.document_id)?;
        Ok(self.store().list_document_snapshots(&params.document_id)?
            .into_iter()
            .map(|snapshot| DocumentSnapshotInfo {
                snapshot_id: snapshot.snapshot_id,
                document_id: snapshot.document_id,
                label: snapshot.label,
                created_at: snapshot.created_at,
                author: snapshot.author,
                change_summary: snapshot.change_summary,
            })
            .collect())
    }

    fn restore_document_snapshot(
        &self,
        params: RestoreDocumentSnapshotParams,
    ) -> McpResult<DocumentInfo> {
        let mut document = self.require_document(&params.document_id)?;
        let snapshot = self.store().get_snapshot(&params.snapshot_id)?
            .ok_or_else(|| mcp_error(McpErrorCode::NotFound, "document snapshot not found"))?;
        if snapshot.document_id != params.document_id {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "snapshot does not belong to document",
            ));
        }
        if snapshot.sections.is_empty() {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "snapshot does not contain captured sections",
            ));
        }
        let current_ids = self.store().list_document_sections(&params.document_id)?
            .into_iter()
            .map(|section| section.section_id)
            .collect::<Vec<_>>();
        self.store().delete_sections(&current_ids)?;
        for section in &snapshot.sections {
            self.store().put_section(section)?;
        }
        document.current_version = snapshot.root_version;
        document.updated_at = Utc::now();
        self.store().put_document(&document)?;
        Ok(Self::document_info(document))
    }

    fn diff_document_snapshots(
        &self,
        params: DiffDocumentSnapshotsParams,
    ) -> McpResult<DiffResult> {
        self.require_document(&params.document_id)?;
        let from = self.store().get_snapshot(&params.from_snapshot)?
            .ok_or_else(|| mcp_error(McpErrorCode::NotFound, "from snapshot not found"))?;
        let to = self.store().get_snapshot(&params.to_snapshot)?
            .ok_or_else(|| mcp_error(McpErrorCode::NotFound, "to snapshot not found"))?;
        if from.document_id != params.document_id || to.document_id != params.document_id {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "both snapshots must belong to the requested document",
            ));
        }
        Ok(self.structured_diff(
            params.from_snapshot.as_str().to_owned(),
            params.to_snapshot.as_str().to_owned(),
            &self.snapshot_markdown(&from),
            &self.snapshot_markdown(&to),
        ))
    }

    fn search_sections(&self, params: SearchSectionsParams) -> McpResult<Vec<SectionSearchResult>> {
        let query = params.query.to_lowercase();
        let query_terms = search_terms(&query);
        let options = params.options.unwrap_or_default();
        let include_content = options.include_content;
        let include_titles = options.include_titles;
        let max_results = options.max_results.unwrap_or(50).max(1) as usize;
        let mut results = self.store().list_document_sections(&params.document_id)?
            .into_iter()
            .filter_map(|section| {
                let title = section.title.to_lowercase();
                let content = section.content.to_lowercase();
                let title_match =
                    include_titles && text_matches_query(&title, &query, &query_terms);
                let content_match =
                    include_content && text_matches_query(&content, &query, &query_terms);
                if !(title_match || content_match) {
                    return None;
                }
                let mut matches = Vec::new();
                if content_match {
                    if let Some((start, end)) = first_query_match(&content, &query, &query_terms) {
                        matches.push(TextMatch {
                            start,
                            end,
                            snippet: snippet(&section.content, start, end),
                        });
                    }
                }
                Some(SectionSearchResult {
                    section: self.section_info(section),
                    score: if title_match { 1.0 } else { 0.5 },
                    title_match,
                    content_matches: matches,
                })
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(max_results);
        Ok(results)
    }

    #[cfg(feature = "semantic-search")]
    fn semantic_search_sections(
        &self,
        params: SemanticSearchSectionsParams,
    ) -> McpResult<Vec<SectionSearchResult>> {
        let query = semantic_query_embedding(&params)?;
        if query.vector.is_empty() {
            return Err(mcp_error(
                McpErrorCode::InvalidInput,
                "query embedding must not be empty",
            ));
        }

        let options = params.options.unwrap_or_default();
        let max_results = options.max_results.unwrap_or(10).max(1) as usize;
        let ef = options.ef.unwrap_or(max_results.saturating_mul(2).max(50));
        let mut index = hnsw_vector_search::HnswGraph::new(
            options.m.unwrap_or(16),
            options.ef_construction.unwrap_or(200),
        );
        let mut sections_by_node = Vec::new();

        for section in self.store().list_document_sections(&params.document_id)? {
            let Some(embedding) = &section.embedding else {
                continue;
            };
            if embedding.vector.len() != query.vector.len() {
                continue;
            }
            if options.require_same_model && embedding.model != query.model {
                continue;
            }

            let node_id = index.insert(embedding.vector.clone());
            if sections_by_node.len() <= node_id {
                sections_by_node.resize_with(node_id + 1, || None);
            }
            sections_by_node[node_id] = Some(section);
        }

        if index.is_empty() {
            return Ok(Vec::new());
        }

        Ok(index
            .search(&query.vector, max_results, ef.max(max_results))
            .into_iter()
            .filter_map(|result| {
                let node_id = semantic_result_node_id(&result);
                let distance = semantic_result_distance(&result);
                sections_by_node
                    .get(node_id)
                    .and_then(|section| section.clone())
                    .map(|section| SectionSearchResult {
                        section: self.section_info(section),
                        score: 1.0 / (1.0 + distance),
                        title_match: false,
                        content_matches: Vec::new(),
                    })
            })
            .collect())
    }

    fn find_by_title(&self, params: FindByTitleParams) -> McpResult<Vec<SectionSearchResult>> {
        let query = params.title.to_lowercase();
        Ok(self.store().list_document_sections(&params.document_id)?
            .into_iter()
            .filter_map(|section| {
                let title = section.title.to_lowercase();
                let matched = if params.fuzzy {
                    title.contains(&query) || query.contains(&title)
                } else {
                    title == query
                };
                matched.then(|| SectionSearchResult {
                    section: self.section_info(section),
                    score: 1.0,
                    title_match: true,
                    content_matches: Vec::new(),
                })
            })
            .collect())
    }

    fn find_by_tag(&self, params: FindByTagParams) -> McpResult<Vec<SectionInfo>> {
        Ok(self.store().list_document_sections(&params.document_id)?
            .into_iter()
            .filter(|section| section.metadata.tags.iter().any(|tag| tag == &params.tag))
            .map(|section| self.section_info(section))
            .collect())
    }

    fn list_recent_changes(&self, params: ListRecentChangesParams) -> McpResult<Vec<ChangeRecord>> {
        self.require_document(&params.document_id)?;
        let mut changes = Vec::new();
        for section in self.store().list_document_sections(&params.document_id)? {
            for version in self.store().list_section_versions(&section.section_id)? {
                changes.push(ChangeRecord {
                    revision_id: version.version_id.as_str().to_owned(),
                    document_id: params.document_id.clone(),
                    section_id: Some(version.section_id.clone()),
                    version_id: Some(version.version_id.clone()),
                    snapshot_id: None,
                    change_kind: ChangeKind::SectionUpdated,
                    created_at: version.created_at,
                    author: version.author,
                    change_summary: version.change_summary,
                });
            }
        }
        for snapshot in self.store().list_document_snapshots(&params.document_id)? {
            changes.push(ChangeRecord {
                revision_id: snapshot.snapshot_id.as_str().to_owned(),
                document_id: params.document_id.clone(),
                section_id: None,
                version_id: None,
                snapshot_id: Some(snapshot.snapshot_id),
                change_kind: ChangeKind::SnapshotCreated,
                created_at: snapshot.created_at,
                author: snapshot.author,
                change_summary: snapshot.change_summary,
            });
        }
        changes.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        if let Some(limit) = params.limit {
            changes.truncate(limit as usize);
        }
        Ok(changes)
    }

    fn validate_document(
        &self,
        params: ValidateDocumentParams,
    ) -> McpResult<Vec<ValidationDiagnostic>> {
        self.require_document(&params.document_id)?;
        Ok(Vec::new())
    }

    fn normalize_document(&self, params: NormalizeDocumentParams) -> McpResult<NormalizeResult> {
        self.require_document(&params.document_id)?;
        Ok(NormalizeResult {
            document_id: params.document_id,
            changed: false,
            diagnostics: Vec::new(),
        })
    }

    fn repair_document(&self, params: RepairDocumentParams) -> McpResult<RepairResult> {
        self.require_document(&params.document_id)?;
        Ok(RepairResult {
            document_id: params.document_id,
            repaired: false,
            diagnostics: Vec::new(),
        })
    }

    fn lock_section(&self, params: LockSectionParams) -> McpResult<LockInfo> {
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        if section.metadata.locked {
            return Err(mcp_error(McpErrorCode::Locked, "section is already locked"));
        }
        section.metadata.locked = true;
        section.updated_at = Utc::now();
        self.store().put_section(&section)?;
        Ok(LockInfo {
            document_id: params.document_id,
            section_id: params.section_id,
            owner: params.owner,
            expires_at: params
                .ttl_seconds
                .map(|ttl| Utc::now() + chrono::Duration::seconds(ttl as i64)),
        })
    }

    fn unlock_section(&self, params: UnlockSectionParams) -> McpResult<UnlockResult> {
        let mut section = self.require_section(&params.document_id, &params.section_id)?;
        let unlocked = section.metadata.locked;
        section.metadata.locked = false;
        section.updated_at = Utc::now();
        self.store().put_section(&section)?;
        Ok(UnlockResult {
            document_id: params.document_id,
            section_id: params.section_id,
            unlocked,
        })
    }

    fn check_conflicts(&self, params: CheckConflictsParams) -> McpResult<ConflictCheckResult> {
        let section = self.require_section(&params.document_id, &params.section_id)?;
        Ok(ConflictCheckResult {
            document_id: params.document_id,
            section_id: params.section_id,
            expected_version: params.expected_version.clone(),
            current_version: section.current_version.clone(),
            conflicted: params.expected_version != section.current_version,
        })
    }

    fn set_workspace(&self, params: SetWorkspaceParams) -> McpResult<WorkspaceInfo> {
        let workspace_path = PathBuf::from(&params.workspace);
        self.set_workspace_path(workspace_path).map_err(|e| {
            mcp_error(McpErrorCode::Internal, format!("Failed to set workspace: {}", e))
        })?;
        
        let database = self.get_database_path();
        Ok(WorkspaceInfo {
            workspace: self.get_workspace_path().map(|p| p.to_string_lossy().into_owned()),
            database: database.to_string_lossy().into_owned(),
        })
    }

    fn get_workspace(&self, _params: GetWorkspaceParams) -> McpResult<WorkspaceInfo> {
        let database = self.get_database_path();
        Ok(WorkspaceInfo {
            workspace: self.get_workspace_path().map(|p| p.to_string_lossy().into_owned()),
            database: database.to_string_lossy().into_owned(),
        })
    }

    fn set_database(&self, params: SetDatabaseParams) -> McpResult<DatabaseInfo> {
        let database_path = PathBuf::from(&params.database);
        self.reopen_database(database_path).map_err(|e| {
            mcp_error(McpErrorCode::Internal, format!("Failed to set database: {}", e))
        })?;
        
        let database = self.get_database_path();
        Ok(DatabaseInfo {
            database: database.to_string_lossy().into_owned(),
        })
    }

    fn get_database(&self, _params: GetDatabaseParams) -> McpResult<DatabaseInfo> {
        let database = self.get_database_path();
        Ok(DatabaseInfo {
            database: database.to_string_lossy().into_owned(),
        })
    }
}

impl ServerHandler for VdsServer {
    fn get_info(&self) -> ServerInfo {
        let docs = endpoint_documentation();
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                    .with_title(docs.name)
                    .with_description(docs.description),
            )
            .with_instructions(format!(
                "{}\n\nAvailable capabilities: tools/list and tools/call. {} tools are advertised through tools/list. {}",
                docs.description,
                docs.tools.len(),
                docs.usage
            ))
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

/// Starts the server over MCP stdio transport and waits until the client exits.
pub async fn serve_stdio(database: PathBuf) -> Result<(), ServiceError> {
    write_startup_banner("stdio", None);
    let service = VdsServer::open(database)?
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

/// Starts the server over streamable HTTP at the supplied bind address.
pub async fn serve_streamable_http(
    database: PathBuf,
    bind: String,
    path: String,
) -> Result<(), ServiceError> {
    let endpoint = format!("http://{bind}{path}");
    write_startup_banner("streamable HTTP", Some(&endpoint));
    let database = Arc::new(database);
    let service: rmcp::transport::streamable_http_server::tower::StreamableHttpService<
        VdsServer,
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
    > = rmcp::transport::streamable_http_server::tower::StreamableHttpService::new(
        move || VdsServer::open((*database).clone()).map_err(std::io::Error::other),
        Default::default(),
        rmcp::transport::StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service(&path, service);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    eprintln!("VDS MCP streamable HTTP server listening on {endpoint}");
    axum::serve(listener, router).await?;
    Ok(())
}

fn write_startup_banner(transport: &str, endpoint: Option<&str>) {
    let docs = endpoint_documentation();

    eprintln!(
        "{} v{} starting in {transport} mode",
        docs.name,
        env!("CARGO_PKG_VERSION")
    );
    if let Some(endpoint) = endpoint {
        eprintln!("Endpoint: {endpoint}");
    } else {
        eprintln!("Transport: MCP over stdio; stdout is reserved for protocol messages.");
    }
    eprintln!("{}", docs.description);
    eprintln!("Capabilities: tools/list, tools/call");
    eprintln!("Usage: {}", docs.usage);
    eprintln!("Advertised tools ({}):", docs.tools.len());
    for tool in docs.tools {
        eprintln!("  - {}: {}", tool.name, tool.description);
    }
}

fn parse<T: DeserializeOwned>(arguments: Option<JsonObject>) -> Result<T, McpError> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|error| mcp_error(McpErrorCode::InvalidInput, error.to_string()))
}

fn tool_from_doc(doc: ToolDocumentation) -> Tool {
    Tool::new(
        doc.name,
        format!("{} {}", doc.description, doc.usage),
        Arc::new(schema_object(doc.tool)),
    )
    .with_title(doc.title)
}

fn schema_object(tool: VdsTool) -> JsonObject {
    match tool_schema(tool) {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    }
}

fn tool_schema(tool: VdsTool) -> Value {
    match tool {
        VdsTool::ListDocuments => object_schema(vec![], vec![]),
        VdsTool::CreateDocument => object_schema(
            vec![string_prop("name", "Human-readable document name.")],
            vec![
                json_prop(
                    "title",
                    nullable(
                        string_schema("Optional display title."),
                        "Optional display title.",
                    ),
                ),
                json_prop(
                    "initial_content",
                    nullable(
                        string_schema(
                            "Markdown content used to create the initial section tree. Alias: markdown.",
                        ),
                        "Optional Markdown content used to create the initial section tree. Alias: markdown.",
                    ),
                ),
                json_prop(
                    "markdown",
                    nullable(
                        string_schema("Alias for initial_content."),
                        "Alias for initial_content.",
                    ),
                ),
            ],
        ),
        VdsTool::ImportDocument => object_schema(
            vec![
                string_prop("name", "Human-readable document name after import."),
                string_prop("path", "Filesystem path to the Markdown source file."),
            ],
            vec![],
        ),
        VdsTool::ExportDocument => object_schema(
            vec![
                document_id_prop(),
                string_prop("path", "Filesystem path where Markdown should be written."),
            ],
            vec![],
        ),
        VdsTool::GetDocument => object_schema(vec![document_id_prop()], vec![]),
        VdsTool::DeleteDocument => object_schema(vec![document_id_prop()], vec![]),
        VdsTool::RenameDocument => object_schema(
            vec![
                document_id_prop(),
                string_prop("name", "New human-readable document name."),
            ],
            vec![],
        ),
        VdsTool::TableOfContents => object_schema(vec![document_id_prop()], vec![]),
        VdsTool::GetSection => section_target_schema(),
        VdsTool::GetSectionTree => object_schema(
            vec![document_id_prop(), section_id_prop()],
            vec![json_prop(
                "depth",
                nullable(
                    integer_schema("Maximum descendant depth to include."),
                    "Maximum descendant depth to include.",
                ),
            )],
        ),
        VdsTool::GetSections => object_schema(
            vec![
                document_id_prop(),
                json_prop(
                    "section_ids",
                    json!({
                        "type": "array",
                        "items": id_schema("Stable section ID."),
                        "description": "Sections to read."
                    }),
                ),
            ],
            vec![],
        ),
        VdsTool::RenderSectionMarkdown => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                bool_prop(
                    "include_children",
                    "Whether descendant sections should be rendered too.",
                ),
            ],
            vec![],
        ),
        VdsTool::RenderDocumentMarkdown => object_schema(vec![document_id_prop()], vec![]),
        VdsTool::CreateSection => object_schema(
            vec![
                document_id_prop(),
                string_prop("title", "New section heading."),
                string_prop("content", "New section body."),
            ],
            vec![
                json_prop(
                    "parent_id",
                    nullable(
                        id_schema("Parent section ID. Aliases: parent, parent_section_id."),
                        "Parent section ID. Aliases: parent, parent_section_id.",
                    ),
                ),
                json_prop(
                    "parent_section_id",
                    nullable(id_schema("Alias for parent_id."), "Alias for parent_id."),
                ),
                json_prop(
                    "parent",
                    nullable(id_schema("Alias for parent_id."), "Alias for parent_id."),
                ),
                json_prop(
                    "position",
                    nullable(
                        insert_position_schema(),
                        "Optional placement among siblings.",
                    ),
                ),
            ],
        ),
        VdsTool::InsertSectionBefore => sibling_insert_schema(),
        VdsTool::InsertSectionAfter => sibling_insert_schema(),
        VdsTool::SplitSection => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                json_prop(
                    "split_at",
                    integer_schema("Byte offset in the section body where the split occurs."),
                ),
                string_prop("new_title", "Heading for the newly created section."),
            ],
            vec![],
        ),
        VdsTool::UpdateSection => {
            edit_schema(vec![string_prop("content", "Replacement section body.")])
        }
        VdsTool::PatchSection => edit_schema(vec![json_prop("patch", section_patch_schema())]),
        VdsTool::AppendToSection => edit_schema(vec![string_prop(
            "content",
            "Content to append to the section body.",
        )]),
        VdsTool::RenameSection => {
            edit_schema(vec![string_prop("new_title", "New section heading title.")])
        }
        VdsTool::SetSectionMetadata => {
            edit_schema(vec![json_prop("metadata", section_metadata_schema())])
        }
        VdsTool::MoveSection => object_schema(
            vec![document_id_prop(), section_id_prop()],
            vec![
                json_prop(
                    "new_parent_id",
                    nullable(
                        id_schema("New parent section ID. Alias: new_parent."),
                        "New parent section ID. Alias: new_parent.",
                    ),
                ),
                json_prop(
                    "new_parent",
                    nullable(
                        id_schema("Alias for new_parent_id."),
                        "Alias for new_parent_id.",
                    ),
                ),
                json_prop(
                    "position",
                    nullable(
                        insert_position_schema(),
                        "Optional placement among new siblings.",
                    ),
                ),
                options_prop(),
            ],
        ),
        VdsTool::RemoveSection => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                bool_prop(
                    "remove_children",
                    "Whether descendant sections should also be removed.",
                ),
            ],
            vec![options_prop()],
        ),
        VdsTool::ReorderSections => object_schema(
            vec![
                document_id_prop(),
                json_prop(
                    "ordered_children",
                    json!({
                        "type": "array",
                        "items": id_schema("Stable child section ID."),
                        "description": "Complete ordered child list after reordering."
                    }),
                ),
            ],
            vec![
                json_prop(
                    "parent_id",
                    nullable(
                        id_schema("Parent whose children are reordered. Alias: parent."),
                        "Parent whose children are reordered. Alias: parent.",
                    ),
                ),
                json_prop(
                    "parent",
                    nullable(id_schema("Alias for parent_id."), "Alias for parent_id."),
                ),
                options_prop(),
            ],
        ),
        VdsTool::PromoteSection | VdsTool::DemoteSection => object_schema(
            vec![document_id_prop(), section_id_prop()],
            vec![options_prop()],
        ),
        VdsTool::SectionVersions => section_target_schema(),
        VdsTool::GetSectionVersion => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                version_id_prop("version_id", "Version to read."),
            ],
            vec![],
        ),
        VdsTool::SwitchSectionVersion => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                version_id_prop("version_id", "Historical version to make current."),
            ],
            vec![options_prop()],
        ),
        VdsTool::DiffSectionVersions => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                version_id_prop("from_version", "Earlier version to compare from."),
                version_id_prop("to_version", "Later version to compare to."),
            ],
            vec![],
        ),
        VdsTool::CreateDocumentSnapshot => object_schema(
            vec![document_id_prop()],
            vec![
                json_prop(
                    "label",
                    nullable(
                        string_schema("Optional display label. Alias: name."),
                        "Optional display label. Alias: name.",
                    ),
                ),
                json_prop(
                    "name",
                    nullable(string_schema("Alias for label."), "Alias for label."),
                ),
                json_prop(
                    "change_summary",
                    nullable(
                        string_schema(
                            "Optional human-readable snapshot description. Alias: description.",
                        ),
                        "Optional human-readable snapshot description. Alias: description.",
                    ),
                ),
                json_prop(
                    "description",
                    nullable(
                        string_schema("Alias for change_summary."),
                        "Alias for change_summary.",
                    ),
                ),
            ],
        ),
        VdsTool::DocumentSnapshots => object_schema(vec![document_id_prop()], vec![]),
        VdsTool::RestoreDocumentSnapshot => object_schema(
            vec![
                document_id_prop(),
                id_prop("snapshot_id", "Snapshot to restore from."),
            ],
            vec![],
        ),
        VdsTool::DiffDocumentSnapshots => object_schema(
            vec![
                document_id_prop(),
                id_prop("from_snapshot", "Earlier snapshot to compare from."),
                id_prop("to_snapshot", "Later snapshot to compare to."),
            ],
            vec![],
        ),
        VdsTool::SearchSections => object_schema(
            vec![document_id_prop(), string_prop("query", "Search query.")],
            vec![json_prop(
                "options",
                nullable(search_options_schema(), "Search behavior options."),
            )],
        ),
        #[cfg(feature = "semantic-search")]
        VdsTool::SemanticSearchSections => object_schema(
            vec![document_id_prop()],
            vec![
                json_prop(
                    "query",
                    nullable(
                        string_schema(
                            "Natural-language query to embed when model paths are provided.",
                        ),
                        "Natural-language query to embed when model paths are provided.",
                    ),
                ),
                json_prop(
                    "query_embedding",
                    nullable(
                        json!({"type": "array", "items": {"type": "number"}, "description": "Precomputed query embedding."}),
                        "Precomputed query embedding.",
                    ),
                ),
                json_prop(
                    "model",
                    nullable(
                        object_schema(
                            vec![
                                string_prop("model_path", "Path to an ONNX embedding model."),
                                string_prop(
                                    "tokenizer_path",
                                    "Path to the tokenizer JSON for the embedding model.",
                                ),
                            ],
                            vec![],
                        ),
                        "Optional local ONNX model configuration.",
                    ),
                ),
                json_prop(
                    "options",
                    nullable(
                        semantic_search_options_schema(),
                        "Semantic search behavior options.",
                    ),
                ),
            ],
        ),
        VdsTool::FindByTitle => object_schema(
            vec![document_id_prop(), string_prop("title", "Title query.")],
            vec![bool_prop(
                "fuzzy",
                "Whether fuzzy title matching should be used.",
            )],
        ),
        VdsTool::FindByTag => object_schema(
            vec![document_id_prop(), string_prop("tag", "Tag to match.")],
            vec![],
        ),
        VdsTool::ListRecentChanges => object_schema(
            vec![document_id_prop()],
            vec![json_prop(
                "limit",
                nullable(
                    integer_schema("Maximum number of changes to return."),
                    "Maximum number of changes to return.",
                ),
            )],
        ),
        VdsTool::ValidateDocument => object_schema(vec![document_id_prop()], vec![]),
        VdsTool::NormalizeDocument => object_schema(
            vec![document_id_prop()],
            vec![json_prop(
                "options",
                nullable(
                    normalize_options_schema(),
                    "Normalization behavior options.",
                ),
            )],
        ),
        VdsTool::RepairDocument => object_schema(vec![document_id_prop()], vec![]),
        VdsTool::LockSection => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                string_prop("owner", "Lock owner identifier."),
            ],
            vec![json_prop(
                "ttl_seconds",
                nullable(
                    integer_schema("Optional lock time-to-live in seconds."),
                    "Optional lock time-to-live in seconds.",
                ),
            )],
        ),
        VdsTool::UnlockSection => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                string_prop("owner", "Lock owner identifier."),
            ],
            vec![],
        ),
        VdsTool::CheckConflicts => object_schema(
            vec![
                document_id_prop(),
                section_id_prop(),
                version_id_prop(
                    "expected_version",
                    "Version the caller expects to still be current.",
                ),
            ],
            vec![],
        ),
        VdsTool::SetWorkspace => object_schema(
            vec![string_prop(
                "workspace",
                "Workspace directory path. Database will be at <workspace>/.vds/vds.db",
            )],
            vec![],
        ),
        VdsTool::GetWorkspace => object_schema(vec![], vec![]),
        VdsTool::SetDatabase => object_schema(
            vec![string_prop("database", "Database file path.")],
            vec![],
        ),
        VdsTool::GetDatabase => object_schema(vec![], vec![]),
    }
}

fn section_target_schema() -> Value {
    object_schema(vec![document_id_prop(), section_id_prop()], vec![])
}

fn sibling_insert_schema() -> Value {
    object_schema(
        vec![
            document_id_prop(),
            id_prop(
                "sibling_section_id",
                "Existing sibling section used for placement.",
            ),
            string_prop("title", "New section heading."),
            string_prop("content", "New section body."),
        ],
        vec![],
    )
}

fn edit_schema(extra_required: Vec<(String, Value)>) -> Value {
    let mut required = vec![document_id_prop(), section_id_prop()];
    required.extend(extra_required);
    object_schema(required, vec![options_prop()])
}

fn object_schema(
    required_props: Vec<(String, Value)>,
    optional_props: Vec<(String, Value)>,
) -> Value {
    let required_names: Vec<String> = required_props
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    let mut properties = serde_json::Map::new();
    for (name, schema) in required_props.into_iter().chain(optional_props) {
        properties.insert(name, schema);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required_names,
        "additionalProperties": false
    })
}

fn document_id_prop() -> (String, Value) {
    id_prop("document_id", "Stable document ID.")
}

fn section_id_prop() -> (String, Value) {
    id_prop("section_id", "Stable section ID.")
}

fn version_id_prop(name: &str, description: &str) -> (String, Value) {
    id_prop(name, description)
}

fn id_prop(name: &str, description: &str) -> (String, Value) {
    json_prop(name, id_schema(description))
}

fn id_schema(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn string_prop(name: &str, description: &str) -> (String, Value) {
    json_prop(name, string_schema(description))
}

fn string_schema(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn bool_prop(name: &str, description: &str) -> (String, Value) {
    json_prop(
        name,
        json!({ "type": "boolean", "description": description }),
    )
}

fn integer_schema(description: &str) -> Value {
    json!({ "type": "integer", "minimum": 0, "description": description })
}

fn json_prop(name: &str, schema: Value) -> (String, Value) {
    (name.to_string(), schema)
}

fn nullable(schema: Value, description: &str) -> Value {
    json!({
        "anyOf": [
            schema,
            { "type": "null" }
        ],
        "description": description
    })
}

fn options_prop() -> (String, Value) {
    json_prop(
        "options",
        nullable(
            edit_options_schema(),
            "Edit metadata and optimistic concurrency options.",
        ),
    )
}

fn edit_options_schema() -> Value {
    object_schema(
        vec![],
        vec![
            json_prop(
                "expected_version",
                nullable(
                    id_schema("Expected current section version for optimistic concurrency."),
                    "Expected current section version for optimistic concurrency.",
                ),
            ),
            json_prop(
                "author",
                nullable(
                    string_schema("Actor responsible for the edit."),
                    "Actor responsible for the edit.",
                ),
            ),
            json_prop(
                "change_summary",
                nullable(
                    string_schema("Human-readable edit summary."),
                    "Human-readable edit summary.",
                ),
            ),
        ],
    )
}

fn section_metadata_schema() -> Value {
    object_schema(
        vec![],
        vec![
            json_prop(
                "anchor",
                nullable(
                    string_schema("Optional stable section anchor."),
                    "Optional stable section anchor.",
                ),
            ),
            json_prop(
                "tags",
                json!({
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Section tags."
                }),
            ),
            json_prop(
                "summary",
                nullable(
                    string_schema("Optional section summary."),
                    "Optional section summary.",
                ),
            ),
            bool_prop(
                "locked",
                "Whether the section is marked locked in metadata.",
            ),
        ],
    )
}

fn section_patch_schema() -> Value {
    object_schema(
        vec![json_prop(
            "operations",
            json!({
                "type": "array",
                "description": "Ordered patch operations. Use the type field form for agent-friendly calls.",
                "items": patch_operation_schema()
            }),
        )],
        vec![],
    )
}

fn patch_operation_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "type": { "const": "replace_content" },
                    "content": { "type": "string" }
                },
                "required": ["type", "content"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "enum": ["append", "append_content"] },
                    "content": { "type": "string" }
                },
                "required": ["type", "content"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "enum": ["prepend", "prepend_content"] },
                    "content": { "type": "string" }
                },
                "required": ["type", "content"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "replace_range" },
                    "start": { "type": "integer", "minimum": 0 },
                    "end": { "type": "integer", "minimum": 0 },
                    "content": { "type": "string" }
                },
                "required": ["type", "start", "end", "content"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "rename" },
                    "title": { "type": "string" }
                },
                "required": ["type", "title"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "set_metadata" },
                    "metadata": section_metadata_schema()
                },
                "required": ["type", "metadata"],
                "additionalProperties": false
            }
        ]
    })
}

fn insert_position_schema() -> Value {
    json!({
        "description": "Sibling placement. Use \"first\", \"last\", a zero-based index number, or a single-key object like {\"Before\":\"sec-...\"}, {\"After\":\"sec-...\"}, or {\"Index\":2}.",
        "oneOf": [
            { "type": "string", "enum": ["first", "last", "First", "Last"] },
            { "type": "integer", "minimum": 0 },
            {
                "type": "object",
                "properties": {
                    "Before": id_schema("Sibling section ID to insert before."),
                    "After": id_schema("Sibling section ID to insert after."),
                    "Index": { "type": "integer", "minimum": 0 }
                },
                "minProperties": 1,
                "maxProperties": 1,
                "additionalProperties": false
            }
        ]
    })
}

fn search_options_schema() -> Value {
    object_schema(
        vec![],
        vec![
            bool_prop(
                "include_content",
                "Whether section body content should be searched.",
            ),
            bool_prop(
                "include_titles",
                "Whether section titles should be searched.",
            ),
            bool_prop(
                "fuzzy_titles",
                "Whether title matching may use fuzzy matching.",
            ),
            json_prop(
                "max_results",
                nullable(
                    integer_schema("Maximum number of results to return. Alias: limit."),
                    "Maximum number of results to return. Alias: limit.",
                ),
            ),
            json_prop(
                "limit",
                nullable(
                    integer_schema("Alias for max_results."),
                    "Alias for max_results.",
                ),
            ),
        ],
    )
}

#[cfg(feature = "semantic-search")]
fn semantic_search_options_schema() -> Value {
    object_schema(
        vec![],
        vec![
            json_prop(
                "max_results",
                nullable(
                    integer_schema("Maximum number of nearest sections to return."),
                    "Maximum number of nearest sections to return.",
                ),
            ),
            json_prop(
                "ef",
                nullable(
                    integer_schema("HNSW search beam width."),
                    "HNSW search beam width.",
                ),
            ),
            json_prop(
                "m",
                nullable(
                    integer_schema("HNSW maximum connections per graph layer."),
                    "HNSW maximum connections per graph layer.",
                ),
            ),
            json_prop(
                "ef_construction",
                nullable(
                    integer_schema("HNSW construction candidate list size."),
                    "HNSW construction candidate list size.",
                ),
            ),
            bool_prop(
                "require_same_model",
                "Only search embeddings from the same model name as the query.",
            ),
        ],
    )
}

fn normalize_options_schema() -> Value {
    object_schema(
        vec![],
        vec![
            bool_prop(
                "fix_heading_levels",
                "Make heading levels structurally consistent.",
            ),
            bool_prop("regenerate_anchors", "Regenerate section anchors."),
            bool_prop("trim_whitespace", "Trim leading and trailing whitespace."),
            bool_prop("remove_empty_sections", "Remove empty sections."),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_tool_schemas_are_specific() {
        let create = tool_schema(VdsTool::CreateDocument);
        assert_eq!(create["required"], json!(["name"]));
        assert!(create["properties"].get("markdown").is_some());
        assert_eq!(create["additionalProperties"], false);

        let diff = tool_schema(VdsTool::DiffSectionVersions);
        assert_eq!(
            diff["required"],
            json!(["document_id", "section_id", "from_version", "to_version"])
        );
        assert!(diff["properties"].get("from_version_id").is_none());

        let patch = tool_schema(VdsTool::PatchSection);
        assert!(patch["properties"]["patch"]["properties"]["operations"].is_object());
        assert_eq!(patch["additionalProperties"], false);
    }
}

fn search_terms(query: &str) -> Vec<&str> {
    query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect()
}

fn text_matches_query(text: &str, query: &str, terms: &[&str]) -> bool {
    if query.trim().is_empty() {
        return false;
    }
    text.contains(query) || (!terms.is_empty() && terms.iter().all(|term| text.contains(term)))
}

fn first_query_match(text: &str, query: &str, terms: &[&str]) -> Option<(usize, usize)> {
    if let Some(start) = text.find(query) {
        return Some((start, start + query.len()));
    }
    terms
        .iter()
        .filter_map(|term| text.find(term).map(|start| (start, start + term.len())))
        .min_by_key(|(start, _)| *start)
}

fn snippet(content: &str, start: usize, end: usize) -> String {
    let prefix = start.saturating_sub(40);
    let suffix = (end + 40).min(content.len());
    content[prefix..suffix].to_owned()
}

#[cfg(feature = "semantic-search")]
fn semantic_query_embedding(
    params: &SemanticSearchSectionsParams,
) -> McpResult<crate::document::TextEmbedding> {
    if let Some(embedding) = &params.query_embedding {
        return Ok(embedding.clone());
    }

    let query = params.query.as_deref().ok_or_else(|| {
        mcp_error(
            McpErrorCode::InvalidInput,
            "semantic search requires query_embedding or query with model paths",
        )
    })?;
    let model = params.model.as_ref().ok_or_else(|| {
        mcp_error(
            McpErrorCode::InvalidInput,
            "semantic search query text requires model and tokenizer paths",
        )
    })?;

    let mut embedder =
        hnsw_vector_search::OnnxEmbedder::new(&model.model_path, &model.tokenizer_path)
            .map_err(|error| mcp_error(McpErrorCode::Internal, error.to_string()))?;
    let vector = embedder
        .embed(query)
        .map_err(|error| mcp_error(McpErrorCode::Internal, error.to_string()))?;
    Ok(crate::document::TextEmbedding {
        model: Some(model.model_path.clone()),
        vector,
    })
}

#[cfg(feature = "semantic-search")]
fn semantic_result_node_id(result: &hnsw_vector_search::hnsw::SearchResult) -> usize {
    result.0
}

#[cfg(feature = "semantic-search")]
fn semantic_result_distance(result: &hnsw_vector_search::hnsw::SearchResult) -> f32 {
    result.1
}

fn to_rmcp_error(error: McpError) -> RmcpError {
    let data = Some(json!({
        "code": error.code,
        "message": error.message,
    }));
    match error.code {
        McpErrorCode::NotFound => RmcpError::resource_not_found("VDS item not found", data),
        McpErrorCode::InvalidInput | McpErrorCode::Conflict | McpErrorCode::Locked => {
            RmcpError::invalid_params("invalid VDS MCP request", data)
        }
        McpErrorCode::Storage | McpErrorCode::Internal => {
            RmcpError::internal_error("VDS MCP service error", data)
        }
    }
}

fn mcp_error(code: McpErrorCode, message: impl Into<String>) -> McpError {
    McpError {
        code,
        message: message.into(),
    }
}

impl From<StorageError> for McpError {
    fn from(value: StorageError) -> Self {
        mcp_error(McpErrorCode::Storage, value.to_string())
    }
}

/// Error type used by CLI service startup.
#[derive(Debug)]
pub enum ServiceError {
    Io(std::io::Error),
    Storage(StorageError),
    Server(rmcp::service::ServerInitializeError),
    Runtime(tokio::task::JoinError),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Storage(error) => write!(f, "{error}"),
            Self::Server(error) => write!(f, "MCP server error: {error}"),
            Self::Runtime(error) => write!(f, "runtime error: {error}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<std::io::Error> for ServiceError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<StorageError> for ServiceError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<rmcp::service::ServerInitializeError> for ServiceError {
    fn from(value: rmcp::service::ServerInitializeError) -> Self {
        Self::Server(value)
    }
}

impl From<tokio::task::JoinError> for ServiceError {
    fn from(value: tokio::task::JoinError) -> Self {
        Self::Runtime(value)
    }
}
