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

//! Workspace-wide semantic search using HNSW vector indexing.
//!
//! This module provides efficient nearest-neighbor search over section embeddings.
//! Unlike the legacy implementation that rebuilt the index for every query, this
//! builds one persistent index per workspace generation and updates it incrementally.

#[cfg(feature = "semantic-search")]
use std::collections::BTreeMap;

#[cfg(feature = "semantic-search")]
use hnsw_vector_search::HnswGraph;

#[cfg(feature = "semantic-search")]
use crate::document::{DocumentId, Section, SectionId, TextEmbedding};
#[cfg(feature = "semantic-search")]
use crate::workspace::WorkspaceState;

/// Filters and result limits for semantic search.
#[cfg(feature = "semantic-search")]
#[derive(Clone, Debug)]
pub struct SemanticSearchOptions {
    pub document_id: Option<DocumentId>,
    pub path_prefix: Option<String>,
    pub require_same_model: bool,
    pub max_results: usize,
    /// HNSW construction parameter (default: 16)
    pub m: Option<usize>,
    /// HNSW construction parameter (default: 200)
    pub ef_construction: Option<usize>,
    /// HNSW search parameter (default: max_results * 2, min 50)
    pub ef: Option<usize>,
}

#[cfg(feature = "semantic-search")]
impl Default for SemanticSearchOptions {
    fn default() -> Self {
        Self {
            document_id: None,
            path_prefix: None,
            require_same_model: false,
            max_results: 10,
            m: Some(16),
            ef_construction: Some(200),
            ef: None,
        }
    }
}

/// One ranked section returned by semantic search.
#[cfg(feature = "semantic-search")]
#[derive(Clone, Debug)]
pub struct SemanticSearchResult {
    pub document_id: DocumentId,
    pub relative_path: String,
    pub section_id: SectionId,
    pub title: String,
    pub heading_ancestry: Vec<String>,
    /// Similarity score (higher is more similar, range 0.0-1.0)
    pub score: f32,
    /// Euclidean distance in embedding space
    pub distance: f32,
}

/// Persistent semantic index for one workspace generation.
#[cfg(feature = "semantic-search")]
#[derive(Clone)]
pub struct SemanticIndex {
    graph: HnswGraph,
    sections: Vec<IndexedSemanticSection>,
    sections_by_id: BTreeMap<SectionId, usize>,
}

#[cfg(feature = "semantic-search")]
impl Default for SemanticIndex {
    fn default() -> Self {
        Self {
            graph: HnswGraph::new(16, 200),
            sections: Vec::new(),
            sections_by_id: BTreeMap::new(),
        }
    }
}

#[cfg(feature = "semantic-search")]
#[derive(Clone, Debug)]
struct IndexedSemanticSection {
    document_id: DocumentId,
    relative_path: String,
    section_id: SectionId,
    title: String,
    heading_ancestry: Vec<String>,
    embedding: TextEmbedding,
    node_id: usize,
}

#[cfg(feature = "semantic-search")]
impl SemanticIndex {
    /// Builds a complete semantic index from sections that have embeddings.
    pub fn build(workspace: &WorkspaceState, options: &SemanticSearchOptions) -> Self {
        let m = options.m.unwrap_or(16);
        let ef_construction = options.ef_construction.unwrap_or(200);
        let mut graph = HnswGraph::new(m, ef_construction);
        let mut sections = Vec::new();
        let mut sections_by_id = BTreeMap::new();

        for document in workspace.documents() {
            let sections_by_section_id = document
                .sections
                .iter()
                .map(|section| (section.section_id.clone(), section))
                .collect::<BTreeMap<_, _>>();

            for section in &document.sections {
                let Some(embedding) = &section.embedding else {
                    continue;
                };

                if embedding.vector.is_empty() {
                    continue;
                }

                let node_id = graph.insert(embedding.vector.clone());
                let index = sections.len();
                sections_by_id.insert(section.section_id.clone(), index);
                sections.push(IndexedSemanticSection {
                    document_id: document.document.id.clone(),
                    relative_path: document.relative_path.clone(),
                    section_id: section.section_id.clone(),
                    title: section.title.clone(),
                    heading_ancestry: section_ancestry(section, &sections_by_section_id),
                    embedding: embedding.clone(),
                    node_id,
                });
            }
        }

        Self {
            graph,
            sections,
            sections_by_id,
        }
    }

