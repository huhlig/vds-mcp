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

//! Git-friendly durable metadata for filesystem-authoritative VDS workspaces.
//!
//! Current Markdown remains authoritative for visible content and hierarchy.
//! This module stores the durable identity, metadata, and immutable versions
//! that cannot be reconstructed safely from Markdown alone.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::document::{
    DocumentId, DocumentSnapshot, Section, SectionId, SectionMetadata, SectionVersion, SnapshotId,
    VersionId,
};
use crate::workspace::MaterializedDocument;

/// Current on-disk metadata schema understood by this implementation.
pub const METADATA_FORMAT_VERSION: u32 = 1;

/// Workspace-wide identity and metadata schema marker.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct WorkspaceManifest {
    pub format_version: u32,
    pub workspace_id: String,
    pub created_at: DateTime<Utc>,
}

/// Stable metadata connecting one managed document to a project file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentRecord {
    pub format_version: u32,
    pub document_id: DocumentId,
    pub relative_path: String,
    /// Optional explicit display name. When set, overrides the path-derived name.
    #[serde(default)]
    pub name: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// Mutable document pointers and the source state from which they were built.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CurrentDocumentRecord {
    pub format_version: u32,
    pub content_hash: String,
    pub current_document_version: VersionId,
    pub root_section_id: SectionId,
    pub updated_at: DateTime<Utc>,
}

/// Hints used to reconnect externally edited headings to stable section IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionMatchingRecord {
    pub embedded_marker: Option<String>,
    pub last_known_title: String,
    pub last_known_ancestry: Vec<String>,
    pub last_known_ordinal: u32,
    pub content_fingerprint: String,
}

/// Current durable identity and metadata for one managed section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionRecord {
    pub format_version: u32,
    pub section_id: SectionId,
    pub current_version: VersionId,
    pub metadata: SectionMetadata,
    pub matching: SectionMatchingRecord,
    pub updated_at: DateTime<Utc>,
}

/// All current durable metadata loaded for one managed document.
#[derive(Clone, Debug)]
pub struct ManagedDocumentMetadata {
    pub document: DocumentRecord,
    pub current: CurrentDocumentRecord,
    pub sections: BTreeMap<SectionId, SectionRecord>,
}

/// Validated current metadata for a workspace.
#[derive(Clone, Debug)]
pub struct MetadataCatalog {
    pub manifest: WorkspaceManifest,
    documents_by_id: BTreeMap<DocumentId, ManagedDocumentMetadata>,
    document_ids_by_path: BTreeMap<String, DocumentId>,
}

impl MetadataCatalog {
    pub fn documents(&self) -> impl Iterator<Item = &ManagedDocumentMetadata> {
        self.documents_by_id.values()
    }

    pub fn document_by_id(&self, document_id: &DocumentId) -> Option<&ManagedDocumentMetadata> {
        self.documents_by_id.get(document_id)
    }

    pub fn document_by_path(&self, relative_path: &str) -> Option<&ManagedDocumentMetadata> {
        let relative_path = normalize_separators(relative_path);
        self.document_ids_by_path
            .get(&relative_path)
            .and_then(|document_id| self.documents_by_id.get(document_id))
    }
}

/// Repository for tracked `.vds` metadata beneath one workspace.
#[derive(Clone, Debug)]
pub struct MetadataRepository {
    workspace_root: PathBuf,
    metadata_root: PathBuf,
}

/// Durable result of a completed managed-document relocation.
#[derive(Clone, Debug)]
pub struct RelocationResult {
    pub document_id: DocumentId,
    pub previous_relative_path: String,
    pub relative_path: String,
    pub content_hash: String,
}

/// Durable result of a committed section mutation.
#[derive(Clone, Debug)]
pub struct SectionMutationResult {
    pub document_id: DocumentId,
    pub section_id: SectionId,
    pub new_version_id: VersionId,
    pub new_content_hash: String,
}

/// Thin envelope for dispatching unknown intent.json files without fully deserializing them.
#[derive(Deserialize)]
struct TransactionEnvelope {
    #[serde(default)]
    transaction_kind: String,
}

fn default_content_mutation_kind() -> String {
    "content_mutation".to_owned()
}

