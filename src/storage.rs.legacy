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

use crate::document::{
    Document, DocumentId, DocumentSnapshot, Section, SectionId, SectionVersion, SnapshotId,
    VersionId,
};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::io;
use std::path::Path;

const DOCUMENTS: TableDefinition<String, String> = TableDefinition::new("documents");
const SECTIONS: TableDefinition<String, String> = TableDefinition::new("sections");
const SECTION_VERSIONS: TableDefinition<String, String> = TableDefinition::new("section_versions");
const SNAPSHOTS: TableDefinition<String, String> = TableDefinition::new("snapshots");

const DOCUMENT_NAME_INDEX: TableDefinition<String, String> =
    TableDefinition::new("idx_document_name");
const DOCUMENT_SECTION_INDEX: TableDefinition<String, String> =
    TableDefinition::new("idx_document_section");
const PARENT_CHILD_INDEX: TableDefinition<String, String> =
    TableDefinition::new("idx_parent_child");
const SECTION_VERSION_INDEX: TableDefinition<String, String> =
    TableDefinition::new("idx_section_version");
const DOCUMENT_SNAPSHOT_INDEX: TableDefinition<String, String> =
    TableDefinition::new("idx_document_snapshot");

/// Storage facade for VDS document data.
///
/// Records are stored as complete JSON payloads for straightforward updates.
/// Query-oriented redb tables maintain the small secondary indexes needed for
/// document listing, tree traversal, section history, and snapshot history.
pub struct DocumentStore {
    db: Database,
}

