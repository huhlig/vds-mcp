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

use chrono::{DateTime, Utc};
use serde::de::{self, Error as DeError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Stable identifier for a document managed by the Versioned Document Service.
///
/// A document ID is intended to survive renames, metadata edits, imports, and
/// exports so callers can address the same logical document over time.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DocumentId(String);

impl DocumentId {
    /// Creates a document ID from an owned string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Creates a compact, time-sortable document ID from a UUIDv7 value.
    pub fn new_v7() -> Self {
        Self(prefixed_base62_uuid("doc"))
    }

    /// Returns the string representation used for storage keys.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identifier for a section within a document tree.
///
/// Section IDs are the primary addressing mechanism for edits. They should
/// remain stable when a section is renamed, moved, or has its content changed.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SectionId(String);

impl SectionId {
    /// Creates a section ID from an owned string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Creates a compact, time-sortable section ID from a UUIDv7 value.
    pub fn new_v7() -> Self {
        Self(prefixed_base62_uuid("sec"))
    }

    /// Returns the string representation used for storage keys.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifier for an immutable version of document or section state.
///
/// Version IDs support optimistic concurrency checks and history operations such
/// as diffing or restoring a prior section revision.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct VersionId(String);

impl VersionId {
    /// Creates a version ID from an owned string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Creates a compact, time-sortable version ID from a UUIDv7 value.
    pub fn new_v7() -> Self {
        Self(prefixed_base62_uuid("ver"))
    }

    /// Returns the string representation used for storage keys.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifier for a document-level snapshot.
///
/// Snapshots capture a named point in time for the full document tree, allowing
/// callers to compare or restore larger units than a single section.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SnapshotId(String);

impl SnapshotId {
    /// Creates a snapshot ID from an owned string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Creates a compact, time-sortable snapshot ID from a UUIDv7 value.
    pub fn new_v7() -> Self {
        Self(prefixed_base62_uuid("snap"))
    }

    /// Returns the string representation used for storage keys.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifier for an individual change record.
///
/// Revisions describe edits made to the document model and can be used for audit
/// trails, recent-change views, or higher-level synchronization.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RevisionId(String);

impl RevisionId {
    /// Creates a revision ID from an owned string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Creates a compact, time-sortable revision ID from a UUIDv7 value.
    pub fn new_v7() -> Self {
        Self(prefixed_base62_uuid("rev"))
    }

    /// Returns the string representation used for storage keys.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A versioned document stored internally as a tree of sections.
///
/// Markdown is treated as an import/export format at the boundary. The durable
/// internal representation is a root section plus metadata, version state, and
/// timestamps for the document as a whole.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    /// Durable document identifier.
    pub id: DocumentId,
    /// Human-readable storage name for the document.
    pub name: String,
    /// Root section of the document tree.
    pub root: SectionId,
    /// Current document-level version.
    pub current_version: VersionId,
    /// Descriptive and format metadata for the document.
    pub metadata: DocumentMetadata,
    /// Optional semantic embedding for document-level search.
    #[serde(default)]
    pub embedding: Option<TextEmbedding>,
    /// Time when the document was created.
    pub created_at: DateTime<Utc>,
    /// Time when the document was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Descriptive metadata attached to a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentMetadata {
    /// Optional display title distinct from the storage name.
    pub title: Option<String>,
    /// Optional summary of the document's purpose or contents.
    pub description: Option<String>,
    /// Searchable labels associated with the document.
    pub tags: Vec<String>,
    /// Original path used when the document was imported, if any.
    pub source_path: Option<String>,
    /// Boundary serialization format for import and export.
    pub format: DocumentFormat,
}

/// Serialization format used at document boundaries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DocumentFormat {
    /// Markdown document format.
    Markdown,
}

/// Dense vector representation used by semantic search.
///
/// The field is intentionally always present in serialized key objects, even
/// when semantic search support is not compiled in, so stored JSON and MCP
/// shapes remain compatible across feature combinations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextEmbedding {
    /// Embedding model or provider identifier, if known.
    pub model: Option<String>,
    /// Vector components in model-native order.
    pub vector: Vec<f32>,
}

/// A single addressable node in a document tree.
///
/// Sections carry stable identity, parent/child relationships, heading metadata,
/// content, and their own current version. Agents should generally edit
/// sections instead of regenerating an entire document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Section {
    /// Durable section identifier.
    pub section_id: SectionId,
    /// Document that owns this section.
    pub document_id: DocumentId,
    /// Parent section, or `None` for the root section.
    pub parent_id: Option<SectionId>,
    /// Ordered child sections.
    pub children: Vec<SectionId>,

    /// Section heading text.
    pub title: String,
    /// Markdown heading level represented by this section.
    pub level: u8,
    /// Body content belonging directly to this section.
    pub content: String,

    /// Position among siblings under the same parent.
    pub ordinal: u32,
    /// Current section-level version.
    pub current_version: VersionId,

    /// Section-specific metadata used for navigation and editing.
    pub metadata: SectionMetadata,
    /// Optional semantic embedding for section title and content.
    #[serde(default)]
    pub embedding: Option<TextEmbedding>,
    /// Time when the section was created.
    pub created_at: DateTime<Utc>,
    /// Time when the section was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Metadata attached to an individual section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionMetadata {
    /// Stable or generated markdown anchor for the section heading.
    pub anchor: Option<String>,
    /// Searchable labels associated with the section.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional short summary of the section's content.
    pub summary: Option<String>,
    /// Whether editing should be blocked or require explicit override.
    #[serde(default)]
    pub locked: bool,
}

/// Lightweight section summary returned by navigation and mutation commands.
///
/// This type avoids returning full section content when a caller only needs the
/// section's identity, placement, version, and update state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionInfo {
    /// Durable section identifier.
    pub section_id: SectionId,
    /// Parent section, or `None` for the root section.
    pub parent_id: Option<SectionId>,
    /// Section heading text.
    pub title: String,
    /// Markdown heading level represented by this section.
    pub level: u8,
    /// Position among siblings under the same parent.
    pub ordinal: u32,
    /// Current section-level version.
    pub current_version: VersionId,
    /// Number of direct child sections.
    pub child_count: usize,
    /// Time when the section was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Immutable historical version of a section.
///
/// A section version captures the title, body content, metadata, author, and
/// change summary for one committed edit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionVersion {
    /// Identifier for this immutable section version.
    pub version_id: VersionId,
    /// Section this version belongs to.
    pub section_id: SectionId,
    /// Section title at the time this version was created.
    pub title: String,
    /// Section content at the time this version was created.
    pub content: String,
    /// Section metadata at the time this version was created.
    pub metadata: SectionMetadata,
    /// Optional semantic embedding captured with this section version.
    #[serde(default)]
    pub embedding: Option<TextEmbedding>,
    /// Time when this version was created.
    pub created_at: DateTime<Utc>,
    /// Optional actor responsible for the change.
    pub author: Option<String>,
    /// Optional human-readable description of the change.
    pub change_summary: Option<String>,
}

/// Document-level snapshot that captures a full-tree point in time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentSnapshot {
    /// Identifier for this snapshot.
    pub snapshot_id: SnapshotId,
    /// Document captured by this snapshot.
    pub document_id: DocumentId,
    /// Root version that anchors the captured document tree.
    pub root_version: VersionId,
    /// Complete section tree captured at snapshot time.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// Optional display label for the snapshot.
    pub label: Option<String>,
    /// Time when the snapshot was created.
    pub created_at: DateTime<Utc>,
    /// Optional actor responsible for creating the snapshot.
    pub author: Option<String>,
    /// Optional human-readable description of the snapshot.
    pub change_summary: Option<String>,
}