fn default_relocation_kind() -> String {
    "relocation".to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ContentMutationIntent {
    format_version: u32,
    transaction_id: String,
    /// "content_mutation" or "structural_mutation"
    #[serde(default = "default_content_mutation_kind")]
    transaction_kind: String,
    mutation_kind: String,
    document_id: DocumentId,
    relative_path: String,
    section_id: SectionId,
    expected_content_hash: String,
    new_content_hash: String,
    created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PromotionIntent {
    format_version: u32,
    transaction_id: String,
    transaction_kind: String,
    document_id: DocumentId,
    created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DeletionTombstone {
    format_version: u32,
    document_id: DocumentId,
    previous_relative_path: String,
    content_hash: String,
    /// "remove" or "unmanage"
    operation: String,
    archived_history: bool,
    created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RelocationIntent {
    format_version: u32,
    transaction_id: String,
    #[serde(default = "default_relocation_kind")]
    transaction_kind: String,
    document_id: DocumentId,
    source_relative_path: String,
    destination_relative_path: String,
    expected_content_hash: String,
    created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SoftDeletionIntent {
    format_version: u32,
    transaction_id: String,
    transaction_kind: String,
    /// "remove" | "unmanage_archive" | "unmanage_drop"
    sub_kind: String,
    document_id: DocumentId,
    relative_path: String,
    content_hash: String,
    created_at: DateTime<Utc>,
}

impl MetadataRepository {
    /// Opens a repository path without creating or modifying it.
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self> {
        let workspace_root = canonical_workspace_root(workspace_root.as_ref())?;
        let metadata_root = workspace_root.join(".vds");
        Ok(Self {
            workspace_root,
            metadata_root,
        })
    }

    /// Initializes a new JSON metadata tree, or opens an existing VDS 2 tree.
    ///
    /// Initialization refuses a legacy `.vds/vds.db`; migration must move that
    /// database explicitly rather than quietly mixing both formats.
    pub fn initialize(workspace_root: impl AsRef<Path>) -> Result<Self> {
        let repository = Self::open(workspace_root)?;
        let legacy_database = repository.metadata_root.join("vds.db");
        if legacy_database.exists() {
            return Err(MetadataError::LegacyDatabasePresent(legacy_database));
        }

        fs::create_dir_all(repository.documents_root())
            .map_err(|source| MetadataError::io(repository.documents_root(), source))?;
        fs::create_dir_all(repository.metadata_root.join("tombstones/documents")).map_err(
            |source| {
                MetadataError::io(
                    repository.metadata_root.join("tombstones/documents"),
                    source,
                )
            },
        )?;
        fs::create_dir_all(repository.recovery_root())
            .map_err(|source| MetadataError::io(repository.recovery_root(), source))?;

        let manifest_path = repository.metadata_root.join("workspace.json");
        if manifest_path.exists() {
            repository.read_manifest()?;
        } else {
            let manifest = WorkspaceManifest {
                format_version: METADATA_FORMAT_VERSION,
                workspace_id: Uuid::now_v7().to_string(),
                created_at: Utc::now(),
            };
            write_json_create_new(&manifest_path, &manifest)?;
        }

        let ignore_path = repository.metadata_root.join(".gitignore");
        if !ignore_path.exists() {
            write_bytes_create_new(&ignore_path, b"/recovery/\n")?;
        }

        Ok(repository)
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn metadata_root(&self) -> &Path {
        &self.metadata_root
    }

    /// Loads and validates all current managed metadata, leaving immutable
    /// historical version bodies cold on disk.
    pub fn load_catalog(&self) -> Result<MetadataCatalog> {
        let manifest = self.read_manifest()?;
        let mut documents_by_id = BTreeMap::new();
        let mut document_ids_by_path = BTreeMap::new();
        let documents_root = self.documents_root();
        if !documents_root.exists() {
            return Ok(MetadataCatalog {
                manifest,
                documents_by_id,
                document_ids_by_path,
            });
        }

        for entry in fs::read_dir(&documents_root)
            .map_err(|source| MetadataError::io(&documents_root, source))?
        {
            let entry = entry.map_err(|source| MetadataError::io(&documents_root, source))?;
            let document_root = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|source| MetadataError::io(&document_root, source))?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }

            let document: DocumentRecord = read_json(&document_root.join("document.json"))?;
            validate_format(document.format_version, document_root.join("document.json"))?;
            validate_relative_path(&document.relative_path)?;
            let directory_id = entry.file_name().to_string_lossy().into_owned();
            if directory_id != document.document_id.as_str() {
                return Err(MetadataError::DocumentDirectoryMismatch {
                    path: document_root,
                    expected: document.document_id.as_str().to_owned(),
                    actual: directory_id,
                });
            }

            let current: CurrentDocumentRecord = read_json(&document_root.join("current.json"))?;
            validate_format(current.format_version, document_root.join("current.json"))?;
            let sections = load_sections(&document_root.join("sections"))?;
            if !sections.contains_key(&current.root_section_id) {
                return Err(MetadataError::MissingRootSection {
                    document_id: document.document_id.clone(),
                    section_id: current.root_section_id,
                });
            }

            let document_id = document.document_id.clone();
            let relative_path = document.relative_path.clone();
            if let Some(previous) =
                document_ids_by_path.insert(relative_path.clone(), document_id.clone())
            {
                return Err(MetadataError::DuplicateDocumentPath {
                    relative_path,
                    first: previous,
                    second: document_id,
                });
            }
            if documents_by_id
                .insert(
                    document_id.clone(),
                    ManagedDocumentMetadata {
                        document,
                        current,
                        sections,
                    },
                )
                .is_some()
            {
                return Err(MetadataError::DuplicateDocumentId(document_id));
            }
        }

        Ok(MetadataCatalog {
            manifest,
            documents_by_id,
            document_ids_by_path,
        })
    }

    /// Atomically promotes one materialized Markdown document into managed
    /// metadata. The document directory appears all at once after a successful
    /// same-filesystem rename.
    pub fn promote(&self, document: &MaterializedDocument) -> Result<ManagedDocumentMetadata> {
        self.read_manifest()?;
        validate_relative_path(&document.relative_path)?;
        let catalog = self.load_catalog()?;
        if catalog.document_by_path(&document.relative_path).is_some() {
            return Err(MetadataError::DocumentPathAlreadyManaged(
                document.relative_path.clone(),
            ));
        }
        if catalog.document_by_id(&document.document.id).is_some() {
            return Err(MetadataError::DocumentIdAlreadyManaged(
                document.document.id.clone(),
            ));
        }

        let source_path = self
            .workspace_root
            .join(path_from_vds(&document.relative_path));
        let source =
            fs::read(&source_path).map_err(|source| MetadataError::io(&source_path, source))?;
        let source_hash = sha256(&source);
        let document_record = DocumentRecord {
            format_version: METADATA_FORMAT_VERSION,
            document_id: document.document.id.clone(),
            relative_path: document.relative_path.clone(),
            name: None,
            title: document.document.metadata.title.clone(),
            description: document.document.metadata.description.clone(),
            tags: document.document.metadata.tags.clone(),
            created_at: document.document.created_at,
        };
        let current_record = CurrentDocumentRecord {
            format_version: METADATA_FORMAT_VERSION,
            content_hash: source_hash,
            current_document_version: document.document.current_version.clone(),
            root_section_id: document.document.root.clone(),
            updated_at: document.document.updated_at,
        };
        let sections = section_records(document);

        let transaction_id = Uuid::now_v7().to_string();
        let transaction_root = self.recovery_root().join(&transaction_id);
        let staged_document = transaction_root.join("staged/document");
        fs::create_dir_all(staged_document.join("sections"))
            .map_err(|source| MetadataError::io(staged_document.join("sections"), source))?;
        fs::create_dir_all(staged_document.join("versions"))
            .map_err(|source| MetadataError::io(staged_document.join("versions"), source))?;
        fs::create_dir_all(staged_document.join("snapshots"))
            .map_err(|source| MetadataError::io(staged_document.join("snapshots"), source))?;

        let promotion_intent = PromotionIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: transaction_id.clone(),
            transaction_kind: "promotion".to_owned(),
            document_id: document_record.document_id.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &promotion_intent)
            .inspect_err(|_e| {
                let _ = fs::remove_dir_all(&transaction_root);
            })?;

        let stage_result = (|| {
            write_json_create_new(&staged_document.join("document.json"), &document_record)?;
            write_json_create_new(&staged_document.join("current.json"), &current_record)?;
            for section in sections.values() {
                write_json_create_new(
                    &staged_document
                        .join("sections")
                        .join(format!("{}.json", section.section_id.as_str())),
                    section,
                )?;
            }
            for version in &document.versions {
                let version_root = staged_document
                    .join("versions")
                    .join(version.section_id.as_str());
                fs::create_dir_all(&version_root)
                    .map_err(|source| MetadataError::io(&version_root, source))?;
                write_json_create_new(
                    &version_root.join(format!("{}.json", version.version_id.as_str())),
                    version,
                )?;
            }
            Ok(())
        })();
        if let Err(error) = stage_result {
            let _ = fs::remove_dir_all(&transaction_root);
            return Err(error);
        }

        let final_document_root = self.documents_root().join(document.document.id.as_str());
        if final_document_root.exists() {
            let _ = fs::remove_dir_all(&transaction_root);
            return Err(MetadataError::DocumentIdAlreadyManaged(
                document.document.id.clone(),
            ));
        }
        fail_point()?;
        fs::rename(&staged_document, &final_document_root)
            .map_err(|source| MetadataError::io(&final_document_root, source))?;
        fail_point()?;
        let _ = fs::remove_dir_all(&transaction_root);

        Ok(ManagedDocumentMetadata {
            document: document_record,
            current: current_record,
            sections,
        })
    }

    /// Completes or rolls back any interrupted transactions found in the recovery directory.
    pub fn recover_transactions(&self) -> Result<()> {
        let recovery_root = self.recovery_root();
        if !recovery_root.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&recovery_root)
            .map_err(|source| MetadataError::io(&recovery_root, source))?
        {
            let entry = entry.map_err(|source| MetadataError::io(&recovery_root, source))?;
            let transaction_root = entry.path();
            if !entry
                .file_type()
                .map_err(|source| MetadataError::io(&transaction_root, source))?
                .is_dir()
            {
                continue;
            }
            let intent_path = transaction_root.join("intent.json");
            if !intent_path.exists() {
                // Orphaned staging dir with no intent — staging was not fully written; remove it.
                let _ = fs::remove_dir_all(&transaction_root);
                continue;
            }
            let envelope: TransactionEnvelope = read_json(&intent_path)?;
            match envelope.transaction_kind.as_str() {
                "relocation" | "" => self.recover_relocation(&transaction_root)?,
                "content_mutation" => {
                    self.recover_content_mutation_on_restart(&transaction_root)?
                }
                "structural_mutation" => {
                    self.recover_structural_mutation_on_restart(&transaction_root)?
                }
                "promotion" => self.recover_promotion(&transaction_root)?,
                "soft_deletion" => self.recover_soft_deletion(&transaction_root)?,
                kind => {
                    return Err(MetadataError::Other(format!(
                        "unknown transaction kind {kind:?} in {}",
                        transaction_root.display()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Moves authoritative Markdown and updates its managed path record using a
    /// recoverable same-workspace transaction.
    pub fn relocate_document(
        &self,
        document_id: &DocumentId,
        destination_relative_path: &str,
        expected_content_hash: &str,
        create_parent_directories: bool,
    ) -> Result<RelocationResult> {
        self.recover_transactions()?;
        validate_relative_path(destination_relative_path)?;
        let catalog = self.load_catalog()?;
        let managed = catalog
            .document_by_id(document_id)
            .ok_or_else(|| MetadataError::DocumentNotManaged(document_id.clone()))?;
        if catalog
            .document_by_path(destination_relative_path)
            .is_some_and(|other| other.document.document_id != *document_id)
        {
            return Err(MetadataError::DocumentPathAlreadyManaged(
                destination_relative_path.to_owned(),
            ));
        }
        let source_relative_path = managed.document.relative_path.clone();
        if source_relative_path == destination_relative_path {
            return Err(MetadataError::SameDocumentPath(source_relative_path));
        }
        let is_case_only_rename =
            cfg!(windows) && source_relative_path.eq_ignore_ascii_case(destination_relative_path);
        // Case-only renames are handled below via an intermediate temp path.

        let source = self
            .workspace_root
            .join(path_from_vds(&source_relative_path));
        let destination = self
            .workspace_root
            .join(path_from_vds(destination_relative_path));
        let contents =
            fs::read(&source).map_err(|source_error| MetadataError::io(&source, source_error))?;
        let actual_content_hash = sha256(&contents);
        if actual_content_hash != expected_content_hash {
            return Err(MetadataError::ContentHashConflict {
                path: source,
                expected: expected_content_hash.to_owned(),
                actual: actual_content_hash,
            });
        }
        if !is_case_only_rename && destination.exists() {
            return Err(MetadataError::DestinationExists(destination));
        }
        let destination_parent = destination.parent().ok_or_else(|| {
            MetadataError::InvalidRelativePath(destination_relative_path.to_owned())
        })?;
        if !destination_parent.exists() {
            if create_parent_directories {
                fs::create_dir_all(destination_parent)
                    .map_err(|source| MetadataError::io(destination_parent, source))?;
            } else {
                return Err(MetadataError::MissingDestinationParent(
                    destination_parent.to_path_buf(),
                ));
            }
        }

        let transaction_id = Uuid::now_v7().to_string();
        let transaction_root = self.recovery_root().join(&transaction_id);
        fs::create_dir_all(transaction_root.join("staged"))
            .map_err(|source| MetadataError::io(&transaction_root, source))?;
        let intent = RelocationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id,
            transaction_kind: "relocation".to_owned(),
            document_id: document_id.clone(),
            source_relative_path: source_relative_path.clone(),
            destination_relative_path: destination_relative_path.to_owned(),
            expected_content_hash: actual_content_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent)?;
        let mut updated_document = managed.document.clone();
        updated_document.relative_path = destination_relative_path.to_owned();
        write_json_create_new(
            &transaction_root.join("staged/document.json"),
            &updated_document,
        )?;

        if is_case_only_rename {
            // On Windows a case-only rename is a no-op with fs::rename because
            // source and destination resolve to the same inode. Go via a temp
            // name in the same directory to force the casing update.
            let temp_name = format!("~vds-rename-{}.md", &intent.transaction_id[..8]);
            let temp_path = destination_parent.join(&temp_name);
            fs::rename(&source, &temp_path).map_err(|e| MetadataError::io(&temp_path, e))?;
            fs::rename(&temp_path, &destination).map_err(|e| MetadataError::io(&destination, e))?;
        } else {
            fs::rename(&source, &destination)
                .map_err(|source| MetadataError::io(&destination, source))?;
        }
        if self
            .publish_relocation_metadata(&transaction_root, &intent)
            .is_err()
        {
            // Retry through the same recovery path used after process restart.
            self.recover_transactions()?;
        }

        // Remove the source parent directory if it is now empty and is not the
        // workspace root. Only one level is cleaned: VDS creates single-level
        // directories and we do not want to silently remove nested user directories.
        if let Some(source_parent) = source.parent()
            && source_parent != self.workspace_root
            && source_parent != destination_parent
            && source_parent.is_dir()
            && source_parent
                .read_dir()
                .is_ok_and(|mut d| d.next().is_none())
        {
            let _ = fs::remove_dir(source_parent);
        }

        Ok(RelocationResult {
            document_id: document_id.clone(),
            previous_relative_path: source_relative_path,
            relative_path: destination_relative_path.to_owned(),
            content_hash: actual_content_hash,
        })
    }

    /// Atomically commits a section content or heading mutation to the
    /// Markdown source and durable `.vds` metadata.
    ///
    /// The caller supplies the complete new Markdown for the document (computed
    /// via surgical span editing or full re-render) along with updated section
    /// and document records. Recovery through process restart is supported.
    pub fn commit_section_mutation(
        &self,
        document_id: &DocumentId,
        section_id: &SectionId,
        expected_content_hash: &str,
        new_markdown: &str,
        mutation_kind: &str,
        new_section_version: &SectionVersion,
        updated_section_record: &SectionRecord,
    ) -> Result<SectionMutationResult> {
        self.recover_transactions()?;
        let catalog = self.load_catalog()?;
        let managed = catalog
            .document_by_id(document_id)
            .ok_or_else(|| MetadataError::DocumentNotManaged(document_id.clone()))?;
        if !managed.sections.contains_key(section_id) {
            return Err(MetadataError::SectionNotManaged {
                document_id: document_id.clone(),
                section_id: section_id.clone(),
            });
        }

        let new_content_hash = sha256(new_markdown.as_bytes());
        let transaction_id = Uuid::now_v7().to_string();
        let transaction_root = self.recovery_root().join(&transaction_id);
        let staged = transaction_root.join("staged");
        fs::create_dir_all(&staged).map_err(|source| MetadataError::io(&staged, source))?;

        let intent = ContentMutationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: transaction_id.clone(),
            transaction_kind: "content_mutation".to_owned(),
            mutation_kind: mutation_kind.to_owned(),
            document_id: document_id.clone(),
            relative_path: managed.document.relative_path.clone(),
            section_id: section_id.clone(),
            expected_content_hash: expected_content_hash.to_owned(),
            new_content_hash: new_content_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent)?;
        fail_point()?;

        let version_path = staged.join(format!("{}.json", new_section_version.version_id.as_str()));
        write_json_create_new(&version_path, new_section_version)?;
        fail_point()?;

        let markdown_path = self
            .workspace_root
            .join(path_from_vds(&intent.relative_path));
        let original_markdown = fs::read_to_string(&markdown_path).unwrap_or_default();
        write_bytes_create_new(
            &staged.join("original_markdown"),
            original_markdown.as_bytes(),
        )?;
        write_bytes_create_new(&staged.join("markdown"), new_markdown.as_bytes())?;
        fail_point()?;

        write_json_create_new(
            &staged.join(format!(
                "section-{}.json",
                updated_section_record.section_id.as_str()
            )),
            updated_section_record,
        )?;
        fail_point()?;

        let result =
            self.apply_content_mutation(&transaction_root, &intent, updated_section_record);
        if result.is_err() {
            self.rollback_content_mutation(&transaction_root);
        }
        result.map(|()| SectionMutationResult {
            document_id: document_id.clone(),
            section_id: section_id.clone(),
            new_version_id: new_section_version.version_id.clone(),
            new_content_hash,
        })
    }

    /// Commits a structural mutation (create/remove/move/reorder sections) that
    /// requires a full document re-render.
    ///
    /// Identical durability semantics to `commit_section_mutation` except
    /// multiple section-version records may be staged.
    pub fn commit_structural_mutation(
        &self,
        document_id: &DocumentId,
        expected_content_hash: &str,
        new_markdown: &str,
        new_section_versions: &[SectionVersion],
        updated_section_records: &[SectionRecord],
        updated_current: &CurrentDocumentRecord,
    ) -> Result<String> {
        self.recover_transactions()?;
        let catalog = self.load_catalog()?;
        let managed = catalog
            .document_by_id(document_id)
            .ok_or_else(|| MetadataError::DocumentNotManaged(document_id.clone()))?;

        let new_content_hash = sha256(new_markdown.as_bytes());
        let transaction_id = Uuid::now_v7().to_string();
        let transaction_root = self.recovery_root().join(&transaction_id);
        let staged = transaction_root.join("staged");
        fs::create_dir_all(&staged).map_err(|source| MetadataError::io(&staged, source))?;

        let dummy_section_id = SectionId::new("structural");
        let intent = ContentMutationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: transaction_id.clone(),
            transaction_kind: "structural_mutation".to_owned(),
            mutation_kind: "structural".to_owned(),
            document_id: document_id.clone(),
            relative_path: managed.document.relative_path.clone(),
            section_id: dummy_section_id,
            expected_content_hash: expected_content_hash.to_owned(),
            new_content_hash: new_content_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent)?;
        fail_point()?;

        for version in new_section_versions {
            let version_path = staged.join(format!("{}.json", version.version_id.as_str()));
            write_json_create_new(&version_path, version)?;
        }
        fail_point()?;

        let markdown_path = self
            .workspace_root
            .join(path_from_vds(&intent.relative_path));
        let original_markdown = fs::read_to_string(&markdown_path).unwrap_or_default();
        write_bytes_create_new(
            &staged.join("original_markdown"),
            original_markdown.as_bytes(),
        )?;
        write_bytes_create_new(&staged.join("markdown"), new_markdown.as_bytes())?;
        fail_point()?;

        write_json_create_new(&staged.join("current.json"), updated_current)?;
        fail_point()?;

        let sections_staged = staged.join("sections");
        fs::create_dir_all(&sections_staged)
            .map_err(|source| MetadataError::io(&sections_staged, source))?;
        for record in updated_section_records {
            write_json_create_new(
                &sections_staged.join(format!("{}.json", record.section_id.as_str())),
                record,
            )?;
        }
        fail_point()?;

        self.apply_structural_mutation(
            &transaction_root,
            &intent,
            updated_current,
            updated_section_records,
        )
        .map(|()| new_content_hash)
    }

    /// Updates section metadata in `.vds` JSON without modifying the Markdown
    /// source. Use this for tag, summary, or lock changes.
    pub fn commit_metadata_only_mutation(
        &self,
        document_id: &DocumentId,
        updated_section_record: &SectionRecord,
        updated_current: &CurrentDocumentRecord,
    ) -> Result<()> {
        let document_root = self.documents_root().join(document_id.as_str());
        if !document_root.exists() {
            return Err(MetadataError::DocumentNotManaged(document_id.clone()));
        }
        let section_id = &updated_section_record.section_id;
        let section_path = document_root
            .join("sections")
            .join(format!("{}.json", section_id.as_str()));
        let current_path = document_root.join("current.json");

        overwrite_json(&section_path, updated_section_record)?;
        overwrite_json(&current_path, updated_current)?;
        Ok(())
    }

    /// Creates a new Markdown file at `relative_path` with `initial_markdown`
    /// content and promotes it to active VDS management.
    pub fn create_document_file(
        &self,
        relative_path: &str,
        initial_markdown: &str,
        title: Option<String>,
    ) -> Result<ManagedDocumentMetadata> {
        validate_relative_path(relative_path)?;
        let catalog = self.load_catalog()?;
        if catalog.document_by_path(relative_path).is_some() {
            return Err(MetadataError::DocumentPathAlreadyManaged(
                relative_path.to_owned(),
            ));
        }
        let dest_path = self.workspace_root.join(path_from_vds(relative_path));
        if dest_path.exists() {
            return Err(MetadataError::DocumentPathAlreadyManaged(
                relative_path.to_owned(),
            ));
        }
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|source| MetadataError::io(parent, source))?;
        }
        fs::write(&dest_path, initial_markdown.as_bytes())
            .map_err(|source| MetadataError::io(&dest_path, source))?;

        // Parse and promote as a managed document.
        let parsed = crate::markdown::parse_markdown_str(
            path_stem(relative_path),
            Some(relative_path.to_owned()),
            initial_markdown,
        );
        let mut doc = parsed.document.clone();
        if title.is_some() {
            doc.metadata.title = title;
        }
        let materialized = crate::workspace::MaterializedDocument {
            relative_path: relative_path.to_owned(),
            document: doc,
            sections: parsed.sections.clone(),
            versions: parsed.versions.clone(),
            managed: false,
            source_matches_metadata: false,
            source_spans: parsed.source_spans.clone(),
        };
        self.promote(&materialized)
    }

    /// Soft-deletes a managed Markdown file: moves active metadata to the
    /// inactive archive, writes a tombstone, and deletes the Markdown file.
    pub fn remove_document_file(
        &self,
        document_id: &DocumentId,
        expected_content_hash: &str,
    ) -> Result<String> {
        self.recover_transactions()?;
        let catalog = self.load_catalog()?;
        let managed = catalog
            .document_by_id(document_id)
            .ok_or_else(|| MetadataError::DocumentNotManaged(document_id.clone()))?;

        let current_hash = &managed.current.content_hash;
        if current_hash != expected_content_hash {
            return Err(MetadataError::ContentHashConflict {
                path: self
                    .workspace_root
                    .join(path_from_vds(&managed.document.relative_path)),
                expected: expected_content_hash.to_owned(),
                actual: current_hash.clone(),
            });
        }

        let previous_path = managed.document.relative_path.clone();
        let transaction_id = Uuid::now_v7().to_string();
        let transaction_root = self.recovery_root().join(&transaction_id);
        fs::create_dir_all(&transaction_root)
            .map_err(|source| MetadataError::io(&transaction_root, source))?;

        let intent = SoftDeletionIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: transaction_id.clone(),
            transaction_kind: "soft_deletion".to_owned(),
            sub_kind: "remove".to_owned(),
            document_id: document_id.clone(),
            relative_path: previous_path.clone(),
            content_hash: current_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).inspect_err(
            |_e| {
                let _ = fs::remove_dir_all(&transaction_root);
            },
        )?;

        let result = self.execute_remove_document_file(document_id, &previous_path, current_hash);
        if result.is_ok() {
            let _ = fs::remove_dir_all(&transaction_root);
        }
        result.map(|_| previous_path)
    }

    fn execute_remove_document_file(
        &self,
        document_id: &DocumentId,
        relative_path: &str,
        content_hash: &str,
    ) -> Result<()> {
        let markdown_path = self.workspace_root.join(path_from_vds(relative_path));
        let document_root = self.documents_root().join(document_id.as_str());
        let inactive_dir = self.inactive_root().join(document_id.as_str());
        let archive_dir = inactive_dir.join("archive");

        fs::create_dir_all(&archive_dir)
            .map_err(|source| MetadataError::io(&archive_dir, source))?;

        if !inactive_dir.join("tombstone.json").exists() {
            let tombstone = DeletionTombstone {
                format_version: METADATA_FORMAT_VERSION,
                document_id: document_id.clone(),
                previous_relative_path: relative_path.to_owned(),
                content_hash: content_hash.to_owned(),
                operation: "remove".to_owned(),
                archived_history: true,
                created_at: Utc::now(),
            };
            write_json_create_new(&inactive_dir.join("tombstone.json"), &tombstone)?;
        }
        fail_point()?;

        let archive_content = archive_dir.join("content.md");
        if markdown_path.exists() && !archive_content.exists() {
            fs::copy(&markdown_path, &archive_content)
                .map_err(|source| MetadataError::io(&archive_content, source))?;
        }
        fail_point()?;

        let archive_metadata = archive_dir.join("metadata");
        if document_root.exists() && !archive_metadata.exists() {
            fs::rename(&document_root, &archive_metadata)
                .map_err(|source| MetadataError::io(&archive_metadata, source))?;
        }
        fail_point()?;

        if markdown_path.exists() {
            fs::remove_file(&markdown_path)
                .map_err(|source| MetadataError::io(&markdown_path, source))?;
        }

        Ok(())
    }

    /// Stops managing a Markdown file, leaving it in place.  If
    /// `archive_history` is true, the metadata and history are preserved in the
    /// inactive archive; otherwise the active metadata directory is deleted.
    pub fn unmanage_document_file(
        &self,
        document_id: &DocumentId,
        expected_content_hash: &str,
        archive_history: bool,
    ) -> Result<String> {
        self.recover_transactions()?;
        let catalog = self.load_catalog()?;
        let managed = catalog
            .document_by_id(document_id)
            .ok_or_else(|| MetadataError::DocumentNotManaged(document_id.clone()))?;

        let current_hash = &managed.current.content_hash;
        if current_hash != expected_content_hash {
            return Err(MetadataError::ContentHashConflict {
                path: self
                    .workspace_root
                    .join(path_from_vds(&managed.document.relative_path)),
                expected: expected_content_hash.to_owned(),
                actual: current_hash.clone(),
            });
        }

        let previous_path = managed.document.relative_path.clone();
        let sub_kind = if archive_history {
            "unmanage_archive"
        } else {
            "unmanage_drop"
        };
        let transaction_id = Uuid::now_v7().to_string();
        let transaction_root = self.recovery_root().join(&transaction_id);
        fs::create_dir_all(&transaction_root)
            .map_err(|source| MetadataError::io(&transaction_root, source))?;

        let intent = SoftDeletionIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: transaction_id.clone(),
            transaction_kind: "soft_deletion".to_owned(),
            sub_kind: sub_kind.to_owned(),
            document_id: document_id.clone(),
            relative_path: previous_path.clone(),
            content_hash: current_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).inspect_err(
            |_e| {
                let _ = fs::remove_dir_all(&transaction_root);
            },
        )?;

        let result = self.execute_unmanage_document_file(
            document_id,
            &previous_path,
            current_hash,
            archive_history,
        );
        if result.is_ok() {
            let _ = fs::remove_dir_all(&transaction_root);
        }
        result.map(|_| previous_path)
    }

    fn execute_unmanage_document_file(
        &self,
        document_id: &DocumentId,
        relative_path: &str,
        content_hash: &str,
        archive_history: bool,
    ) -> Result<()> {
        let document_root = self.documents_root().join(document_id.as_str());
        let inactive_dir = self.inactive_root().join(document_id.as_str());

        if archive_history {
            let archive_dir = inactive_dir.join("archive");
            fs::create_dir_all(&archive_dir)
                .map_err(|source| MetadataError::io(&archive_dir, source))?;
            if !inactive_dir.join("tombstone.json").exists() {
                let tombstone = DeletionTombstone {
                    format_version: METADATA_FORMAT_VERSION,
                    document_id: document_id.clone(),
                    previous_relative_path: relative_path.to_owned(),
                    content_hash: content_hash.to_owned(),
                    operation: "unmanage".to_owned(),
                    archived_history: true,
                    created_at: Utc::now(),
                };
                write_json_create_new(&inactive_dir.join("tombstone.json"), &tombstone)?;
            }
            fail_point()?;
            let archive_metadata = archive_dir.join("metadata");
            if document_root.exists() && !archive_metadata.exists() {
                fs::rename(&document_root, &archive_metadata)
                    .map_err(|source| MetadataError::io(&archive_metadata, source))?;
            }
            fail_point()?;
        } else {
            fs::create_dir_all(&inactive_dir)
                .map_err(|source| MetadataError::io(&inactive_dir, source))?;
            if !inactive_dir.join("tombstone.json").exists() {
                let tombstone = DeletionTombstone {
                    format_version: METADATA_FORMAT_VERSION,
                    document_id: document_id.clone(),
                    previous_relative_path: relative_path.to_owned(),
                    content_hash: content_hash.to_owned(),
                    operation: "unmanage".to_owned(),
                    archived_history: false,
                    created_at: Utc::now(),
                };
                write_json_create_new(&inactive_dir.join("tombstone.json"), &tombstone)?;
            }
            fail_point()?;
            if document_root.exists() {
                fs::remove_dir_all(&document_root)
                    .map_err(|source| MetadataError::io(&document_root, source))?;
            }
            fail_point()?;
        }

        Ok(())
    }

    /// Updates the display name stored in `document.json`.  The file is not
    /// moved; only the `name` field in the active metadata record is changed.
    pub fn rename_document(&self, document_id: &DocumentId, new_name: &str) -> Result<()> {
        let document_root = self.documents_root().join(document_id.as_str());
        if !document_root.exists() {
            return Err(MetadataError::DocumentNotManaged(document_id.clone()));
        }
        let doc_json_path = document_root.join("document.json");
        let mut record: DocumentRecord = read_json(&doc_json_path)?;
        record.name = Some(new_name.to_owned());
        overwrite_json(&doc_json_path, &record)
    }

    /// Updates the path recorded in `document.json` to reflect an external
    /// file rename that happened outside VDS.  The Markdown file is not
    /// touched; only the managed path pointer is updated.
    pub fn record_external_rename(
        &self,
        document_id: &DocumentId,
        new_relative_path: &str,
    ) -> Result<()> {
        let document_root = self.documents_root().join(document_id.as_str());
        if !document_root.exists() {
            return Err(MetadataError::DocumentNotManaged(document_id.clone()));
        }
        let doc_json_path = document_root.join("document.json");
        let mut record: DocumentRecord = read_json(&doc_json_path)?;
        record.relative_path = new_relative_path.to_owned();
        overwrite_json(&doc_json_path, &record)
    }

    /// Lists all version IDs stored for a section, sorted oldest-first by
    /// UUIDv7 lexicographic order (which equals chronological order).
    pub fn list_section_versions(
        &self,
        document_id: &DocumentId,
        section_id: &SectionId,
    ) -> Result<Vec<VersionId>> {
        let version_dir = self
            .documents_root()
            .join(document_id.as_str())
            .join("versions")
            .join(section_id.as_str());
        if !version_dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in fs::read_dir(&version_dir).map_err(|e| MetadataError::io(&version_dir, e))? {
            let entry = entry.map_err(|e| MetadataError::io(&version_dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".json") {
                ids.push(VersionId::new(stem));
            }
        }
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(ids)
    }

    /// Reads one immutable section version from disk.
    pub fn read_section_version(
        &self,
        document_id: &DocumentId,
        section_id: &SectionId,
        version_id: &VersionId,
    ) -> Result<SectionVersion> {
        let path = self
            .documents_root()
            .join(document_id.as_str())
            .join("versions")
            .join(section_id.as_str())
            .join(format!("{}.json", version_id.as_str()));
        if !path.exists() {
            return Err(MetadataError::Other(format!(
                "version {} not found for section {}",
                version_id.as_str(),
                section_id.as_str()
            )));
        }
        read_json(&path)
    }

    /// Creates a new document-level snapshot capturing the current section tree.
    ///
    /// The snapshot is written as an immutable JSON file under
    /// `.vds/documents/{doc_id}/snapshots/{snapshot_id}.json`.
    pub fn create_snapshot(
        &self,
        document_id: &DocumentId,
        sections: Vec<Section>,
        root_version: VersionId,
        label: Option<String>,
        change_summary: Option<String>,
    ) -> Result<DocumentSnapshot> {
        let document_root = self.documents_root().join(document_id.as_str());
        if !document_root.exists() {
            return Err(MetadataError::DocumentNotManaged(document_id.clone()));
        }
        let snapshots_dir = document_root.join("snapshots");
        fs::create_dir_all(&snapshots_dir).map_err(|e| MetadataError::io(&snapshots_dir, e))?;
        let snapshot_id = SnapshotId::new_v7();
        let snapshot = DocumentSnapshot {
            snapshot_id: snapshot_id.clone(),
            document_id: document_id.clone(),
            root_version,
            sections,
            label,
            created_at: Utc::now(),
            author: None,
            change_summary,
        };
        let path = snapshots_dir.join(format!("{}.json", snapshot_id.as_str()));
        write_json_create_new(&path, &snapshot)?;
        Ok(snapshot)
    }

    /// Lists all snapshots for a document, ordered oldest-first.
    pub fn list_snapshots(&self, document_id: &DocumentId) -> Result<Vec<DocumentSnapshot>> {
        let snapshots_dir = self
            .documents_root()
            .join(document_id.as_str())
            .join("snapshots");
        if !snapshots_dir.exists() {
            return Ok(Vec::new());
        }
        let mut snapshots = Vec::new();
        for entry in
            fs::read_dir(&snapshots_dir).map_err(|e| MetadataError::io(&snapshots_dir, e))?
        {
            let entry = entry.map_err(|e| MetadataError::io(&snapshots_dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") {
                let snapshot: DocumentSnapshot = read_json(&entry.path())?;
                snapshots.push(snapshot);
            }
        }
        snapshots.sort_by(|a, b| a.snapshot_id.as_str().cmp(b.snapshot_id.as_str()));
        Ok(snapshots)
    }

    /// Reads one snapshot from disk.
    pub fn read_snapshot(
        &self,
        document_id: &DocumentId,
        snapshot_id: &SnapshotId,
    ) -> Result<DocumentSnapshot> {
        let path = self
            .documents_root()
            .join(document_id.as_str())
            .join("snapshots")
            .join(format!("{}.json", snapshot_id.as_str()));
        if !path.exists() {
            return Err(MetadataError::Other(format!(
                "snapshot {} not found for document {}",
                snapshot_id.as_str(),
                document_id.as_str()
            )));
        }
        read_json(&path)
    }

    /// Restores a soft-deleted or unmanaged document from the inactive archive.
    ///
    /// For `remove` tombstones, rewrites the archived `content.md` to the
    /// restore path and moves metadata back to the active documents directory.
    /// For `unmanage` tombstones with archived history, the Markdown file must
    /// already exist (or a `relative_path` override must be provided); metadata
    /// is moved back to active. Tombstones without archived history cannot be
    /// restored and return an error.
    pub fn restore_document_file(
        &self,
        document_id: &DocumentId,
        relative_path: Option<&str>,
    ) -> Result<String> {
        let inactive_dir = self.inactive_root().join(document_id.as_str());
        let tombstone_path = inactive_dir.join("tombstone.json");

        if !tombstone_path.exists() {
            return Err(MetadataError::DocumentNotManaged(document_id.clone()));
        }

        let tombstone: DeletionTombstone = read_json(&tombstone_path)?;
        if !tombstone.archived_history {
            return Err(MetadataError::Other(format!(
                "document {} was unmanaged without archiving history and cannot be restored",
                document_id.as_str()
            )));
        }

        let restore_path = relative_path
            .map(|s| s.to_owned())
            .unwrap_or_else(|| tombstone.previous_relative_path.clone());

        validate_relative_path(&restore_path)?;

        let restore_abs = self.workspace_root.join(path_from_vds(&restore_path));
        if restore_abs.exists() {
            return Err(MetadataError::DestinationExists(restore_abs));
        }

        let archive_dir = inactive_dir.join("archive");
        let archived_metadata = archive_dir.join("metadata");
        let document_root = self.documents_root().join(document_id.as_str());

        if tombstone.operation == "remove" {
            // Recreate the Markdown file from the archived copy.
            let archived_content = archive_dir.join("content.md");
            if !archived_content.exists() {
                return Err(MetadataError::Other(format!(
                    "archived content for document {} not found at {}",
                    document_id.as_str(),
                    archived_content.display()
                )));
            }
            if let Some(parent) = restore_abs.parent() {
                fs::create_dir_all(parent).map_err(|source| MetadataError::io(parent, source))?;
            }
            fs::copy(&archived_content, &restore_abs)
                .map_err(|source| MetadataError::io(&restore_abs, source))?;
        }

        // Move archived metadata back to the active documents directory.
        fs::rename(&archived_metadata, &document_root)
            .map_err(|source| MetadataError::io(&archived_metadata, source))?;

        // Update relative_path in document.json if we're restoring to a different path.
        let doc_json_path = document_root.join("document.json");
        let mut doc_record: DocumentRecord = read_json(&doc_json_path)?;
        if doc_record.relative_path != restore_path {
            doc_record.relative_path = restore_path.clone();
            overwrite_json(&doc_json_path, &doc_record)?;
        }

        // Clean up the inactive archive directory.
        fs::remove_dir_all(&inactive_dir)
            .map_err(|source| MetadataError::io(&inactive_dir, source))?;

        Ok(restore_path)
    }

    fn apply_content_mutation(
        &self,
        transaction_root: &Path,
        intent: &ContentMutationIntent,
        updated_section_record: &SectionRecord,
    ) -> Result<()> {
        let document_root = self.documents_root().join(intent.document_id.as_str());
        let staged = transaction_root.join("staged");
        let markdown_path = self
            .workspace_root
            .join(path_from_vds(&intent.relative_path));
        let tmp_path = markdown_path.with_extension("md.tmp");

        let current_disk_hash = sha256(&fs::read(&markdown_path).unwrap_or_default());
        let written_hash;
        if current_disk_hash != intent.new_content_hash {
            if current_disk_hash != intent.expected_content_hash {
                // External edit detected. Attempt a three-way merge using the
                // original content that was staged alongside our new content.
                let original_path = staged.join("original_markdown");
                if original_path.exists() {
                    let original = fs::read_to_string(&original_path).unwrap_or_default();
                    let ours = fs::read_to_string(staged.join("markdown")).unwrap_or_default();
                    let theirs = fs::read_to_string(&markdown_path).unwrap_or_default();
                    match diffy::merge(&original, &ours, &theirs) {
                        Ok(merged) => {
                            let merged_bytes = merged.into_bytes();
                            written_hash = sha256(&merged_bytes);
                            fs::write(&tmp_path, &merged_bytes)
                                .map_err(|source| MetadataError::io(&tmp_path, source))?;
                            fs::rename(&tmp_path, &markdown_path)
                                .map_err(|source| MetadataError::io(&markdown_path, source))?;
                        }
                        Err(_) => {
                            return Err(MetadataError::ExternalContentConflict {
                                document_id: intent.document_id.clone(),
                                path: markdown_path.clone(),
                                expected_hash: intent.expected_content_hash.clone(),
                                actual_hash: current_disk_hash,
                            });
                        }
                    }
                } else {
                    return Err(MetadataError::ExternalContentConflict {
                        document_id: intent.document_id.clone(),
                        path: markdown_path.clone(),
                        expected_hash: intent.expected_content_hash.clone(),
                        actual_hash: current_disk_hash,
                    });
                }
            } else {
                let staged_markdown = staged.join("markdown");
                if staged_markdown.exists() {
                    fs::copy(&staged_markdown, &tmp_path)
                        .map_err(|source| MetadataError::io(&tmp_path, source))?;
                    fs::rename(&tmp_path, &markdown_path)
                        .map_err(|source| MetadataError::io(&markdown_path, source))?;
                }
                written_hash = intent.new_content_hash.clone();
            }
        } else {
            written_hash = intent.new_content_hash.clone();
        }

        let version_dir = document_root
            .join("versions")
            .join(intent.section_id.as_str());
        fs::create_dir_all(&version_dir)
            .map_err(|source| MetadataError::io(&version_dir, source))?;
        for entry in fs::read_dir(&staged).map_err(|source| MetadataError::io(&staged, source))? {
            let entry = entry.map_err(|source| MetadataError::io(&staged, source))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") && name != "current.json" && !name.starts_with("section-") {
                let dest = version_dir.join(&name);
                if !dest.exists() {
                    fs::rename(entry.path(), &dest)
                        .map_err(|source| MetadataError::io(&dest, source))?;
                }
            }
        }

        let section_path = document_root
            .join("sections")
            .join(format!("{}.json", intent.section_id.as_str()));
        overwrite_json(&section_path, updated_section_record)?;

        let current_path = document_root.join("current.json");
        let mut current: CurrentDocumentRecord = read_json(&current_path)?;
        current.content_hash = written_hash;
        current.updated_at = Utc::now();
        overwrite_json(&current_path, &current)?;

        fs::remove_dir_all(transaction_root)
            .map_err(|source| MetadataError::io(transaction_root, source))?;
        Ok(())
    }

    /// Called inline when `apply_content_mutation` fails mid-flight. Cleans up the
    /// transaction directory; the mutation is considered rolled back.
    fn rollback_content_mutation(&self, transaction_root: &Path) {
        let _ = fs::remove_dir_all(transaction_root);
    }

    /// Called on process restart when a `content_mutation` transaction is found in
    /// the recovery directory. Completes the commit if staged files are present,
    /// rolls back if the disk is still at the original hash, or leaves the directory
    /// in place so the operator can inspect it.
    fn recover_content_mutation_on_restart(&self, transaction_root: &Path) -> Result<()> {
        let intent: ContentMutationIntent = read_json(&transaction_root.join("intent.json"))?;
        validate_format(intent.format_version, transaction_root.join("intent.json"))?;
        let markdown_path = self
            .workspace_root
            .join(path_from_vds(&intent.relative_path));
        let disk_hash = sha256(&fs::read(&markdown_path).unwrap_or_default());

        let staged = transaction_root.join("staged");

        if disk_hash == intent.new_content_hash {
            // Markdown is already at the new content. The transaction may have been
            // interrupted after the rename but before metadata was updated (C-F).
            // If the staged section record still exists, complete those steps now.
            let section_record_path =
                staged.join(format!("section-{}.json", intent.section_id.as_str()));
            if section_record_path.exists() {
                let section_record: SectionRecord = read_json(&section_record_path)?;
                return self.apply_content_mutation(transaction_root, &intent, &section_record);
            }
            let _ = fs::remove_dir_all(transaction_root);
            return Ok(());
        }

        let staged_markdown = staged.join("markdown");
        if staged_markdown.exists() {
            let staged_hash = sha256(&fs::read(&staged_markdown).unwrap_or_default());
            if staged_hash == intent.new_content_hash {
                let section_record_path =
                    staged.join(format!("section-{}.json", intent.section_id.as_str()));
                if section_record_path.exists() {
                    let section_record: SectionRecord = read_json(&section_record_path)?;
                    return self.apply_content_mutation(transaction_root, &intent, &section_record);
                }
            }
        }

        // If disk is still at the expected (pre-mutation) hash the mutation was never applied.
        if disk_hash == intent.expected_content_hash {
            let _ = fs::remove_dir_all(transaction_root);
        }
        // Otherwise we cannot determine safe state; leave the directory for diagnostics.
        Ok(())
    }

    /// Called on process restart when a `structural_mutation` transaction is found.
    /// Reads all staged files from disk and completes the commit.
    fn recover_structural_mutation_on_restart(&self, transaction_root: &Path) -> Result<()> {
        let intent: ContentMutationIntent = read_json(&transaction_root.join("intent.json"))?;
        validate_format(intent.format_version, transaction_root.join("intent.json"))?;
        let markdown_path = self
            .workspace_root
            .join(path_from_vds(&intent.relative_path));
        let disk_hash = sha256(&fs::read(&markdown_path).unwrap_or_default());

        let staged = transaction_root.join("staged");
        let staged_current_path = staged.join("current.json");
        let staged_markdown = staged.join("markdown");

        if disk_hash == intent.new_content_hash {
            // Markdown already at new content. If staged current.json remains,
            // the metadata updates (section records, current.json) haven't been
            // applied yet — complete them now.
            if staged_current_path.exists() {
                let updated_current: CurrentDocumentRecord = read_json(&staged_current_path)?;
                let staged_sections_dir = staged.join("sections");
                let mut section_records: Vec<SectionRecord> = Vec::new();
                if staged_sections_dir.exists() {
                    for entry in fs::read_dir(&staged_sections_dir)
                        .map_err(|source| MetadataError::io(&staged_sections_dir, source))?
                    {
                        let entry = entry
                            .map_err(|source| MetadataError::io(&staged_sections_dir, source))?;
                        if entry.file_name().to_string_lossy().ends_with(".json") {
                            section_records.push(read_json(&entry.path())?);
                        }
                    }
                }
                return self.apply_structural_mutation(
                    transaction_root,
                    &intent,
                    &updated_current,
                    &section_records,
                );
            }
            let _ = fs::remove_dir_all(transaction_root);
            return Ok(());
        }

        if staged_markdown.exists() && staged_current_path.exists() {
            let staged_hash = sha256(&fs::read(&staged_markdown).unwrap_or_default());
            if staged_hash == intent.new_content_hash {
                let updated_current: CurrentDocumentRecord = read_json(&staged_current_path)?;
                let staged_sections_dir = staged.join("sections");
                let mut section_records: Vec<SectionRecord> = Vec::new();
                if staged_sections_dir.exists() {
                    for entry in fs::read_dir(&staged_sections_dir)
                        .map_err(|source| MetadataError::io(&staged_sections_dir, source))?
                    {
                        let entry = entry
                            .map_err(|source| MetadataError::io(&staged_sections_dir, source))?;
                        if entry.file_name().to_string_lossy().ends_with(".json") {
                            section_records.push(read_json(&entry.path())?);
                        }
                    }
                }
                return self.apply_structural_mutation(
                    transaction_root,
                    &intent,
                    &updated_current,
                    &section_records,
                );
            }
        }

        if disk_hash == intent.expected_content_hash {
            let _ = fs::remove_dir_all(transaction_root);
        }
        Ok(())
    }

    /// Called on process restart when a `promotion` transaction is found.
    fn recover_promotion(&self, transaction_root: &Path) -> Result<()> {
        let intent: PromotionIntent = read_json(&transaction_root.join("intent.json"))?;
        validate_format(intent.format_version, transaction_root.join("intent.json"))?;
        let final_document_root = self.documents_root().join(intent.document_id.as_str());

        if final_document_root.exists() {
            // Promotion completed; transaction dir is orphaned.
            let _ = fs::remove_dir_all(transaction_root);
            return Ok(());
        }

        let staged_document = transaction_root.join("staged/document");
        if staged_document.exists() {
            fs::rename(&staged_document, &final_document_root)
                .map_err(|source| MetadataError::io(&final_document_root, source))?;
            let _ = fs::remove_dir_all(transaction_root);
            return Ok(());
        }

        // Staging was interrupted before any files were written; nothing to recover.
        let _ = fs::remove_dir_all(transaction_root);
        Ok(())
    }

    fn apply_structural_mutation(
        &self,
        transaction_root: &Path,
        intent: &ContentMutationIntent,
        updated_current: &CurrentDocumentRecord,
        updated_section_records: &[SectionRecord],
    ) -> Result<()> {
        let document_root = self.documents_root().join(intent.document_id.as_str());
        let staged = transaction_root.join("staged");
        let markdown_path = self
            .workspace_root
            .join(path_from_vds(&intent.relative_path));
        let tmp_path = markdown_path.with_extension("md.tmp");

        let current_disk_hash = sha256(&fs::read(&markdown_path).unwrap_or_default());
        if current_disk_hash != intent.new_content_hash {
            if current_disk_hash != intent.expected_content_hash {
                return Err(MetadataError::ExternalContentConflict {
                    document_id: intent.document_id.clone(),
                    path: markdown_path.clone(),
                    expected_hash: intent.expected_content_hash.clone(),
                    actual_hash: current_disk_hash,
                });
            }
            let staged_markdown = staged.join("markdown");
            if staged_markdown.exists() {
                fs::copy(&staged_markdown, &tmp_path)
                    .map_err(|source| MetadataError::io(&tmp_path, source))?;
                fs::rename(&tmp_path, &markdown_path)
                    .map_err(|source| MetadataError::io(&markdown_path, source))?;
            }
        }

        for record in updated_section_records {
            let section_path = document_root
                .join("sections")
                .join(format!("{}.json", record.section_id.as_str()));
            overwrite_json(&section_path, record)?;
        }

        let sections_dir = document_root.join("sections");
        let staged_sections = staged.join("sections");
        if staged_sections.exists() {
            for entry in fs::read_dir(&staged_sections)
                .map_err(|source| MetadataError::io(&staged_sections, source))?
            {
                let entry = entry.map_err(|source| MetadataError::io(&staged_sections, source))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".json") {
                    let dest = sections_dir.join(&name);
                    if !dest.exists() {
                        fs::rename(entry.path(), &dest)
                            .map_err(|source| MetadataError::io(&dest, source))?;
                    }
                }
            }
        }

        for entry in fs::read_dir(&staged).map_err(|source| MetadataError::io(&staged, source))? {
            let entry = entry.map_err(|source| MetadataError::io(&staged, source))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") && name != "current.json" && !name.starts_with("section") {
                let section_id_str = name.trim_end_matches(".json");
                let version_dir = document_root.join("versions").join(section_id_str);
                fs::create_dir_all(&version_dir)
                    .map_err(|source| MetadataError::io(&version_dir, source))?;
            }
        }

        let mut persisted_current = updated_current.clone();
        persisted_current.content_hash = intent.new_content_hash.clone();
        overwrite_json(&document_root.join("current.json"), &persisted_current)?;

        fs::remove_dir_all(transaction_root)
            .map_err(|source| MetadataError::io(transaction_root, source))?;
        Ok(())
    }

    fn read_manifest(&self) -> Result<WorkspaceManifest> {
        let path = self.metadata_root.join("workspace.json");
        if !path.exists() {
            return Err(MetadataError::MissingManifest(path));
        }
        let manifest: WorkspaceManifest = read_json(&path)?;
        validate_format(manifest.format_version, path)?;
        Ok(manifest)
    }

    fn documents_root(&self) -> PathBuf {
        self.metadata_root.join("documents")
    }

    fn recovery_root(&self) -> PathBuf {
        self.metadata_root.join("recovery")
    }

    fn inactive_root(&self) -> PathBuf {
        self.metadata_root.join("inactive")
    }

    fn recover_soft_deletion(&self, transaction_root: &Path) -> Result<()> {
        let intent: SoftDeletionIntent = read_json(&transaction_root.join("intent.json"))?;
        validate_format(intent.format_version, transaction_root.join("intent.json"))?;

        let inactive_dir = self.inactive_root().join(intent.document_id.as_str());
        let tombstone_path = inactive_dir.join("tombstone.json");

        if !tombstone_path.exists() {
            // Nothing was durably committed; clean up the transaction dir.
            let _ = fs::remove_dir_all(transaction_root);
            return Ok(());
        }

        match intent.sub_kind.as_str() {
            "remove" => {
                let result = self.execute_remove_document_file(
                    &intent.document_id,
                    &intent.relative_path,
                    &intent.content_hash,
                );
                if result.is_ok() {
                    let _ = fs::remove_dir_all(transaction_root);
                }
                result
            }
            "unmanage_archive" => {
                let result = self.execute_unmanage_document_file(
                    &intent.document_id,
                    &intent.relative_path,
                    &intent.content_hash,
                    true,
                );
                if result.is_ok() {
                    let _ = fs::remove_dir_all(transaction_root);
                }
                result
            }
            "unmanage_drop" => {
                let result = self.execute_unmanage_document_file(
                    &intent.document_id,
                    &intent.relative_path,
                    &intent.content_hash,
                    false,
                );
                if result.is_ok() {
                    let _ = fs::remove_dir_all(transaction_root);
                }
                result
            }
            sub_kind => Err(MetadataError::Other(format!(
                "unknown soft_deletion sub_kind {sub_kind:?} in {}",
                transaction_root.display()
            ))),
        }
    }

    fn recover_relocation(&self, transaction_root: &Path) -> Result<()> {
        let intent: RelocationIntent = read_json(&transaction_root.join("intent.json"))?;
        validate_format(intent.format_version, transaction_root.join("intent.json"))?;
        let source = self
            .workspace_root
            .join(path_from_vds(&intent.source_relative_path));
        let destination = self
            .workspace_root
            .join(path_from_vds(&intent.destination_relative_path));
        match (source.exists(), destination.exists()) {
            (true, false) => {
                self.restore_original_metadata(transaction_root, &intent)?;
                fs::remove_dir_all(transaction_root)
                    .map_err(|source| MetadataError::io(transaction_root, source))?;
                Ok(())
            }
            (false, true) => self.publish_relocation_metadata(transaction_root, &intent),
            (source_exists, destination_exists) => Err(MetadataError::RecoveryConflict {
                transaction: transaction_root.to_path_buf(),
                source_exists,
                destination_exists,
            }),
        }
    }

    fn publish_relocation_metadata(
        &self,
        transaction_root: &Path,
        intent: &RelocationIntent,
    ) -> Result<()> {
        let document_json = self
            .documents_root()
            .join(intent.document_id.as_str())
            .join("document.json");
        let staged = transaction_root.join("staged/document.json");
        let backup = transaction_root.join("original-document.json");

        if document_json.exists() {
            let current: DocumentRecord = read_json(&document_json)?;
            if current.relative_path == intent.destination_relative_path {
                fs::remove_dir_all(transaction_root)
                    .map_err(|source| MetadataError::io(transaction_root, source))?;
                return Ok(());
            }
            if !backup.exists() {
                fs::rename(&document_json, &backup)
                    .map_err(|source| MetadataError::io(&backup, source))?;
            }
        }
        if staged.exists() {
            fs::rename(&staged, &document_json)
                .map_err(|source| MetadataError::io(&document_json, source))?;
        } else if !document_json.exists() {
            return Err(MetadataError::RecoveryConflict {
                transaction: transaction_root.to_path_buf(),
                source_exists: false,
                destination_exists: true,
            });
        }
        fs::remove_dir_all(transaction_root)
            .map_err(|source| MetadataError::io(transaction_root, source))?;
        Ok(())
    }

    fn restore_original_metadata(
        &self,
        transaction_root: &Path,
        intent: &RelocationIntent,
    ) -> Result<()> {
        let document_json = self
            .documents_root()
            .join(intent.document_id.as_str())
            .join("document.json");
        let backup = transaction_root.join("original-document.json");
        if backup.exists() {
            if document_json.exists() {
                fs::remove_file(&document_json)
                    .map_err(|source| MetadataError::io(&document_json, source))?;
            }
            fs::rename(&backup, &document_json)
                .map_err(|source| MetadataError::io(&document_json, source))?;
        }
        Ok(())
    }
}

fn load_sections(root: &Path) -> Result<BTreeMap<SectionId, SectionRecord>> {
    let mut sections = BTreeMap::new();
    if !root.exists() {
        return Ok(sections);
    }
    for entry in fs::read_dir(root).map_err(|source| MetadataError::io(root, source))? {
        let entry = entry.map_err(|source| MetadataError::io(root, source))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|source| MetadataError::io(&path, source))?
            .is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let section: SectionRecord = read_json(&path)?;
        validate_format(section.format_version, &path)?;
        let expected_name = format!("{}.json", section.section_id.as_str());
        let actual_name = entry.file_name().to_string_lossy().into_owned();
        if actual_name != expected_name {
            return Err(MetadataError::SectionFileMismatch {
                path,
                expected: expected_name,
                actual: actual_name,
            });
        }
        if sections
            .insert(section.section_id.clone(), section)
            .is_some()
        {
            return Err(MetadataError::DuplicateSectionId(actual_name));
        }
    }
    Ok(sections)
}