impl DocumentStore {
    /// Opens or creates a redb-backed document store and initializes tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = Self {
            db: Database::create(path)?,
        };
        store.initialize()?;
        Ok(store)
    }

    /// Ensures every table and index exists.
    pub fn initialize(&self) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            tx.open_table(DOCUMENTS)?;
            tx.open_table(SECTIONS)?;
            tx.open_table(SECTION_VERSIONS)?;
            tx.open_table(SNAPSHOTS)?;
            tx.open_table(DOCUMENT_NAME_INDEX)?;
            tx.open_table(DOCUMENT_SECTION_INDEX)?;
            tx.open_table(PARENT_CHILD_INDEX)?;
            tx.open_table(SECTION_VERSION_INDEX)?;
            tx.open_table(DOCUMENT_SNAPSHOT_INDEX)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Stores a document and all supplied child records in one transaction.
    ///
    /// This is the import/bootstrap path: callers can persist the document
    /// record, section tree, section versions, and snapshots atomically.
    pub fn store_document_state(
        &self,
        document: &Document,
        sections: &[Section],
        versions: &[SectionVersion],
        snapshots: &[DocumentSnapshot],
    ) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            self.put_document_in(&tx, document)?;
            for section in sections {
                self.put_section_in(&tx, section)?;
            }
            for version in versions {
                self.put_section_version_in(&tx, version)?;
            }
            for snapshot in snapshots {
                self.put_snapshot_in(&tx, snapshot)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Inserts or replaces a document record.
    pub fn put_document(&self, document: &Document) -> Result<()> {
        let tx = self.db.begin_write()?;
        self.put_document_in(&tx, document)?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes a document and all associated section, version, and snapshot records.
    pub fn delete_document(
        &self,
        document_id: &DocumentId,
    ) -> Result<Option<(usize, usize, usize)>> {
        let tx = self.db.begin_write()?;
        let counts = {
            let document = {
                let mut documents = tx.open_table(DOCUMENTS)?;
                documents
                    .remove(document_id.as_str().to_owned())?
                    .map(|value| decode_json::<Document>(&value.value()))
                    .transpose()?
            };
            let Some(document) = document else {
                tx.commit()?;
                return Ok(None);
            };

            let mut names = tx.open_table(DOCUMENT_NAME_INDEX)?;
            names.remove(document.name)?;

            let section_ids = collect_index_values(
                &tx,
                DOCUMENT_SECTION_INDEX,
                &document_section_prefix(document_id),
            )?;
            let snapshot_ids = collect_index_values(
                &tx,
                DOCUMENT_SNAPSHOT_INDEX,
                &document_snapshot_prefix(document_id),
            )?;

            let mut version_ids = Vec::new();
            for section_id in &section_ids {
                version_ids.extend(collect_index_values(
                    &tx,
                    SECTION_VERSION_INDEX,
                    &section_version_prefix(&SectionId::new(section_id.clone())),
                )?);
            }

            let mut sections = tx.open_table(SECTIONS)?;
            for section_id in &section_ids {
                sections.remove(section_id.clone())?;
            }
            drop(sections);

            let mut versions = tx.open_table(SECTION_VERSIONS)?;
            for version_id in &version_ids {
                versions.remove(version_id.clone())?;
            }
            drop(versions);

            let mut snapshots = tx.open_table(SNAPSHOTS)?;
            for snapshot_id in &snapshot_ids {
                snapshots.remove(snapshot_id.clone())?;
            }
            drop(snapshots);

            remove_prefix_entries(
                &tx,
                DOCUMENT_SECTION_INDEX,
                &document_section_prefix(document_id),
            )?;
            remove_prefix_entries(
                &tx,
                PARENT_CHILD_INDEX,
                &format!("{}\0", document_id.as_str()),
            )?;
            for section_id in &section_ids {
                remove_prefix_entries(
                    &tx,
                    SECTION_VERSION_INDEX,
                    &section_version_prefix(&SectionId::new(section_id.clone())),
                )?;
            }
            remove_prefix_entries(
                &tx,
                DOCUMENT_SNAPSHOT_INDEX,
                &document_snapshot_prefix(document_id),
            )?;

            (section_ids.len(), version_ids.len(), snapshot_ids.len())
        };
        tx.commit()?;
        Ok(Some(counts))
    }

    /// Reads a document by ID.
    pub fn get_document(&self, document_id: &DocumentId) -> Result<Option<Document>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(DOCUMENTS)?;
        get_json(&table, document_id.as_str())
    }

    /// Resolves a document by its human-readable name.
    pub fn get_document_by_name(&self, name: &str) -> Result<Option<Document>> {
        let tx = self.db.begin_read()?;
        let names = tx.open_table(DOCUMENT_NAME_INDEX)?;
        let Some(document_id) = names.get(name.to_owned())?.map(|value| value.value()) else {
            return Ok(None);
        };

        let documents = tx.open_table(DOCUMENTS)?;
        get_json(&documents, document_id.as_str())
    }

    /// Lists all stored documents ordered by document ID.
    pub fn list_documents(&self) -> Result<Vec<Document>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(DOCUMENTS)?;
        collect_json_values(table.range::<String>(..)?, None)
    }

    /// Inserts or replaces a section and refreshes its query indexes.
    pub fn put_section(&self, section: &Section) -> Result<()> {
        let tx = self.db.begin_write()?;
        self.put_section_in(&tx, section)?;
        tx.commit()?;
        Ok(())
    }

    /// Reads a section by ID.
    pub fn get_section(&self, section_id: &SectionId) -> Result<Option<Section>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SECTIONS)?;
        get_json(&table, section_id.as_str())
    }

    /// Lists every section belonging to a document.
    pub fn list_document_sections(&self, document_id: &DocumentId) -> Result<Vec<Section>> {
        let tx = self.db.begin_read()?;
        let index = tx.open_table(DOCUMENT_SECTION_INDEX)?;
        let sections = tx.open_table(SECTIONS)?;
        let prefix = document_section_prefix(document_id);

        let mut result = Vec::new();
        for entry in prefix_entries(index.range(prefix_range(&prefix))?) {
            let section_id = entry?;
            if let Some(section) = get_json(&sections, section_id.as_str())? {
                result.push(section);
            }
        }
        Ok(result)
    }

    /// Lists direct child sections in ordinal order.
    pub fn list_child_sections(
        &self,
        document_id: &DocumentId,
        parent_id: Option<&SectionId>,
    ) -> Result<Vec<Section>> {
        let tx = self.db.begin_read()?;
        let index = tx.open_table(PARENT_CHILD_INDEX)?;
        let sections = tx.open_table(SECTIONS)?;
        let prefix = parent_child_prefix(document_id, parent_id);

        let mut result = Vec::new();
        for entry in prefix_entries(index.range(prefix_range(&prefix))?) {
            let section_id = entry?;
            if let Some(section) = get_json(&sections, section_id.as_str())? {
                result.push(section);
            }
        }
        Ok(result)
    }

    /// Deletes a section record and removes its query indexes.
    pub fn delete_section(&self, section_id: &SectionId) -> Result<Option<Section>> {
        let tx = self.db.begin_write()?;
        let removed = {
            let mut sections = tx.open_table(SECTIONS)?;
            let removed = sections
                .remove(section_id.as_str().to_owned())?
                .map(|value| decode_json::<Section>(&value.value()))
                .transpose()?;

            if let Some(section) = &removed {
                remove_section_indexes(&tx, section)?;
            }

            removed
        };
        tx.commit()?;
        Ok(removed)
    }

    /// Deletes a section without removing its immutable versions.
    pub fn delete_sections(&self, section_ids: &[SectionId]) -> Result<Vec<Section>> {
        let tx = self.db.begin_write()?;
        let mut removed = Vec::new();
        {
            let mut sections = tx.open_table(SECTIONS)?;
            for section_id in section_ids {
                let removed_section = sections
                    .remove(section_id.as_str().to_owned())?
                    .map(|value| decode_json::<Section>(&value.value()))
                    .transpose()?;
                if let Some(section) = removed_section {
                    remove_section_indexes(&tx, &section)?;
                    removed.push(section);
                }
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Inserts or replaces an immutable section version.
    pub fn put_section_version(&self, version: &SectionVersion) -> Result<()> {
        let tx = self.db.begin_write()?;
        self.put_section_version_in(&tx, version)?;
        tx.commit()?;
        Ok(())
    }

    /// Reads a section version by version ID.
    pub fn get_section_version(&self, version_id: &VersionId) -> Result<Option<SectionVersion>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SECTION_VERSIONS)?;
        get_json(&table, version_id.as_str())
    }

    /// Lists versions for one section ordered by creation time and version ID.
    pub fn list_section_versions(&self, section_id: &SectionId) -> Result<Vec<SectionVersion>> {
        let tx = self.db.begin_read()?;
        let index = tx.open_table(SECTION_VERSION_INDEX)?;
        let versions = tx.open_table(SECTION_VERSIONS)?;
        let prefix = section_version_prefix(section_id);

        let mut result = Vec::new();
        for entry in prefix_entries(index.range(prefix_range(&prefix))?) {
            let version_id = entry?;
            if let Some(version) = get_json(&versions, version_id.as_str())? {
                result.push(version);
            }
        }
        Ok(result)
    }

    /// Inserts or replaces a document-level snapshot.
    pub fn put_snapshot(&self, snapshot: &DocumentSnapshot) -> Result<()> {
        let tx = self.db.begin_write()?;
        self.put_snapshot_in(&tx, snapshot)?;
        tx.commit()?;
        Ok(())
    }

    /// Reads a document snapshot by snapshot ID.
    pub fn get_snapshot(&self, snapshot_id: &SnapshotId) -> Result<Option<DocumentSnapshot>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SNAPSHOTS)?;
        get_json(&table, snapshot_id.as_str())
    }

    /// Lists snapshots for one document ordered by creation time and snapshot ID.
    pub fn list_document_snapshots(
        &self,
        document_id: &DocumentId,
    ) -> Result<Vec<DocumentSnapshot>> {
        let tx = self.db.begin_read()?;
        let index = tx.open_table(DOCUMENT_SNAPSHOT_INDEX)?;
        let snapshots = tx.open_table(SNAPSHOTS)?;
        let prefix = document_snapshot_prefix(document_id);

        let mut result = Vec::new();
        for entry in prefix_entries(index.range(prefix_range(&prefix))?) {
            let snapshot_id = entry?;
            if let Some(snapshot) = get_json(&snapshots, snapshot_id.as_str())? {
                result.push(snapshot);
            }
        }
        Ok(result)
    }

    fn put_document_in(&self, tx: &redb::WriteTransaction, document: &Document) -> Result<()> {
        let mut documents = tx.open_table(DOCUMENTS)?;
        if let Some(old) = documents
            .get(document.id.as_str().to_owned())?
            .map(|value| decode_json::<Document>(&value.value()))
            .transpose()?
        {
            let mut names = tx.open_table(DOCUMENT_NAME_INDEX)?;
            names.remove(old.name)?;
        }

        documents.insert(document.id.as_str().to_owned(), encode_json(document)?)?;

        let mut names = tx.open_table(DOCUMENT_NAME_INDEX)?;
        names.insert(document.name.clone(), document.id.as_str().to_owned())?;
        Ok(())
    }

    fn put_section_in(&self, tx: &redb::WriteTransaction, section: &Section) -> Result<()> {
        let mut sections = tx.open_table(SECTIONS)?;
        if let Some(old) = sections
            .get(section.section_id.as_str().to_owned())?
            .map(|value| decode_json::<Section>(&value.value()))
            .transpose()?
        {
            remove_section_indexes(tx, &old)?;
        }

        sections.insert(
            section.section_id.as_str().to_owned(),
            encode_json(section)?,
        )?;

        let mut document_sections = tx.open_table(DOCUMENT_SECTION_INDEX)?;
        document_sections.insert(
            document_section_key(&section.document_id, &section.section_id),
            section.section_id.as_str().to_owned(),
        )?;

        let mut parent_children = tx.open_table(PARENT_CHILD_INDEX)?;
        parent_children.insert(
            parent_child_key(
                &section.document_id,
                section.parent_id.as_ref(),
                section.ordinal,
                &section.section_id,
            ),
            section.section_id.as_str().to_owned(),
        )?;

        Ok(())
    }

    fn put_section_version_in(
        &self,
        tx: &redb::WriteTransaction,
        version: &SectionVersion,
    ) -> Result<()> {
        let mut versions = tx.open_table(SECTION_VERSIONS)?;
        versions.insert(
            version.version_id.as_str().to_owned(),
            encode_json(version)?,
        )?;

        let mut index = tx.open_table(SECTION_VERSION_INDEX)?;
        index.insert(
            section_version_key(version),
            version.version_id.as_str().to_owned(),
        )?;

        Ok(())
    }

    fn put_snapshot_in(
        &self,
        tx: &redb::WriteTransaction,
        snapshot: &DocumentSnapshot,
    ) -> Result<()> {
        let mut snapshots = tx.open_table(SNAPSHOTS)?;
        snapshots.insert(
            snapshot.snapshot_id.as_str().to_owned(),
            encode_json(snapshot)?,
        )?;

        let mut index = tx.open_table(DOCUMENT_SNAPSHOT_INDEX)?;
        index.insert(
            document_snapshot_key(snapshot),
            snapshot.snapshot_id.as_str().to_owned(),
        )?;

        Ok(())
    }
}