/// Recursive entry in a document table of contents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TableOfContentsEntry {
    /// Section represented by this entry.
    pub section_id: SectionId,
    /// Section heading text.
    pub title: String,
    /// Markdown heading level represented by this entry.
    pub level: u8,
    /// Position among siblings under the same parent.
    pub ordinal: u32,
    /// Nested child entries in document order.
    pub children: Vec<TableOfContentsEntry>,
}

/// Ordered set of operations to apply to a section.
///
/// Patches let callers make targeted edits to a section without replacing the
/// entire section content.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionPatch {
    /// Operations applied in order.
    pub operations: Vec<PatchOp>,
}

/// A single targeted section mutation.
#[derive(Clone, Debug, Serialize)]
pub enum PatchOp {
    /// Replace the entire section body.
    ReplaceContent {
        /// New section body content.
        content: String,
    },
    /// Append content to the existing section body.
    AppendContent {
        /// Content to append.
        content: String,
    },
    /// Prepend content to the existing section body.
    PrependContent {
        /// Content to prepend.
        content: String,
    },
    /// Replace a byte range in the existing section body.
    ReplaceRange {
        /// Inclusive start byte offset.
        start: usize,
        /// Exclusive end byte offset.
        end: usize,
        /// Replacement content.
        content: String,
    },
    /// Rename the section heading.
    Rename {
        /// New section title.
        title: String,
    },
    /// Replace the section metadata.
    SetMetadata {
        /// New section metadata.
        metadata: SectionMetadata,
    },
}

impl<'de> Deserialize<'de> for PatchOp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        if let serde_json::Value::Object(object) = &mut value {
            if let Some(kind) = object.remove("type") {
                let kind = kind
                    .as_str()
                    .ok_or_else(|| D::Error::custom("patch operation type must be a string"))?;
                return patch_op_from_parts(kind, serde_json::Value::Object(object.clone()))
                    .map_err(D::Error::custom);
            }