fn section_records(document: &MaterializedDocument) -> BTreeMap<SectionId, SectionRecord> {
    let sections_by_id = document
        .sections
        .iter()
        .map(|section| (section.section_id.clone(), section))
        .collect::<BTreeMap<_, _>>();
    document
        .sections
        .iter()
        .map(|section| {
            let ancestry = section_ancestry(section, &sections_by_id);
            let record = SectionRecord {
                format_version: METADATA_FORMAT_VERSION,
                section_id: section.section_id.clone(),
                current_version: section.current_version.clone(),
                metadata: section.metadata.clone(),
                matching: SectionMatchingRecord {
                    embedded_marker: None,
                    last_known_title: section.title.clone(),
                    last_known_ancestry: ancestry,
                    last_known_ordinal: section.ordinal,
                    content_fingerprint: sha256(section.content.as_bytes()),
                },
                updated_at: section.updated_at,
            };
            (section.section_id.clone(), record)
        })
        .collect()
}

fn section_ancestry(
    section: &crate::document::Section,
    sections_by_id: &BTreeMap<SectionId, &crate::document::Section>,
) -> Vec<String> {
    let mut titles = Vec::new();
    let mut parent_id = section.parent_id.as_ref();
    while let Some(id) = parent_id {
        let Some(parent) = sections_by_id.get(id) else {
            break;
        };
        if parent.parent_id.is_some() {
            titles.push(parent.title.clone());
        }
        parent_id = parent.parent_id.as_ref();
    }
    titles.reverse();
    titles
}