fn remove_section_indexes(tx: &redb::WriteTransaction, section: &Section) -> Result<()> {
    let mut document_sections = tx.open_table(DOCUMENT_SECTION_INDEX)?;
    document_sections.remove(document_section_key(
        &section.document_id,
        &section.section_id,
    ))?;

    let mut parent_children = tx.open_table(PARENT_CHILD_INDEX)?;
    parent_children.remove(parent_child_key(
        &section.document_id,
        section.parent_id.as_ref(),
        section.ordinal,
        &section.section_id,
    ))?;

    Ok(())
}

fn get_json<T>(table: &impl ReadableTable<String, String>, key: &str) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    table
        .get(key.to_owned())?
        .map(|value| decode_json::<T>(&value.value()))
        .transpose()
}

fn collect_json_values<T>(
    range: redb::Range<'_, String, String>,
    max_results: Option<usize>,
) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let mut result = Vec::new();
    for entry in range {
        let (_, value) = entry?;
        result.push(decode_json(&value.value())?);
        if max_results.is_some_and(|max| result.len() >= max) {
            break;
        }
    }
    Ok(result)
}

fn prefix_entries(range: redb::Range<'_, String, String>) -> impl Iterator<Item = Result<String>> {
    range.map(|entry| {
        let (_, value) = entry?;
        Ok(value.value())
    })
}