    /// Returns the number of indexed sections.
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Searches for sections semantically similar to the query embedding.
    pub fn search(
        &self,
        query: &TextEmbedding,
        options: &SemanticSearchOptions,
    ) -> Vec<SemanticSearchResult> {
        if query.vector.is_empty() || self.graph.is_empty() {
            return Vec::new();
        }

        let max_results = options.max_results.max(1);
        let ef = options
            .ef
            .unwrap_or_else(|| max_results.saturating_mul(2).max(50));

        let results = self.graph.search(&query.vector, max_results, ef);
        let normalized_prefix = options.path_prefix.as_deref().map(normalize_path_prefix);

        results
            .into_iter()
            .filter_map(|result| {
                let node_id = result.0;
                let distance = result.1;

                self.sections.iter().find(|s| s.node_id == node_id).and_then(|section| {
                    // Apply filters
                    if let Some(doc_id) = &options.document_id {
                        if &section.document_id != doc_id {
                            return None;
                        }
                    }

                    if let Some(prefix) = normalized_prefix.as_deref() {
                        let matches = section.relative_path == prefix
                            || section
                                .relative_path
                                .strip_prefix(prefix)
                                .is_some_and(|suffix| suffix.starts_with('/'));
                        if !matches {
                            return None;
                        }
                    }

                    if options.require_same_model {
                        if section.embedding.model != query.model {
                            return None;
                        }
                    }

                    // Dimension mismatch check
                    if section.embedding.vector.len() != query.vector.len() {
                        return None;
                    }

                    // Convert distance to similarity score (higher is better)
                    let score = 1.0 / (1.0 + distance);

                    Some(SemanticSearchResult {
                        document_id: section.document_id.clone(),
                        relative_path: section.relative_path.clone(),
                        section_id: section.section_id.clone(),
                        title: section.title.clone(),
                        heading_ancestry: section.heading_ancestry.clone(),
                        score,
                        distance,
                    })
                })
            })
            .collect()
    }

    /// Removes all indexed sections from one document.
    pub fn remove_document(&mut self, document_id: &DocumentId) {
        let to_remove: Vec<usize> = self
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| &s.document_id == document_id)
            .map(|(idx, _)| idx)
            .collect();

        // Remove from sections_by_id map
        for idx in &to_remove {
            let section_id = &self.sections[*idx].section_id;
            self.sections_by_id.remove(section_id);
        }

        // Remove sections in reverse order to maintain indices
        for idx in to_remove.into_iter().rev() {
            self.sections.remove(idx);
        }

        // Rebuild the index after removal
        self.rebuild_graph();
    }

    /// Adds or updates sections from one document.
    pub fn add_document(
        &mut self,
        document: &crate::workspace::MaterializedDocument,
        sections_by_id: &BTreeMap<SectionId, &Section>,
    ) {
        for section in &document.sections {
            let Some(embedding) = &section.embedding else {
                continue;
            };

            if embedding.vector.is_empty() {
                continue;
            }

            let node_id = self.graph.insert(embedding.vector.clone());
            let index = self.sections.len();
            self.sections_by_id
                .insert(section.section_id.clone(), index);
            self.sections.push(IndexedSemanticSection {
                document_id: document.document.id.clone(),
                relative_path: document.relative_path.clone(),
                section_id: section.section_id.clone(),
                title: section.title.clone(),
                heading_ancestry: section_ancestry(section, sections_by_id),
                embedding: embedding.clone(),
                node_id,
            });
        }
    }

    fn rebuild_graph(&mut self) {
        // Reconstruct the HNSW graph from remaining sections
        let m = 16; // Use default parameters for rebuild
        let ef_construction = 200;
        let mut new_graph = HnswGraph::new(m, ef_construction);
        let mut new_sections_by_id = BTreeMap::new();

        for (index, section) in self.sections.iter_mut().enumerate() {
            section.node_id = new_graph.insert(section.embedding.vector.clone());
            new_sections_by_id.insert(section.section_id.clone(), index);
        }

        self.graph = new_graph;
        self.sections_by_id = new_sections_by_id;
    }
}

#[cfg(feature = "semantic-search")]
fn section_ancestry(
    section: &Section,
    sections_by_id: &BTreeMap<SectionId, &Section>,
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

#[cfg(feature = "semantic-search")]
fn normalize_path_prefix(prefix: &str) -> String {
    prefix.replace('\\', "/").trim_end_matches('/').to_owned()
}

#[cfg(all(test, feature = "semantic-search"))]
mod tests {
    use super::*;
    use crate::workspace::WorkspaceState;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!("vds-semantic-{nonce}"));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, relative_path: &str, contents: &str) {
            let path = self.root.join(relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn builds_empty_index_when_no_embeddings_present() {
        let workspace = TestWorkspace::new();
        workspace.write("doc.md", "# Document\n\nNo embeddings here.\n");
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = SemanticIndex::build(&state, &SemanticSearchOptions::default());
        assert_eq!(index.section_count(), 0);
    }

    #[test]
    fn searches_empty_index_returns_empty() {
        let workspace = TestWorkspace::new();
        workspace.write("doc.md", "# Document\n\nNo embeddings.\n");
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = SemanticIndex::build(&state, &SemanticSearchOptions::default());

        let query = TextEmbedding {
            model: Some("test".to_owned()),
            vector: vec![0.1, 0.2, 0.3],
        };

        let results = index.search(&query, &SemanticSearchOptions::default());
        assert!(results.is_empty());
    }
}