fn canonical_workspace_root(root: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(root).map_err(|source| MetadataError::io(root, source))?;
    if !canonical.is_dir() {
        return Err(MetadataError::InvalidWorkspaceRoot(canonical));
    }
    Ok(canonical)
}

fn validate_relative_path(relative_path: &str) -> Result<()> {
    if relative_path.is_empty() || relative_path.contains('\\') {
        return Err(MetadataError::InvalidRelativePath(relative_path.to_owned()));
    }
    let path = Path::new(relative_path);
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(MetadataError::InvalidRelativePath(relative_path.to_owned()));
        }
    }
    Ok(())
}

fn path_from_vds(relative_path: &str) -> PathBuf {
    relative_path.split('/').collect()
}

fn path_stem(relative_path: &str) -> String {
    let last = relative_path.rsplit('/').next().unwrap_or(relative_path);
    match last.rfind('.') {
        Some(dot) => last[..dot].to_owned(),
        None => last.to_owned(),
    }
}

fn normalize_separators(path: &str) -> String {
    path.replace('\\', "/")
}

fn validate_format(format_version: u32, path: impl Into<PathBuf>) -> Result<()> {
    if format_version != METADATA_FORMAT_VERSION {
        return Err(MetadataError::UnsupportedFormat {
            path: path.into(),
            found: format_version,
            supported: METADATA_FORMAT_VERSION,
        });
    }
    Ok(())
}