fn collect_index_values(
    tx: &redb::WriteTransaction,
    definition: TableDefinition<String, String>,
    prefix: &str,
) -> Result<Vec<String>> {
    let index = tx.open_table(definition)?;
    prefix_entries(index.range(prefix_range(prefix))?).collect()
}

fn remove_prefix_entries(
    tx: &redb::WriteTransaction,
    definition: TableDefinition<String, String>,
    prefix: &str,
) -> Result<()> {
    let keys = {
        let table = tx.open_table(definition)?;
        table
            .range(prefix_range(prefix))?
            .map(|entry| {
                let (key, _) = entry?;
                Ok(key.value())
            })
            .collect::<Result<Vec<_>>>()?
    };
    let mut table = tx.open_table(definition)?;
    for key in keys {
        table.remove(key)?;
    }
    Ok(())
}

fn encode_json<T>(value: &T) -> Result<String>
where
    T: Serialize,
{
    Ok(serde_json::to_string(value)?)
}

fn decode_json<T>(value: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    Ok(serde_json::from_str(value)?)
}

fn prefix_range(prefix: &str) -> std::ops::Range<String> {
    prefix.to_owned()..format!("{prefix}\u{10ffff}")
}

fn document_section_prefix(document_id: &DocumentId) -> String {
    format!("{}\0", document_id.as_str())
}