            if object.len() == 1 {
                let (kind, payload) = object
                    .iter()
                    .next()
                    .map(|(kind, payload)| (kind.clone(), payload.clone()))
                    .expect("checked len");
                return patch_op_from_parts(&kind, payload).map_err(D::Error::custom);
            }
        }

        Err(D::Error::custom(
            "patch operation must be either {\"Variant\": {...}} or {\"type\": \"...\", ...}",
        ))
    }
}

fn patch_op_from_parts(
    kind: &str,
    payload: serde_json::Value,
) -> Result<PatchOp, serde_json::Error> {
    #[derive(Deserialize)]
    struct ContentPayload {
        content: String,
    }

    #[derive(Deserialize)]
    struct RangePayload {
        start: usize,
        end: usize,
        content: String,
    }

    #[derive(Deserialize)]
    struct RenamePayload {
        title: String,
    }

    #[derive(Deserialize)]
    struct MetadataPayload {
        metadata: SectionMetadata,
    }

    fn unknown(kind: &str) -> serde_json::Error {
        de::Error::custom(format!("unknown patch operation: {kind}"))
    }

    match normalize_patch_op_kind(kind).as_str() {
        "replacecontent" => serde_json::from_value::<ContentPayload>(payload).map(|payload| {
            PatchOp::ReplaceContent {
                content: payload.content,
            }
        }),
        "appendcontent" | "append" => {
            serde_json::from_value::<ContentPayload>(payload).map(|payload| {
                PatchOp::AppendContent {
                    content: payload.content,
                }
            })
        }
        "prependcontent" | "prepend" => {
            serde_json::from_value::<ContentPayload>(payload).map(|payload| {
                PatchOp::PrependContent {
                    content: payload.content,
                }
            })
        }
        "replacerange" => {
            serde_json::from_value::<RangePayload>(payload).map(|payload| PatchOp::ReplaceRange {
                start: payload.start,
                end: payload.end,
                content: payload.content,
            })
        }
        "rename" => {
            serde_json::from_value::<RenamePayload>(payload).map(|payload| PatchOp::Rename {
                title: payload.title,
            })
        }
        "setmetadata" => {
            serde_json::from_value::<MetadataPayload>(payload).map(|payload| PatchOp::SetMetadata {
                metadata: payload.metadata,
            })
        }
        _ => Err(unknown(kind)),
    }
}

fn normalize_patch_op_kind(kind: &str) -> String {
    kind.chars()
        .filter(|ch| *ch != '_' && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Options that describe and guard an edit operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct EditOptions {
    /// Version the caller believes is current, used for optimistic concurrency.
    pub expected_version: Option<VersionId>,
    /// Optional actor responsible for the edit.
    pub author: Option<String>,
    /// Optional human-readable description of the edit.
    pub change_summary: Option<String>,
}

/// Validation message produced while checking document integrity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationDiagnostic {
    /// Diagnostic severity.
    pub severity: DiagnosticSeverity,
    /// Section associated with the diagnostic, if applicable.
    pub section_id: Option<SectionId>,
    /// Human-readable validation message.
    pub message: String,
}

/// Severity level for validation diagnostics.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    /// Informational note.
    Info,
    /// Potential issue that may not block normal use.
    Warning,
    /// Issue that indicates invalid or unsafe document state.
    Error,
}

fn prefixed_base62_uuid(prefix: &str) -> String {
    format!("{prefix}-{}", base62_uuid(Uuid::now_v7()))
}

fn base62_uuid(uuid: Uuid) -> String {
    let mut value = uuid.as_u128();
    let mut encoded = [0u8; 22];

    for index in (0..encoded.len()).rev() {
        encoded[index] = BASE62[(value % 62) as usize];
        value /= 62;
    }

    String::from_utf8(encoded.to_vec()).expect("base62 alphabet is valid utf-8")
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;

    #[test]
    fn sections_deserialize_without_embedding_field() {
        let now = Utc::now();
        let value = json!({
            "section_id": "sec-1",
            "document_id": "doc-1",
            "parent_id": null,
            "children": [],
            "title": "Intro",
            "level": 1,
            "content": "Body",
            "ordinal": 0,
            "current_version": "ver-1",
            "metadata": {
                "anchor": null,
                "tags": [],
                "summary": null,
                "locked": false
            },
            "created_at": now,
            "updated_at": now
        });

        let section: Section = serde_json::from_value(value).unwrap();

        assert!(section.embedding.is_none());
    }

    #[test]
    fn generated_ids_use_prefixed_base62_uuidv7_shape() {
        let ids = [
            DocumentId::new_v7().as_str().to_owned(),
            SectionId::new_v7().as_str().to_owned(),
            VersionId::new_v7().as_str().to_owned(),
            SnapshotId::new_v7().as_str().to_owned(),
            RevisionId::new_v7().as_str().to_owned(),
        ];

        for id in ids {
            let (_, encoded) = id.split_once('-').unwrap();
            assert_eq!(encoded.len(), 22);
            assert!(encoded.bytes().all(|byte| byte.is_ascii_alphanumeric()));
            assert!(!encoded.contains('-'));
        }
    }
}