fn sha256(contents: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(contents)))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path).map_err(|source| MetadataError::io(path, source))?;
    serde_json::from_reader(file).map_err(|source| MetadataError::json(path, source))
}

fn overwrite_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|source| MetadataError::json(path, source))?;
    bytes.push(b'\n');
    let mut file = File::create(path).map_err(|source| MetadataError::io(path, source))?;
    file.write_all(&bytes)
        .map_err(|source| MetadataError::io(path, source))?;
    file.sync_all()
        .map_err(|source| MetadataError::io(path, source))?;
    Ok(())
}

fn write_json_create_new<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|source| MetadataError::json(path, source))?;
    bytes.push(b'\n');
    write_bytes_create_new(path, &bytes)
}

fn write_bytes_create_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| MetadataError::io(path, source))?;
    file.write_all(bytes)
        .map_err(|source| MetadataError::io(path, source))?;
    file.sync_all()
        .map_err(|source| MetadataError::io(path, source))?;
    Ok(())
}

/// Errors produced by the durable metadata repository.
#[derive(Debug)]
pub enum MetadataError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidWorkspaceRoot(PathBuf),
    InvalidRelativePath(String),
    MissingManifest(PathBuf),
    LegacyDatabasePresent(PathBuf),
    UnsupportedFormat {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    DocumentDirectoryMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    SectionFileMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    MissingRootSection {
        document_id: DocumentId,
        section_id: SectionId,
    },
    DuplicateDocumentPath {
        relative_path: String,
        first: DocumentId,
        second: DocumentId,
    },
    DuplicateDocumentId(DocumentId),
    DuplicateSectionId(String),
    DocumentPathAlreadyManaged(String),
    DocumentIdAlreadyManaged(DocumentId),
    DocumentNotManaged(DocumentId),
    SameDocumentPath(String),
    CaseOnlyRenameUnsupported {
        source: String,
        destination: String,
    },
    ContentHashConflict {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    DestinationExists(PathBuf),
    MissingDestinationParent(PathBuf),
    RecoveryConflict {
        transaction: PathBuf,
        source_exists: bool,
        destination_exists: bool,
    },
    SectionNotManaged {
        document_id: DocumentId,
        section_id: SectionId,
    },
    ExternalContentConflict {
        document_id: DocumentId,
        path: PathBuf,
        expected_hash: String,
        actual_hash: String,
    },
    Other(String),
}

impl MetadataError {
    fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Json { path, source } => {
                write!(formatter, "invalid JSON at {}: {source}", path.display())
            }
            Self::InvalidWorkspaceRoot(path) => {
                write!(
                    formatter,
                    "workspace root is not a directory: {}",
                    path.display()
                )
            }
            Self::InvalidRelativePath(path) => {
                write!(formatter, "invalid VDS relative path: {path}")
            }
            Self::MissingManifest(path) => {
                write!(
                    formatter,
                    "VDS 2 workspace manifest is missing: {}",
                    path.display()
                )
            }
            Self::LegacyDatabasePresent(path) => write!(
                formatter,
                "legacy database must be migrated before VDS 2 initialization: {}",
                path.display()
            ),
            Self::UnsupportedFormat {
                path,
                found,
                supported,
            } => write!(
                formatter,
                "unsupported metadata format {found} at {}; supported format is {supported}",
                path.display()
            ),
            Self::DocumentDirectoryMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "document directory {} is {actual:?}, expected {expected:?}",
                path.display()
            ),
            Self::SectionFileMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "section file {} is {actual:?}, expected {expected:?}",
                path.display()
            ),
            Self::MissingRootSection {
                document_id,
                section_id,
            } => write!(
                formatter,
                "managed document {} is missing root section {}",
                document_id.as_str(),
                section_id.as_str()
            ),
            Self::DuplicateDocumentPath {
                relative_path,
                first,
                second,
            } => write!(
                formatter,
                "managed path {relative_path:?} is claimed by {} and {}",
                first.as_str(),
                second.as_str()
            ),
            Self::DuplicateDocumentId(id) => {
                write!(formatter, "duplicate managed document ID {}", id.as_str())
            }
            Self::DuplicateSectionId(id) => write!(formatter, "duplicate section ID {id}"),
            Self::DocumentPathAlreadyManaged(path) => {
                write!(formatter, "document path is already managed: {path}")
            }
            Self::DocumentIdAlreadyManaged(id) => {
                write!(formatter, "document ID is already managed: {}", id.as_str())
            }
            Self::DocumentNotManaged(id) => {
                write!(formatter, "document is not managed: {}", id.as_str())
            }
            Self::SameDocumentPath(path) => {
                write!(
                    formatter,
                    "source and destination paths are the same: {path}"
                )
            }
            Self::CaseOnlyRenameUnsupported {
                source,
                destination,
            } => write!(
                formatter,
                "case-only rename from {source:?} to {destination:?} is not yet supported"
            ),
            Self::ContentHashConflict {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "content changed at {}; expected {expected}, found {actual}",
                path.display()
            ),
            Self::DestinationExists(path) => {
                write!(formatter, "destination already exists: {}", path.display())
            }
            Self::MissingDestinationParent(path) => write!(
                formatter,
                "destination parent does not exist: {}",
                path.display()
            ),
            Self::RecoveryConflict {
                transaction,
                source_exists,
                destination_exists,
            } => write!(
                formatter,
                "cannot recover relocation at {}: source_exists={source_exists}, destination_exists={destination_exists}",
                transaction.display()
            ),
            Self::SectionNotManaged {
                document_id,
                section_id,
            } => write!(
                formatter,
                "section {} is not managed in document {}",
                section_id.as_str(),
                document_id.as_str()
            ),
            Self::ExternalContentConflict {
                document_id,
                path,
                expected_hash,
                actual_hash,
            } => write!(
                formatter,
                "external edit detected on {} (document {}): expected {expected_hash}, found {actual_hash}; reload the document and retry",
                path.display(),
                document_id.as_str()
            ),
            Self::Other(msg) => write!(formatter, "{msg}"),
        }
    }
}