fn document_section_key(document_id: &DocumentId, section_id: &SectionId) -> String {
    format!(
        "{}{section_id}",
        document_section_prefix(document_id),
        section_id = section_id.as_str()
    )
}

fn parent_child_prefix(document_id: &DocumentId, parent_id: Option<&SectionId>) -> String {
    format!(
        "{}\0{}\0",
        document_id.as_str(),
        parent_id.map(SectionId::as_str).unwrap_or("")
    )
}

fn parent_child_key(
    document_id: &DocumentId,
    parent_id: Option<&SectionId>,
    ordinal: u32,
    section_id: &SectionId,
) -> String {
    format!(
        "{}{ordinal:010}\0{}",
        parent_child_prefix(document_id, parent_id),
        section_id.as_str()
    )
}

fn section_version_prefix(section_id: &SectionId) -> String {
    format!("{}\0", section_id.as_str())
}

fn section_version_key(version: &SectionVersion) -> String {
    format!(
        "{}{}\0{}",
        section_version_prefix(&version.section_id),
        version.created_at.timestamp_micros(),
        version.version_id.as_str()
    )
}

fn document_snapshot_prefix(document_id: &DocumentId) -> String {
    format!("{}\0", document_id.as_str())
}

fn document_snapshot_key(snapshot: &DocumentSnapshot) -> String {
    format!(
        "{}{}\0{}",
        document_snapshot_prefix(&snapshot.document_id),
        snapshot.created_at.timestamp_micros(),
        snapshot.snapshot_id.as_str()
    )
}

/// Storage-layer result type.
pub type Result<T> = std::result::Result<T, StorageError>;

/// Error type used by the redb storage facade.
#[derive(Debug)]
pub enum StorageError {
    /// Database open/create error.
    Database(redb::DatabaseError),
    /// Transaction lifecycle error.
    Transaction(redb::TransactionError),
    /// Table open or schema error.
    Table(redb::TableError),
    /// Low-level storage error.
    Storage(redb::StorageError),
    /// Commit error.
    Commit(redb::CommitError),
    /// JSON encoding or decoding error.
    Json(serde_json::Error),
    /// Filesystem IO error.
    Io(io::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(f, "database error: {error}"),
            Self::Transaction(error) => write!(f, "transaction error: {error}"),
            Self::Table(error) => write!(f, "table error: {error}"),
            Self::Storage(error) => write!(f, "storage error: {error}"),
            Self::Commit(error) => write!(f, "commit error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<redb::DatabaseError> for StorageError {
    fn from(value: redb::DatabaseError) -> Self {
        Self::Database(value)
    }
}

impl From<redb::TransactionError> for StorageError {
    fn from(value: redb::TransactionError) -> Self {
        Self::Transaction(value)
    }
}

impl From<redb::TableError> for StorageError {
    fn from(value: redb::TableError) -> Self {
        Self::Table(value)
    }
}

impl From<redb::StorageError> for StorageError {
    fn from(value: redb::StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<redb::CommitError> for StorageError {
    fn from(value: redb::CommitError) -> Self {
        Self::Commit(value)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<io::Error> for StorageError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::document::{DocumentFormat, DocumentMetadata, SectionMetadata};

    #[test]
    fn stores_and_queries_document_state() {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test-dbs")
            .join(format!("{}.redb", Uuid::now_v7()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let store = DocumentStore::open(&path).unwrap();
        let now = Utc::now();
        let document_id = DocumentId::new("doc-1");
        let root_id = SectionId::new("section-root");
        let child_id = SectionId::new("section-child");
        let version_id = VersionId::new("version-1");
        let snapshot_id = SnapshotId::new("snapshot-1");

        let document = Document {
            id: document_id.clone(),
            name: "guide".to_owned(),
            root: root_id.clone(),
            current_version: version_id.clone(),
            metadata: DocumentMetadata {
                title: Some("Guide".to_owned()),
                description: None,
                tags: vec!["docs".to_owned()],
                source_path: None,
                format: DocumentFormat::Markdown,
            },
            embedding: None,
            created_at: now,
            updated_at: now,
        };

        let metadata = SectionMetadata {
            anchor: None,
            tags: Vec::new(),
            summary: None,
            locked: false,
        };
        let root = Section {
            section_id: root_id.clone(),
            document_id: document_id.clone(),
            parent_id: None,
            children: vec![child_id.clone()],
            title: "Root".to_owned(),
            level: 1,
            content: String::new(),
            ordinal: 0,
            current_version: version_id.clone(),
            metadata: metadata.clone(),
            embedding: None,
            created_at: now,
            updated_at: now,
        };
        let child = Section {
            section_id: child_id.clone(),
            document_id: document_id.clone(),
            parent_id: Some(root_id.clone()),
            children: Vec::new(),
            title: "Child".to_owned(),
            level: 2,
            content: "Body".to_owned(),
            ordinal: 1,
            current_version: version_id.clone(),
            metadata: metadata.clone(),
            embedding: None,
            created_at: now,
            updated_at: now,
        };
        let version = SectionVersion {
            version_id: version_id.clone(),
            section_id: child_id.clone(),
            title: child.title.clone(),
            content: child.content.clone(),
            metadata,
            embedding: None,
            created_at: now,
            author: Some("test".to_owned()),
            change_summary: Some("initial".to_owned()),
        };
        let snapshot = DocumentSnapshot {
            snapshot_id: snapshot_id.clone(),
            document_id: document_id.clone(),
            root_version: version_id,
            sections: vec![root.clone(), child.clone()],
            label: Some("first".to_owned()),
            created_at: now,
            author: Some("test".to_owned()),
            change_summary: Some("snapshot".to_owned()),
        };

        store
            .store_document_state(
                &document,
                &[root.clone(), child.clone()],
                &[version],
                &[snapshot],
            )
            .unwrap();

        assert_eq!(
            store.get_document_by_name("guide").unwrap().unwrap().id,
            document_id
        );
        assert_eq!(store.list_documents().unwrap().len(), 1);
        assert_eq!(store.list_document_sections(&document_id).unwrap().len(), 2);
        assert_eq!(
            store
                .list_child_sections(&document_id, Some(&root_id))
                .unwrap()[0]
                .section_id,
            child_id
        );
        assert_eq!(store.list_section_versions(&child_id).unwrap().len(), 1);
        assert_eq!(
            store.list_document_snapshots(&document_id).unwrap().len(),
            1
        );
    }
}