impl std::error::Error for MetadataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, MetadataError>;

/// Error returned when a workspace write lease cannot be acquired.
#[derive(Debug)]
pub enum LeaseError {
    /// Another VDS writer process already holds the lease for this workspace.
    AlreadyHeld {
        lock_path: PathBuf,
        /// PID of the incumbent process as recorded in the lock file, if readable.
        incumbent_pid: Option<u32>,
    },
    Io(io::Error),
}

impl fmt::Display for LeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyHeld {
                lock_path,
                incumbent_pid: Some(pid),
            } => write!(
                f,
                "another VDS writer (PID {pid}) holds the workspace lease at {}; \
                 if that process is no longer running, delete the lock file and retry",
                lock_path.display()
            ),
            Self::AlreadyHeld {
                lock_path,
                incumbent_pid: None,
            } => write!(
                f,
                "another VDS writer holds the workspace lease at {}; \
                 if no VDS process is running, delete the lock file and retry",
                lock_path.display()
            ),
            Self::Io(e) => write!(f, "lease I/O error: {e}"),
        }
    }
}

impl std::error::Error for LeaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Exclusive write lease for one VDS workspace.
///
/// Held for the lifetime of a writable `FilesystemVdsServer`. Stored in the
/// OS temp directory so it is never Git-tracked or Dropbox-synced. Released
/// and the lock file deleted when this value is dropped.
pub struct WorkspaceLease {
    lock_path: PathBuf,
}

impl WorkspaceLease {
    /// Attempts to acquire the exclusive write lease for `workspace_root`.
    ///
    /// Returns `Err(LeaseError::AlreadyHeld)` if the lock file already exists
    /// (meaning another VDS writer is, or recently was, running). If the lock
    /// file is stale (left by a crashed process), delete it manually and retry.
    pub fn acquire(workspace_root: &Path) -> std::result::Result<Self, LeaseError> {
        let canonical = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let lock_path = Self::lock_path_for(&canonical);

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let pid = std::process::id();
                let _ = file.write_all(pid.to_string().as_bytes());
                let _ = file.sync_all();
                Ok(Self { lock_path })
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let incumbent_pid = fs::read_to_string(&lock_path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok());
                Err(LeaseError::AlreadyHeld {
                    lock_path,
                    incumbent_pid,
                })
            }
            Err(e) => Err(LeaseError::Io(e)),
        }
    }

    /// Path to the lock file in the OS temp directory.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    fn lock_path_for(canonical: &Path) -> PathBuf {
        let fingerprint = sha256(canonical.to_string_lossy().as_bytes());
        std::env::temp_dir().join(format!("vds-write-{}.lock", &fingerprint[..16]))
    }
}

impl fmt::Debug for WorkspaceLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceLease")
            .field("lock_path", &self.lock_path)
            .finish()
    }
}

impl Drop for WorkspaceLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_POINT_COUNTER: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// In production this is a no-op eliminated by the optimizer.
/// In tests it decrements the thread-local step counter and returns `Err`
/// when the counter reaches zero, simulating a crash at a durable-write boundary.
fn fail_point() -> Result<()> {
    #[cfg(test)]
    FAIL_POINT_COUNTER.with(|c| {
        if let Some(n) = c.get() {
            if n == 0 {
                return Err(MetadataError::Other(
                    "injected failure at durable step boundary".to_owned(),
                ));
            }
            c.set(Some(n - 1));
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::workspace::WorkspaceState;

    use super::*;

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!("vds-metadata-{name}-{nonce}"));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, relative_path: &str, contents: &str) {
            let path = self.root.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, contents).unwrap();
        }

        fn read(&self, relative_path: &str) -> String {
            fs::read_to_string(self.root.join(relative_path)).unwrap()
        }
    }

    fn set_fail_at_step(n: u32) {
        FAIL_POINT_COUNTER.with(|c| c.set(Some(n)));
    }

    fn clear_fail_point() {
        FAIL_POINT_COUNTER.with(|c| c.set(None));
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn initializes_once_and_preserves_workspace_identity() {
        let workspace = TestWorkspace::new("initialize");
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let first = repository.load_catalog().unwrap().manifest;
        let second = MetadataRepository::initialize(&workspace.root)
            .unwrap()
            .load_catalog()
            .unwrap()
            .manifest;

        assert_eq!(first, second);
        assert_eq!(first.format_version, METADATA_FORMAT_VERSION);
        assert_eq!(
            fs::read_to_string(workspace.root.join(".vds/.gitignore")).unwrap(),
            "/recovery/\n"
        );
    }

    #[test]
    fn refuses_to_mix_json_metadata_with_a_legacy_database() {
        let workspace = TestWorkspace::new("legacy");
        workspace.write(".vds/vds.db", "legacy");

        let error = MetadataRepository::initialize(&workspace.root).unwrap_err();

        assert!(matches!(error, MetadataError::LegacyDatabasePresent(_)));
        assert!(!workspace.root.join(".vds/workspace.json").exists());
    }

    #[test]
    fn promotes_a_materialized_document_and_reloads_its_catalog() {
        let workspace = TestWorkspace::new("promote");
        workspace.write(
            "docs/architecture.md",
            "# Architecture\n\nIntro.\n\n## Storage\n\nFilesystem first.\n",
        );
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("docs/architecture.md").unwrap();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();

        let promoted = repository.promote(materialized).unwrap();
        let catalog = repository.load_catalog().unwrap();
        let reloaded = catalog.document_by_path("docs\\architecture.md").unwrap();

        assert_eq!(promoted.document.document_id, materialized.document.id);
        assert_eq!(reloaded.document.document_id, materialized.document.id);
        assert_eq!(reloaded.sections.len(), materialized.sections.len());
        assert_eq!(reloaded.current.root_section_id, materialized.document.root);
        let document_root = workspace
            .root
            .join(".vds/documents")
            .join(materialized.document.id.as_str());
        assert!(document_root.join("document.json").exists());
        assert!(document_root.join("current.json").exists());
        assert!(
            document_root
                .join("versions")
                .join(materialized.document.root.as_str())
                .exists()
        );
        assert!(
            fs::read_dir(workspace.root.join(".vds/recovery"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn recovers_when_markdown_moved_before_metadata_was_published() {
        let workspace = TestWorkspace::new("recover-relocation");
        workspace.write("docs/architecture.md", "# Architecture\n\nRecover me.\n");
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("docs/architecture.md").unwrap();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        repository.promote(materialized).unwrap();
        let managed = repository
            .load_catalog()
            .unwrap()
            .document_by_id(&materialized.document.id)
            .unwrap()
            .clone();

        let transaction_root = repository.recovery_root().join("interrupted-move");
        fs::create_dir_all(transaction_root.join("staged")).unwrap();
        let intent = RelocationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: "interrupted-move".to_owned(),
            transaction_kind: "relocation".to_owned(),
            document_id: materialized.document.id.clone(),
            source_relative_path: "docs/architecture.md".to_owned(),
            destination_relative_path: "docs/recovered.md".to_owned(),
            expected_content_hash: managed.current.content_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();
        let mut updated = managed.document;
        updated.relative_path = "docs/recovered.md".to_owned();
        write_json_create_new(&transaction_root.join("staged/document.json"), &updated).unwrap();
        fs::rename(
            workspace.root.join("docs/architecture.md"),
            workspace.root.join("docs/recovered.md"),
        )
        .unwrap();

        repository.recover_transactions().unwrap();

        assert!(
            repository
                .load_catalog()
                .unwrap()
                .document_by_path("docs/recovered.md")
                .is_some()
        );
        assert!(!transaction_root.exists());
    }

    #[test]
    fn recovers_interrupted_content_mutation_by_completing_the_commit() {
        use crate::document::{SectionMetadata, VersionId};

        let workspace = TestWorkspace::new("recover-content");
        let original = "# Notes\n\nOriginal content.\n";
        let new_content = "# Notes\n\nUpdated content.\n";
        workspace.write("notes.md", original);
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("notes.md").unwrap();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let promoted = repository.promote(materialized).unwrap();
        let section_id = promoted.current.root_section_id.clone();

        let original_hash = sha256(original.as_bytes());
        let new_hash = sha256(new_content.as_bytes());
        let txn_id = "interrupted-content-mutation";
        let transaction_root = repository.recovery_root().join(txn_id);
        let staged = transaction_root.join("staged");
        fs::create_dir_all(&staged).unwrap();

        let version_id = VersionId::new("v-recovery-test");
        let meta = SectionMetadata {
            anchor: None,
            tags: vec![],
            summary: None,
            locked: false,
        };
        let version = SectionVersion {
            version_id: version_id.clone(),
            section_id: section_id.clone(),
            title: "Notes".to_owned(),
            content: "Updated content.".to_owned(),
            metadata: meta.clone(),
            embedding: None,
            author: None,
            change_summary: Some("recovery test".to_owned()),
            created_at: Utc::now(),
        };
        let base_record = promoted.sections.get(&section_id).unwrap();
        let updated_section_record = SectionRecord {
            format_version: base_record.format_version,
            section_id: section_id.clone(),
            current_version: version_id.clone(),
            metadata: meta,
            matching: base_record.matching.clone(),
            updated_at: Utc::now(),
        };

        let intent = ContentMutationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "content_mutation".to_owned(),
            mutation_kind: "replace_content".to_owned(),
            document_id: promoted.document.document_id.clone(),
            relative_path: promoted.document.relative_path.clone(),
            section_id: section_id.clone(),
            expected_content_hash: original_hash,
            new_content_hash: new_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();
        write_json_create_new(
            &staged.join(format!("{}.json", version_id.as_str())),
            &version,
        )
        .unwrap();
        write_bytes_create_new(&staged.join("markdown"), new_content.as_bytes()).unwrap();
        write_json_create_new(
            &staged.join(format!("section-{}.json", section_id.as_str())),
            &updated_section_record,
        )
        .unwrap();

        // Simulate crash before markdown was written — disk still has original content.
        repository.recover_transactions().unwrap();

        let disk = fs::read_to_string(workspace.root.join("notes.md")).unwrap();
        assert_eq!(disk, new_content);
        assert!(!transaction_root.exists());
    }

    #[test]
    fn recovers_interrupted_content_mutation_by_rolling_back_when_disk_is_unchanged() {
        let workspace = TestWorkspace::new("recover-content-rollback");
        let original = "# Doc\n\nOriginal.\n";
        workspace.write("doc.md", original);
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("doc.md").unwrap();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let promoted = repository.promote(materialized).unwrap();
        let section_id = promoted.current.root_section_id.clone();

        let original_hash = sha256(original.as_bytes());
        let txn_id = "interrupted-content-no-write";
        let transaction_root = repository.recovery_root().join(txn_id);
        let staged = transaction_root.join("staged");
        fs::create_dir_all(&staged).unwrap();

        let intent = ContentMutationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "content_mutation".to_owned(),
            mutation_kind: "replace_content".to_owned(),
            document_id: promoted.document.document_id.clone(),
            relative_path: promoted.document.relative_path.clone(),
            section_id: section_id.clone(),
            expected_content_hash: original_hash.clone(),
            new_content_hash: "some-future-hash".to_owned(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();
        // No staged markdown — crash happened before staging was complete.

        repository.recover_transactions().unwrap();

        // Disk is unchanged and the transaction dir was safely removed.
        let disk = fs::read_to_string(workspace.root.join("doc.md")).unwrap();
        assert_eq!(disk, original);
        assert!(!transaction_root.exists());
    }

    #[test]
    fn recovers_interrupted_promotion_by_completing_the_rename() {
        let workspace = TestWorkspace::new("recover-promotion");
        workspace.write("guide.md", "# Guide\n\nContent.\n");
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("guide.md").unwrap();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();

        // Stage a promotion transaction manually (as if the rename crashed).
        let txn_id = "interrupted-promotion";
        let transaction_root = repository.recovery_root().join(txn_id);
        let staged_document = transaction_root.join("staged/document");
        fs::create_dir_all(staged_document.join("sections")).unwrap();
        fs::create_dir_all(staged_document.join("versions")).unwrap();
        fs::create_dir_all(staged_document.join("snapshots")).unwrap();

        let doc_id = materialized.document.id.clone();
        let intent = PromotionIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "promotion".to_owned(),
            document_id: doc_id.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();

        use crate::document::VersionId;
        let doc_record = DocumentRecord {
            format_version: METADATA_FORMAT_VERSION,
            document_id: doc_id.clone(),
            name: None,
            title: Some("Guide".to_owned()),
            description: None,
            tags: vec![],
            relative_path: "guide.md".to_owned(),
            created_at: Utc::now(),
        };
        write_json_create_new(&staged_document.join("document.json"), &doc_record).unwrap();
        let current_record = CurrentDocumentRecord {
            format_version: METADATA_FORMAT_VERSION,
            root_section_id: materialized.document.root.clone(),
            current_document_version: VersionId::new_v7(),
            content_hash: sha256(b"# Guide\n\nContent.\n"),
            updated_at: Utc::now(),
        };
        write_json_create_new(&staged_document.join("current.json"), &current_record).unwrap();

        repository.recover_transactions().unwrap();

        let final_root = workspace.root.join(".vds/documents").join(doc_id.as_str());
        assert!(
            final_root.exists(),
            "document directory should exist after recovery"
        );
        assert!(final_root.join("document.json").exists());
        assert!(!transaction_root.exists());
    }

    #[test]
    fn workspace_lease_rejects_second_writer_on_same_workspace() {
        let workspace = TestWorkspace::new("lease-exclusion");
        let canonical = workspace.root.canonicalize().unwrap();

        let first = WorkspaceLease::acquire(&canonical).expect("first lease should succeed");
        let second = WorkspaceLease::acquire(&canonical);

        assert!(
            matches!(second, Err(LeaseError::AlreadyHeld { .. })),
            "second lease attempt should return AlreadyHeld"
        );

        drop(first);
        // After dropping the first lease the lock file is removed; a new
        // acquisition on the same path must succeed.
        let third = WorkspaceLease::acquire(&canonical);
        assert!(
            third.is_ok(),
            "lease should be re-acquirable after previous holder drops it"
        );
    }

    #[test]
    fn apply_content_mutation_rejects_external_edit_on_disk() {
        let workspace = TestWorkspace::new("external-edit-conflict");
        let original = "# Doc\n\nVersion A.\n";
        let external_edit = "# Doc\n\nExternal edit.\n";
        let vds_edit = "# Doc\n\nVersion B.\n";
        workspace.write("doc.md", original);
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("doc.md").unwrap();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let promoted = repository.promote(materialized).unwrap();

        // Simulate an external process modifying the file after VDS last read it.
        workspace.write("doc.md", external_edit);

        // Now attempt a VDS mutation that was computed against the original hash.
        let section_id = promoted.current.root_section_id.clone();
        let original_hash = sha256(original.as_bytes());
        let new_hash = sha256(vds_edit.as_bytes());
        let txn_id = "vds-content-mutation";
        let transaction_root = repository.recovery_root().join(txn_id);
        let staged = transaction_root.join("staged");
        fs::create_dir_all(&staged).unwrap();

        let intent = ContentMutationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "content_mutation".to_owned(),
            mutation_kind: "replace_content".to_owned(),
            document_id: promoted.document.document_id.clone(),
            relative_path: promoted.document.relative_path.clone(),
            section_id: section_id.clone(),
            expected_content_hash: original_hash,
            new_content_hash: new_hash,
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();
        write_bytes_create_new(&staged.join("markdown"), vds_edit.as_bytes()).unwrap();

        let base = promoted.sections.get(&section_id).unwrap();
        let section_record = SectionRecord {
            format_version: base.format_version,
            section_id: section_id.clone(),
            current_version: base.current_version.clone(),
            metadata: base.metadata.clone(),
            matching: base.matching.clone(),
            updated_at: Utc::now(),
        };
        write_json_create_new(
            &staged.join(format!("section-{}.json", section_id.as_str())),
            &section_record,
        )
        .unwrap();

        let result = repository.recover_transactions();
        assert!(
            matches!(result, Err(MetadataError::ExternalContentConflict { .. })),
            "expected ExternalContentConflict, got: {result:?}"
        );
        // The external edit must be preserved on disk.
        let on_disk = fs::read_to_string(workspace.root.join("doc.md")).unwrap();
        assert_eq!(on_disk, external_edit);
    }

    #[test]
    fn recovers_interrupted_soft_deletion_after_metadata_was_moved() {
        let workspace = TestWorkspace::new("recover-soft-delete");
        workspace.write("report.md", "# Report\n\nContent.\n");
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("report.md").unwrap();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let promoted = repository.promote(materialized).unwrap();
        let doc_id = promoted.document.document_id.clone();
        let content_hash = promoted.current.content_hash.clone();

        // Simulate: tombstone written and metadata moved, but markdown not yet deleted.
        let inactive_dir = workspace.root.join(".vds/inactive").join(doc_id.as_str());
        let archive_dir = inactive_dir.join("archive");
        fs::create_dir_all(&archive_dir).unwrap();
        let tombstone = DeletionTombstone {
            format_version: METADATA_FORMAT_VERSION,
            document_id: doc_id.clone(),
            previous_relative_path: "report.md".to_owned(),
            content_hash: content_hash.clone(),
            operation: "remove".to_owned(),
            archived_history: true,
            created_at: Utc::now(),
        };
        write_json_create_new(&inactive_dir.join("tombstone.json"), &tombstone).unwrap();
        let document_root = workspace.root.join(".vds/documents").join(doc_id.as_str());
        fs::rename(&document_root, archive_dir.join("metadata")).unwrap();
        // Markdown file still on disk — crash before delete.

        // Write the recovery intent so recover_transactions knows this is in progress.
        let txn_id = "interrupted-soft-delete";
        let transaction_root = repository.recovery_root().join(txn_id);
        fs::create_dir_all(&transaction_root).unwrap();
        let intent = SoftDeletionIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "soft_deletion".to_owned(),
            sub_kind: "remove".to_owned(),
            document_id: doc_id.clone(),
            relative_path: "report.md".to_owned(),
            content_hash: content_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();

        repository.recover_transactions().unwrap();

        assert!(
            !workspace.root.join("report.md").exists(),
            "markdown should be deleted"
        );
        assert!(!transaction_root.exists(), "recovery dir should be removed");
        assert!(
            inactive_dir.join("archive/metadata").exists(),
            "archive should remain"
        );
    }

    #[test]
    fn recovers_interrupted_unmanage_before_metadata_was_moved() {
        let workspace = TestWorkspace::new("recover-unmanage");
        workspace.write("notes.md", "# Notes\n\nKeep this.\n");
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("notes.md").unwrap();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let promoted = repository.promote(materialized).unwrap();
        let doc_id = promoted.document.document_id.clone();
        let content_hash = promoted.current.content_hash.clone();

        // Simulate: tombstone written but rename not yet done.
        let inactive_dir = workspace.root.join(".vds/inactive").join(doc_id.as_str());
        let archive_dir = inactive_dir.join("archive");
        fs::create_dir_all(&archive_dir).unwrap();
        let tombstone = DeletionTombstone {
            format_version: METADATA_FORMAT_VERSION,
            document_id: doc_id.clone(),
            previous_relative_path: "notes.md".to_owned(),
            content_hash: content_hash.clone(),
            operation: "unmanage".to_owned(),
            archived_history: true,
            created_at: Utc::now(),
        };
        write_json_create_new(&inactive_dir.join("tombstone.json"), &tombstone).unwrap();
        // documents/{id}/ still exists — crash before rename.

        let txn_id = "interrupted-unmanage";
        let transaction_root = repository.recovery_root().join(txn_id);
        fs::create_dir_all(&transaction_root).unwrap();
        let intent = SoftDeletionIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "soft_deletion".to_owned(),
            sub_kind: "unmanage_archive".to_owned(),
            document_id: doc_id.clone(),
            relative_path: "notes.md".to_owned(),
            content_hash: content_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();

        repository.recover_transactions().unwrap();

        // Markdown must still be on disk; metadata must be in archive.
        assert!(
            workspace.root.join("notes.md").exists(),
            "markdown should be preserved"
        );
        let document_root = workspace.root.join(".vds/documents").join(doc_id.as_str());
        assert!(
            !document_root.exists(),
            "active document dir should be gone"
        );
        assert!(
            inactive_dir.join("archive/metadata").exists(),
            "archive should contain metadata"
        );
        assert!(!transaction_root.exists());
    }

    // --- Failure injection tests ----------------------------------------------------------------

    #[test]
    fn failure_injection_promote_recovers_at_all_steps() {
        // Two injected fail points: step 0 = before rename (staged complete),
        // step 1 = after rename (orphaned transaction dir). Both should leave the
        // document fully managed after recovery.
        for step in 0..2u32 {
            clear_fail_point();
            let workspace = TestWorkspace::new(&format!("inject-promote-{step}"));
            workspace.write("doc.md", "# Hello\n\nContent.\n");
            let state = WorkspaceState::load(&workspace.root).unwrap();
            let materialized = state.document_by_path("doc.md").unwrap().clone();
            let repository = MetadataRepository::initialize(&workspace.root).unwrap();

            set_fail_at_step(step);
            let _ = repository.promote(&materialized);
            clear_fail_point();

            let repo2 = MetadataRepository::initialize(&workspace.root).unwrap();
            repo2.recover_transactions().unwrap();

            let recovery_root = workspace.root.join(".vds/recovery");
            if recovery_root.exists() {
                assert_eq!(
                    fs::read_dir(&recovery_root).unwrap().count(),
                    0,
                    "orphaned transaction at promote step {step}"
                );
            }
            let catalog = repo2.load_catalog().unwrap();
            assert!(
                catalog.document_by_id(&materialized.document.id).is_some(),
                "document should be managed after recovery at promote step {step}"
            );
        }
    }

    #[test]
    fn failure_injection_content_mutation_staging_recovers_at_all_steps() {
        use crate::document::{SectionMetadata, VersionId};

        let original = "# Notes\n\nOriginal content.\n";
        let new_content = "# Notes\n\nUpdated content.\n";

        // 4 fail points in commit_section_mutation staging (steps 0-3):
        //   0: after intent.json        → no staged markdown → rollback
        //   1: after version JSON       → no staged markdown → rollback
        //   2: after staged markdown    → no section record  → rollback
        //   3: after staged section rec → staging complete   → recovery completes
        for step in 0..4u32 {
            clear_fail_point();
            let workspace = TestWorkspace::new(&format!("inject-content-{step}"));
            workspace.write("notes.md", original);
            let state = WorkspaceState::load(&workspace.root).unwrap();
            let materialized = state.document_by_path("notes.md").unwrap().clone();
            let repository = MetadataRepository::initialize(&workspace.root).unwrap();
            let promoted = repository.promote(&materialized).unwrap();
            let section_id = promoted.current.root_section_id.clone();
            let original_hash = promoted.current.content_hash.clone();

            let version_id = VersionId::new("v-inject");
            let meta = SectionMetadata {
                anchor: None,
                tags: vec![],
                summary: None,
                locked: false,
            };
            let version = SectionVersion {
                version_id: version_id.clone(),
                section_id: section_id.clone(),
                title: "Notes".to_owned(),
                content: "Updated content.".to_owned(),
                metadata: meta.clone(),
                embedding: None,
                author: None,
                change_summary: None,
                created_at: Utc::now(),
            };
            let base = promoted.sections.get(&section_id).unwrap();
            let updated_record = SectionRecord {
                format_version: base.format_version,
                section_id: section_id.clone(),
                current_version: version_id.clone(),
                metadata: meta,
                matching: base.matching.clone(),
                updated_at: Utc::now(),
            };

            set_fail_at_step(step);
            let _ = repository.commit_section_mutation(
                &promoted.document.document_id,
                &section_id,
                &original_hash,
                new_content,
                "replace_content",
                &version,
                &updated_record,
            );
            clear_fail_point();

            let repo2 = MetadataRepository::initialize(&workspace.root).unwrap();
            repo2.recover_transactions().unwrap();

            let recovery_root = workspace.root.join(".vds/recovery");
            if recovery_root.exists() {
                assert_eq!(
                    fs::read_dir(&recovery_root).unwrap().count(),
                    0,
                    "orphaned transaction at content-mutation step {step}"
                );
            }

            // Disk and catalog must agree on hash — the key invariant.
            let catalog = repo2.load_catalog().unwrap();
            let managed = catalog
                .document_by_id(&promoted.document.document_id)
                .unwrap();
            let disk = fs::read_to_string(workspace.root.join("notes.md")).unwrap();
            let disk_hash = sha256(disk.as_bytes());
            assert_eq!(
                managed.current.content_hash, disk_hash,
                "hash mismatch after recovery at content-mutation step {step}"
            );
            // Content must be one of the two valid states.
            assert!(
                disk == original || disk == new_content,
                "unexpected disk content at step {step}"
            );
        }
    }

    #[test]
    fn content_mutation_after_rename_crash_recovers_metadata_updates() {
        use crate::document::{SectionMetadata, VersionId};

        // Simulates a crash that happened AFTER the markdown was renamed to the new
        // content but BEFORE current.json and the section record were updated. This
        // covers the recovery path fixed to detect "disk == new_hash AND staged
        // section record still present".
        let workspace = TestWorkspace::new("recover-after-rename");
        let original = "# Log\n\nOriginal.\n";
        let new_content = "# Log\n\nUpdated.\n";
        workspace.write("log.md", original);
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("log.md").unwrap().clone();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let promoted = repository.promote(&materialized).unwrap();
        let section_id = promoted.current.root_section_id.clone();

        let original_hash = sha256(original.as_bytes());
        let new_hash = sha256(new_content.as_bytes());
        let txn_id = "after-rename-crash";
        let transaction_root = repository.recovery_root().join(txn_id);
        let staged = transaction_root.join("staged");
        fs::create_dir_all(&staged).unwrap();

        let version_id = VersionId::new("v-after-rename");
        let meta = SectionMetadata {
            anchor: None,
            tags: vec![],
            summary: None,
            locked: false,
        };
        let version = SectionVersion {
            version_id: version_id.clone(),
            section_id: section_id.clone(),
            title: "Log".to_owned(),
            content: "Updated.".to_owned(),
            metadata: meta.clone(),
            embedding: None,
            author: None,
            change_summary: None,
            created_at: Utc::now(),
        };
        let base = promoted.sections.get(&section_id).unwrap();
        let updated_record = SectionRecord {
            format_version: base.format_version,
            section_id: section_id.clone(),
            current_version: version_id.clone(),
            metadata: meta,
            matching: base.matching.clone(),
            updated_at: Utc::now(),
        };
        let intent = ContentMutationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "content_mutation".to_owned(),
            mutation_kind: "replace_content".to_owned(),
            document_id: promoted.document.document_id.clone(),
            relative_path: promoted.document.relative_path.clone(),
            section_id: section_id.clone(),
            expected_content_hash: original_hash,
            new_content_hash: new_hash.clone(),
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();
        write_json_create_new(
            &staged.join(format!("{}.json", version_id.as_str())),
            &version,
        )
        .unwrap();
        write_json_create_new(
            &staged.join(format!("section-{}.json", section_id.as_str())),
            &updated_record,
        )
        .unwrap();
        write_bytes_create_new(&staged.join("markdown"), new_content.as_bytes()).unwrap();

        // Simulate: markdown was already renamed to new content before crash.
        workspace.write("log.md", new_content);

        repository.recover_transactions().unwrap();

        // Metadata must now reflect the new content.
        let catalog = repository.load_catalog().unwrap();
        let managed = catalog
            .document_by_id(&promoted.document.document_id)
            .unwrap();
        assert_eq!(
            managed.current.content_hash, new_hash,
            "current.json should reflect new hash"
        );
        assert_eq!(
            managed.sections.get(&section_id).unwrap().current_version,
            version_id,
            "section record should point to new version"
        );
        assert!(
            !transaction_root.exists(),
            "transaction dir should be cleaned up"
        );
    }

    #[test]
    fn failure_injection_soft_deletion_recovers_at_all_steps() {
        // 4 fail points in execute_remove_document_file (steps 0-3):
        //   0: after tombstone written
        //   1: after content copied to archive
        //   2: after metadata renamed to archive
        //   3: after markdown removed
        // Recovery re-runs execute_remove_document_file idempotently.
        for step in 0..4u32 {
            clear_fail_point();
            let workspace = TestWorkspace::new(&format!("inject-remove-{step}"));
            workspace.write("doc.md", "# Doc\n\nContent.\n");
            let state = WorkspaceState::load(&workspace.root).unwrap();
            let materialized = state.document_by_path("doc.md").unwrap().clone();
            let repository = MetadataRepository::initialize(&workspace.root).unwrap();
            let promoted = repository.promote(&materialized).unwrap();
            let hash = promoted.current.content_hash.clone();
            let doc_id = promoted.document.document_id.clone();

            set_fail_at_step(step);
            let _ = repository.remove_document_file(&doc_id, &hash);
            clear_fail_point();

            let repo2 = MetadataRepository::initialize(&workspace.root).unwrap();
            repo2.recover_transactions().unwrap();

            let recovery_root = workspace.root.join(".vds/recovery");
            if recovery_root.exists() {
                assert_eq!(
                    fs::read_dir(&recovery_root).unwrap().count(),
                    0,
                    "orphaned transaction at soft-deletion step {step}"
                );
            }

            // After successful removal (or recovery), markdown must be gone and
            // tombstone must exist.
            assert!(
                !workspace.root.join("doc.md").exists(),
                "markdown should be gone after remove recovery at step {step}"
            );
            let inactive = workspace.root.join(".vds/inactive").join(doc_id.as_str());
            assert!(
                inactive.join("tombstone.json").exists(),
                "tombstone should exist at step {step}"
            );
        }
    }

    // --- Three-way merge tests -----------------------------------------------------------------

    #[test]
    fn three_way_merge_applies_non_conflicting_external_edit() {
        use crate::document::{SectionMetadata, VersionId};

        // Our mutation edits line A; an external edit changes line C independently.
        // diffy should merge them cleanly without conflict markers.
        let original = "# Doc\n\nLine A.\nLine B.\nLine C.\n";
        let externally = "# Doc\n\nLine A.\nLine B.\nLine C (external).\n";
        let ours = "# Doc\n\nLine A (ours).\nLine B.\nLine C.\n";
        // Expected: both edits present
        let expected_merged = "# Doc\n\nLine A (ours).\nLine B.\nLine C (external).\n";

        let workspace = TestWorkspace::new("three-way-merge");
        workspace.write("doc.md", original);
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("doc.md").unwrap().clone();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let promoted = repository.promote(&materialized).unwrap();
        let section_id = promoted.current.root_section_id.clone();

        let original_hash = sha256(original.as_bytes());
        let new_hash = sha256(ours.as_bytes());
        let txn_id = "three-way-merge-txn";
        let transaction_root = repository.recovery_root().join(txn_id);
        let staged = transaction_root.join("staged");
        fs::create_dir_all(&staged).unwrap();

        let version_id = VersionId::new("v-merge");
        let meta = SectionMetadata {
            anchor: None,
            tags: vec![],
            summary: None,
            locked: false,
        };
        let version = SectionVersion {
            version_id: version_id.clone(),
            section_id: section_id.clone(),
            title: "Doc".to_owned(),
            content: "Line A (ours).\nLine B.\nLine C.".to_owned(),
            metadata: meta.clone(),
            embedding: None,
            author: None,
            change_summary: None,
            created_at: Utc::now(),
        };
        let base = promoted.sections.get(&section_id).unwrap();
        let updated_record = SectionRecord {
            format_version: base.format_version,
            section_id: section_id.clone(),
            current_version: version_id.clone(),
            metadata: meta,
            matching: base.matching.clone(),
            updated_at: Utc::now(),
        };
        let intent = ContentMutationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "content_mutation".to_owned(),
            mutation_kind: "replace_content".to_owned(),
            document_id: promoted.document.document_id.clone(),
            relative_path: promoted.document.relative_path.clone(),
            section_id: section_id.clone(),
            expected_content_hash: original_hash,
            new_content_hash: new_hash,
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();
        write_json_create_new(
            &staged.join(format!("{}.json", version_id.as_str())),
            &version,
        )
        .unwrap();
        write_bytes_create_new(&staged.join("original_markdown"), original.as_bytes()).unwrap();
        write_bytes_create_new(&staged.join("markdown"), ours.as_bytes()).unwrap();
        write_json_create_new(
            &staged.join(format!("section-{}.json", section_id.as_str())),
            &updated_record,
        )
        .unwrap();

        // External edit to a non-overlapping line.
        workspace.write("doc.md", externally);

        repository.recover_transactions().unwrap();

        let disk = workspace.read("doc.md");
        assert_eq!(
            disk, expected_merged,
            "three-way merge should produce merged content"
        );

        let catalog = repository.load_catalog().unwrap();
        let managed = catalog
            .document_by_id(&promoted.document.document_id)
            .unwrap();
        let merged_hash = sha256(expected_merged.as_bytes());
        assert_eq!(
            managed.current.content_hash, merged_hash,
            "catalog should reflect merged hash"
        );
        assert!(
            !transaction_root.exists(),
            "transaction dir should be cleaned up after merge"
        );
    }

    #[test]
    fn three_way_merge_returns_conflict_on_overlapping_edits() {
        use crate::document::{SectionMetadata, VersionId};

        // Both our mutation and the external edit change the same line — conflict.
        let original = "# Doc\n\nThe line.\n";
        let externally = "# Doc\n\nThe line (external).\n";
        let ours = "# Doc\n\nThe line (ours).\n";

        let workspace = TestWorkspace::new("three-way-conflict");
        workspace.write("doc.md", original);
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let materialized = state.document_by_path("doc.md").unwrap().clone();
        let repository = MetadataRepository::initialize(&workspace.root).unwrap();
        let promoted = repository.promote(&materialized).unwrap();
        let section_id = promoted.current.root_section_id.clone();

        let original_hash = sha256(original.as_bytes());
        let new_hash = sha256(ours.as_bytes());
        let txn_id = "three-way-conflict-txn";
        let transaction_root = repository.recovery_root().join(txn_id);
        let staged = transaction_root.join("staged");
        fs::create_dir_all(&staged).unwrap();

        let version_id = VersionId::new("v-conflict");
        let meta = SectionMetadata {
            anchor: None,
            tags: vec![],
            summary: None,
            locked: false,
        };
        let version = SectionVersion {
            version_id: version_id.clone(),
            section_id: section_id.clone(),
            title: "Doc".to_owned(),
            content: "The line (ours).".to_owned(),
            metadata: meta.clone(),
            embedding: None,
            author: None,
            change_summary: None,
            created_at: Utc::now(),
        };
        let base = promoted.sections.get(&section_id).unwrap();
        let updated_record = SectionRecord {
            format_version: base.format_version,
            section_id: section_id.clone(),
            current_version: version_id.clone(),
            metadata: meta,
            matching: base.matching.clone(),
            updated_at: Utc::now(),
        };
        let intent = ContentMutationIntent {
            format_version: METADATA_FORMAT_VERSION,
            transaction_id: txn_id.to_owned(),
            transaction_kind: "content_mutation".to_owned(),
            mutation_kind: "replace_content".to_owned(),
            document_id: promoted.document.document_id.clone(),
            relative_path: promoted.document.relative_path.clone(),
            section_id: section_id.clone(),
            expected_content_hash: original_hash,
            new_content_hash: new_hash,
            created_at: Utc::now(),
        };
        write_json_create_new(&transaction_root.join("intent.json"), &intent).unwrap();
        write_json_create_new(
            &staged.join(format!("{}.json", version_id.as_str())),
            &version,
        )
        .unwrap();
        write_bytes_create_new(&staged.join("original_markdown"), original.as_bytes()).unwrap();
        write_bytes_create_new(&staged.join("markdown"), ours.as_bytes()).unwrap();
        write_json_create_new(
            &staged.join(format!("section-{}.json", section_id.as_str())),
            &updated_record,
        )
        .unwrap();

        // External edit to the SAME line — should produce an irreconcilable conflict.
        workspace.write("doc.md", externally);

        let result = repository.recover_transactions();
        assert!(
            matches!(result, Err(MetadataError::ExternalContentConflict { .. })),
            "expected ExternalContentConflict, got: {result:?}"
        );
        // Disk preserves the external edit; transaction dir remains for inspection.
        assert_eq!(
            workspace.read("doc.md"),
            externally,
            "external edit should be preserved"
        );
        assert!(
            transaction_root.exists(),
            "transaction dir should remain on conflict"
        );
    }
}
