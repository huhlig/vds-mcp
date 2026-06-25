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

//! Workspace-wide in-memory full-text search for materialized Markdown.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use crate::document::{DocumentId, Section, SectionId};
use crate::workspace::WorkspaceState;

/// Filters and result limits for one lexical search.
#[derive(Clone, Debug)]
pub struct FullTextSearchOptions {
    pub document_id: Option<DocumentId>,
    pub path_prefix: Option<String>,
    pub require_all_terms: bool,
    pub max_results: usize,
}

impl Default for FullTextSearchOptions {
    fn default() -> Self {
        Self {
            document_id: None,
            path_prefix: None,
            require_all_terms: true,
            max_results: 50,
        }
    }
}

/// One byte-addressed match in original section content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullTextMatch {
    pub start: usize,
    pub end: usize,
    pub snippet: String,
}

/// One ranked section returned by workspace-wide lexical search.
#[derive(Clone, Debug)]
pub struct FullTextSearchResult {
    pub document_id: DocumentId,
    pub relative_path: String,
    pub section_id: SectionId,
    pub title: String,
    pub heading_ancestry: Vec<String>,
    pub score: f32,
    pub title_match: bool,
    pub content_matches: Vec<FullTextMatch>,
}

/// Disposable inverted index for one coherent workspace generation.
#[derive(Clone, Debug, Default)]
pub struct FullTextIndex {
    sections: BTreeMap<SectionKey, IndexedSection>,
    postings: BTreeMap<String, Vec<Posting>>,
    average_title_length: f32,
    average_content_length: f32,
}

impl FullTextIndex {
    /// Builds a complete lexical index from current materialized sections.
    pub fn build(workspace: &WorkspaceState) -> Self {
        let mut index = Self::default();
        let mut total_title_length = 0usize;
        let mut total_content_length = 0usize;

        for document in workspace.documents() {
            let sections_by_id = document
                .sections
                .iter()
                .map(|section| (section.section_id.clone(), section))
                .collect::<BTreeMap<_, _>>();
            for section in &document.sections {
                let key = SectionKey {
                    document_id: document.document.id.clone(),
                    section_id: section.section_id.clone(),
                };
                let title_tokens = tokenize(&section.title);
                let content_tokens = tokenize(&section.content);
                let title_length = title_tokens.len().max(1);
                let content_length = content_tokens.len().max(1);
                total_title_length += title_length;
                total_content_length += content_length;

                let mut builders = BTreeMap::<String, PostingBuilder>::new();
                for token in title_tokens {
                    builders
                        .entry(token.term)
                        .or_default()
                        .title_offsets
                        .push((token.start, token.end));
                }
                for token in content_tokens {
                    builders
                        .entry(token.term)
                        .or_default()
                        .content_offsets
                        .push((token.start, token.end));
                }
                for (term, builder) in builders {
                    index.postings.entry(term).or_default().push(Posting {
                        key: key.clone(),
                        title_offsets: builder.title_offsets,
                        content_offsets: builder.content_offsets,
                    });
                }

                index.sections.insert(
                    key,
                    IndexedSection {
                        relative_path: document.relative_path.clone(),
                        title: section.title.clone(),
                        content: section.content.clone(),
                        heading_ancestry: section_ancestry(section, &sections_by_id),
                        title_length,
                        content_length,
                    },
                );
            }
        }

        let section_count = index.sections.len().max(1) as f32;
        index.average_title_length = total_title_length as f32 / section_count;
        index.average_content_length = total_content_length as f32 / section_count;
        index
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Searches the index using BM25-style field scoring.
    ///
    /// Supports exact terms, `prefix*` atoms, and `"quoted phrases"`.
    /// Phrases are matched by verifying they appear as a contiguous lowercased
    /// substring in the section title or content.
    pub fn search(
        &self,
        query: &str,
        options: &FullTextSearchOptions,
    ) -> Vec<FullTextSearchResult> {
        let atoms = query_atoms(query);
        if atoms.is_empty() || options.max_results == 0 {
            return Vec::new();
        }

        let normalized_prefix = options.path_prefix.as_deref().map(normalize_path_prefix);
        let mut accumulators = BTreeMap::<SectionKey, SearchAccumulator>::new();

        for atom in &atoms {
            match atom {
                QueryAtom::Term { .. } => {
                    self.score_term_atom(
                        atom,
                        options,
                        normalized_prefix.as_deref(),
                        &mut accumulators,
                    );
                }
                QueryAtom::Phrase { terms, raw } => {
                    let phrase_matches = matched_phrase_keys(
                        self,
                        terms,
                        raw,
                        options,
                        normalized_prefix.as_deref(),
                    );
                    if phrase_matches.is_empty() {
                        if options.require_all_terms {
                            return Vec::new();
                        }
                    } else {
                        for (key, acc) in phrase_matches {
                            let a = accumulators.entry(key).or_default();
                            a.score += acc.score;
                            a.matched_atoms += 1;
                            a.title_match |= acc.title_match;
                            a.content_offsets.extend(acc.content_offsets);
                        }
                    }
                }
            }
        }

        let required_matches = if options.require_all_terms {
            atoms.len()
        } else {
            1
        };
        let mut results = accumulators
            .into_iter()
            .filter(|(_, acc)| acc.matched_atoms >= required_matches)
            .filter_map(|(key, mut acc)| {
                let section = self.sections.get(&key)?;
                acc.content_offsets.sort_unstable();
                acc.content_offsets.dedup();
                let content_matches = acc
                    .content_offsets
                    .into_iter()
                    .take(3)
                    .map(|(start, end)| FullTextMatch {
                        start,
                        end,
                        snippet: snippet(&section.content, start, end),
                    })
                    .collect();
                Some(FullTextSearchResult {
                    document_id: key.document_id,
                    relative_path: section.relative_path.clone(),
                    section_id: key.section_id,
                    title: section.title.clone(),
                    heading_ancestry: section.heading_ancestry.clone(),
                    score: acc.score,
                    title_match: acc.title_match,
                    content_matches,
                })
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
                .then_with(|| left.section_id.cmp(&right.section_id))
        });
        results.truncate(options.max_results);
        results
    }

    fn score_term_atom(
        &self,
        atom: &QueryAtom,
        options: &FullTextSearchOptions,
        path_prefix: Option<&str>,
        accumulators: &mut BTreeMap<SectionKey, SearchAccumulator>,
    ) {
        let matching_terms = self.matching_terms(atom);
        let mut atom_matches = BTreeMap::<SectionKey, AtomAccumulator>::new();
        let atom_df = matching_terms
            .iter()
            .flat_map(|t| self.postings.get(*t).into_iter().flatten())
            .map(|p| p.key.clone())
            .collect::<BTreeSet<_>>()
            .len();
        if atom_df == 0 {
            if let QueryAtom::Term { .. } = atom {
                // caller handles require_all_terms
            }
            return;
        }
        let inv_df = idf(self.sections.len(), atom_df);

        for term in matching_terms {
            let Some(postings) = self.postings.get(term) else {
                continue;
            };
            for posting in postings {
                let Some(section) = self.sections.get(&posting.key) else {
                    continue;
                };
                if !matches_filters(&posting.key, section, options, path_prefix) {
                    continue;
                }
                let title_score = bm25_term_score(
                    posting.title_offsets.len(),
                    section.title_length,
                    self.average_title_length,
                ) * 2.5;
                let content_score = bm25_term_score(
                    posting.content_offsets.len(),
                    section.content_length,
                    self.average_content_length,
                );
                let m = atom_matches.entry(posting.key.clone()).or_default();
                m.score += inv_df * (title_score + content_score);
                m.title_match |= !posting.title_offsets.is_empty();
                m.content_offsets
                    .extend(posting.content_offsets.iter().copied());
            }
        }

        for (key, am) in atom_matches {
            let a = accumulators.entry(key).or_default();
            a.score += am.score;
            a.matched_atoms += 1;
            a.title_match |= am.title_match;
            a.content_offsets.extend(am.content_offsets);
        }
    }

    fn matching_terms<'a>(&'a self, atom: &QueryAtom) -> Vec<&'a String> {
        match atom {
            QueryAtom::Term {
                term,
                prefix: false,
            } => self
                .postings
                .get_key_value(term.as_str())
                .map(|(t, _)| vec![t])
                .unwrap_or_default(),
            QueryAtom::Term { term, prefix: true } => self
                .postings
                .range::<str, _>((Bound::Included(term.as_str()), Bound::Unbounded))
                .map(|(t, _)| t)
                .take_while(|t| t.starts_with(term.as_str()))
                .collect(),
            QueryAtom::Phrase { .. } => Vec::new(), // handled separately
        }
    }

    /// Removes all postings for one document from the index in-place.
    ///
    /// Used for incremental updates: call `remove_document` then `add_document`
    /// to refresh only the changed document without rebuilding the whole index.
    pub fn remove_document(&mut self, document_id: &DocumentId) {
        let to_remove: Vec<SectionKey> = self
            .sections
            .keys()
            .filter(|k| &k.document_id == document_id)
            .cloned()
            .collect();

        for key in &to_remove {
            self.sections.remove(key);
        }
        for postings in self.postings.values_mut() {
            postings.retain(|p| !to_remove.contains(&p.key));
        }
        self.postings.retain(|_, postings| !postings.is_empty());

        // Recompute averages from remaining sections.
        let count = self.sections.len().max(1) as f32;
        let total_title: usize = self.sections.values().map(|s| s.title_length).sum();
        let total_content: usize = self.sections.values().map(|s| s.content_length).sum();
        self.average_title_length = total_title as f32 / count;
        self.average_content_length = total_content as f32 / count;
    }

    /// Indexes all sections of one document and adds them to the existing index.
    ///
    /// Call after `remove_document` to replace stale postings without a full rebuild.
    pub fn add_document(
        &mut self,
        document: &crate::workspace::MaterializedDocument,
        sections_by_id: &BTreeMap<SectionId, &Section>,
    ) {
        let mut total_title_delta = 0usize;
        let mut total_content_delta = 0usize;

        for section in &document.sections {
            let key = SectionKey {
                document_id: document.document.id.clone(),
                section_id: section.section_id.clone(),
            };
            let title_tokens = tokenize(&section.title);
            let content_tokens = tokenize(&section.content);
            let title_length = title_tokens.len().max(1);
            let content_length = content_tokens.len().max(1);
            total_title_delta += title_length;
            total_content_delta += content_length;

            let mut builders = BTreeMap::<String, PostingBuilder>::new();
            for token in title_tokens {
                builders
                    .entry(token.term)
                    .or_default()
                    .title_offsets
                    .push((token.start, token.end));
            }
            for token in content_tokens {
                builders
                    .entry(token.term)
                    .or_default()
                    .content_offsets
                    .push((token.start, token.end));
            }
            for (term, builder) in builders {
                self.postings.entry(term).or_default().push(Posting {
                    key: key.clone(),
                    title_offsets: builder.title_offsets,
                    content_offsets: builder.content_offsets,
                });
            }

            self.sections.insert(
                key,
                IndexedSection {
                    relative_path: document.relative_path.clone(),
                    title: section.title.clone(),
                    content: section.content.clone(),
                    heading_ancestry: section_ancestry(section, sections_by_id),
                    title_length,
                    content_length,
                },
            );
        }

        // Update running averages.
        let count = self.sections.len().max(1) as f32;
        let total_title: usize = self.sections.values().map(|s| s.title_length).sum();
        let total_content: usize = self.sections.values().map(|s| s.content_length).sum();
        self.average_title_length = total_title as f32 / count;
        self.average_content_length = total_content as f32 / count;
        let _ = (total_title_delta, total_content_delta); // used for running total above
    }
}

/// Finds sections that contain all `terms` from a phrase AND pass path/document
/// filters, then verifies the `raw` phrase appears as a contiguous lowercased
/// substring in the section title or content.
///
/// Returns `(SectionKey, SearchAccumulator)` pairs with a fixed phrase bonus
/// score applied.
fn matched_phrase_keys(
    index: &FullTextIndex,
    terms: &[String],
    raw: &str,
    options: &FullTextSearchOptions,
    path_prefix: Option<&str>,
) -> BTreeMap<SectionKey, SearchAccumulator> {
    if terms.is_empty() {
        return BTreeMap::new();
    }

    // Collect candidate sections that contain all phrase terms.
    let mut candidate_keys: Option<BTreeSet<SectionKey>> = None;
    for term in terms {
        let term_keys: BTreeSet<SectionKey> = index
            .postings
            .get(term.as_str())
            .into_iter()
            .flatten()
            .map(|p| p.key.clone())
            .collect();
        candidate_keys = Some(match candidate_keys {
            None => term_keys,
            Some(existing) => existing.intersection(&term_keys).cloned().collect(),
        });
        if candidate_keys.as_ref().is_some_and(|k| k.is_empty()) {
            return BTreeMap::new();
        }
    }

    let mut result = BTreeMap::new();
    for key in candidate_keys.into_iter().flatten() {
        let Some(section) = index.sections.get(&key) else {
            continue;
        };
        if !matches_filters(&key, section, options, path_prefix) {
            continue;
        }
        let lower_title = section.title.to_lowercase();
        let lower_content = section.content.to_lowercase();
        let title_hit = lower_title.contains(raw as &str);
        let content_hit = lower_content.contains(raw as &str);
        if !title_hit && !content_hit {
            continue;
        }
        // Score phrase matches with a fixed boost relative to individual terms.
        let phrase_score = if title_hit { 5.0 } else { 0.0 } + if content_hit { 2.0 } else { 0.0 };
        let acc = SearchAccumulator {
            score: phrase_score,
            matched_atoms: 1,
            title_match: title_hit,
            ..Default::default()
        };
        result.insert(key, acc);
    }
    result
}

#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
struct SectionKey {
    document_id: DocumentId,
    section_id: SectionId,
}

#[derive(Clone, Debug)]
struct IndexedSection {
    relative_path: String,
    title: String,
    content: String,
    heading_ancestry: Vec<String>,
    title_length: usize,
    content_length: usize,
}

#[derive(Clone, Debug)]
struct Posting {
    key: SectionKey,
    title_offsets: Vec<(usize, usize)>,
    content_offsets: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Default)]
struct PostingBuilder {
    title_offsets: Vec<(usize, usize)>,
    content_offsets: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Default)]
struct SearchAccumulator {
    score: f32,
    matched_atoms: usize,
    title_match: bool,
    content_offsets: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Default)]
struct AtomAccumulator {
    score: f32,
    title_match: bool,
    content_offsets: Vec<(usize, usize)>,
}

#[derive(Clone, Debug)]
struct Token {
    term: String,
    start: usize,
    end: usize,
}

/// A single search atom: either a single term (with optional prefix wildcard) or
/// an ordered phrase that must appear contiguously in the target text.
#[derive(Clone, Debug, Eq, PartialEq)]
enum QueryAtom {
    Term {
        term: String,
        prefix: bool,
    },
    /// Ordered sequence of lowercased terms that must appear as a contiguous
    /// run in title or content. `raw` is the lowercased, whitespace-normalised
    /// phrase string used for fast substring matching.
    Phrase {
        terms: Vec<String>,
        raw: String,
    },
}

/// Parses a user query into a sequence of atoms.
///
/// Text wrapped in double or single quotes is treated as an ordered phrase.
/// All other whitespace-separated tokens are individual term atoms.
/// A trailing `*` on a term enables prefix matching.
fn query_atoms(query: &str) -> Vec<QueryAtom> {
    let mut atoms: Vec<QueryAtom> = Vec::new();
    let mut chars = query.chars().peekable();
    let mut current = String::new();

    while let Some(&ch) = chars.peek() {
        if ch == '"' || ch == '\'' {
            let quote = ch;
            chars.next();
            // Flush any accumulated term before the phrase
            flush_term_atoms(&current, &mut atoms);
            current.clear();
            // Collect phrase text up to the matching closing quote
            let mut phrase = String::new();
            for c in chars.by_ref() {
                if c == quote {
                    break;
                }
                phrase.push(c);
            }
            let terms: Vec<String> = tokenize(&phrase).into_iter().map(|t| t.term).collect();
            let raw = terms.join(" ");
            if !terms.is_empty() {
                atoms.push(QueryAtom::Phrase { terms, raw });
            }
        } else if ch.is_whitespace() {
            flush_term_atoms(&current, &mut atoms);
            current.clear();
            chars.next();
        } else {
            current.push(ch);
            chars.next();
        }
    }
    flush_term_atoms(&current, &mut atoms);
    atoms
}

fn flush_term_atoms(raw: &str, atoms: &mut Vec<QueryAtom>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    let (value, prefix) = raw
        .strip_suffix('*')
        .map_or((raw, false), |value| (value, true));
    for token in tokenize(value) {
        let atom = QueryAtom::Term {
            term: token.term,
            prefix,
        };
        if !atoms.contains(&atom) {
            atoms.push(atom);
        }
    }
}

/// Tokenises text into lowercased terms, additionally splitting at
/// camelCase / PascalCase / acronym boundaries so that identifiers like
/// `camelCase`, `XMLParser`, and `getHTTPResponse` are searchable by their
/// component words.
///
/// For each alphanumeric run the full lowercased token is always emitted.
/// When the run contains case transitions, the sub-tokens are also emitted.
/// The caller deduplicates if needed (the posting builder uses a `BTreeMap`).
fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (offset, character) in text.char_indices() {
        if character.is_alphanumeric() {
            start.get_or_insert(offset);
        } else if let Some(token_start) = start.take() {
            push_tokens(&mut tokens, text, token_start, offset);
        }
    }
    if let Some(token_start) = start {
        push_tokens(&mut tokens, text, token_start, text.len());
    }
    tokens
}

fn push_tokens(tokens: &mut Vec<Token>, text: &str, start: usize, end: usize) {
    let raw = &text[start..end];
    let whole = raw.to_lowercase();
    if whole.is_empty() {
        return;
    }
    // Always emit the complete token lowercased.
    tokens.push(Token {
        term: whole.clone(),
        start,
        end,
    });

    // Additionally split camelCase / PascalCase / acronym boundaries and
    // emit sub-tokens so `camelCase` matches both `camel` and `case`.
    let sub_ranges = camel_split_ranges(raw);
    if sub_ranges.len() > 1 {
        for (sub_start, sub_end) in sub_ranges {
            let sub_term = raw[sub_start..sub_end].to_lowercase();
            if !sub_term.is_empty() && sub_term != whole {
                tokens.push(Token {
                    term: sub_term,
                    start: start + sub_start,
                    end: start + sub_end,
                });
            }
        }
    }
}

/// Returns byte ranges of camelCase/PascalCase sub-words within `text`.
///
/// Splitting rules (applied in priority order):
/// 1. Lowercase→Uppercase transition (`camelCase` → `camel | Case`)
/// 2. Multiple-uppercase run before a lowercase char (`XMLParser` → `XML | Parser`)
fn camel_split_ranges(text: &str) -> Vec<(usize, usize)> {
    // Collect (byte_offset, char) pairs so we can look ahead safely.
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    if n < 2 {
        return if n == 0 {
            vec![]
        } else {
            vec![(0, text.len())]
        };
    }

    let mut splits: Vec<usize> = vec![0]; // segment start offsets
    let mut i = 0;
    while i < n {
        let (_, ch) = chars[i];
        if i + 1 < n {
            let (next_off, next_ch) = chars[i + 1];
            // Rule 1: lowercase → uppercase
            if ch.is_lowercase() && next_ch.is_uppercase() {
                splits.push(next_off);
            }
            // Rule 2: uppercase-uppercase → lowercase (keep last upper with rest)
            // e.g. X M L P a r → split before P
            else if ch.is_uppercase()
                && next_ch.is_uppercase()
                && i + 2 < n
                && chars[i + 2].1.is_lowercase()
            {
                splits.push(next_off);
            }
        }
        i += 1;
    }
    splits.push(text.len());
    splits.windows(2).map(|w| (w[0], w[1])).collect()
}

fn matches_filters(
    key: &SectionKey,
    section: &IndexedSection,
    options: &FullTextSearchOptions,
    path_prefix: Option<&str>,
) -> bool {
    if options
        .document_id
        .as_ref()
        .is_some_and(|document_id| document_id != &key.document_id)
    {
        return false;
    }
    path_prefix.is_none_or(|prefix| {
        section.relative_path == prefix
            || section
                .relative_path
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn normalize_path_prefix(prefix: &str) -> String {
    prefix.replace('\\', "/").trim_end_matches('/').to_owned()
}

fn idf(section_count: usize, document_frequency: usize) -> f32 {
    let section_count = section_count as f32;
    let document_frequency = document_frequency as f32;
    (1.0 + (section_count - document_frequency + 0.5) / (document_frequency + 0.5)).ln()
}

fn bm25_term_score(term_frequency: usize, field_length: usize, average_length: f32) -> f32 {
    if term_frequency == 0 {
        return 0.0;
    }
    let term_frequency = term_frequency as f32;
    let field_length = field_length as f32;
    let average_length = average_length.max(1.0);
    let k1 = 1.2;
    let b = 0.75;
    term_frequency * (k1 + 1.0)
        / (term_frequency + k1 * (1.0 - b + b * field_length / average_length))
}

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

fn snippet(text: &str, match_start: usize, match_end: usize) -> String {
    const CONTEXT: usize = 80;
    let mut start = match_start.saturating_sub(CONTEXT).min(text.len());
    let mut end = match_end.saturating_add(CONTEXT).min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < text.len() { "..." } else { "" };
    format!("{prefix}{}{suffix}", text[start..end].trim())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!("vds-search-{nonce}"));
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

    fn indexed_workspace() -> (TestWorkspace, WorkspaceState, FullTextIndex) {
        let workspace = TestWorkspace::new();
        workspace.write(
            "docs/architecture.md",
            "# Architecture\n\nSystem overview.\n\n## Storage\n\nFilesystem authority and durable metadata.\n",
        );
        workspace.write(
            "guides/search.md",
            "# Search Guide\n\nMaterialization builds the lexical index.\n\n## Prefix Queries\n\nPrefix matching is available.\n",
        );
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = FullTextIndex::build(&state);
        (workspace, state, index)
    }

    #[test]
    fn searches_multiple_documents_with_bm25_title_boosting() {
        let (_workspace, state, index) = indexed_workspace();
        let results = index.search("storage filesystem", &FullTextSearchOptions::default());

        // Count actual sections from documents to understand the structure
        let actual_section_count: usize = state.documents().map(|doc| doc.sections.len()).sum();
        assert_eq!(index.section_count(), actual_section_count);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].relative_path, "docs/architecture.md");
        assert_eq!(results[0].title, "Storage");
        assert!(results[0].title_match);
        assert!(!results[0].content_matches.is_empty());
        assert!(results[0].content_matches[0].snippet.contains("Filesystem"));
    }

    #[test]
    fn supports_prefix_queries_and_path_filters() {
        let (_workspace, _state, index) = indexed_workspace();
        let options = FullTextSearchOptions {
            path_prefix: Some("guides".to_owned()),
            ..FullTextSearchOptions::default()
        };
        let results = index.search("mater*", &options);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].relative_path, "guides/search.md");
        assert_eq!(results[0].title, "Search Guide");
    }

    #[test]
    fn can_filter_to_one_document_and_use_or_semantics() {
        let (_workspace, state, index) = indexed_workspace();
        let architecture_id = state
            .document_by_path("docs/architecture.md")
            .unwrap()
            .document
            .id
            .clone();
        let options = FullTextSearchOptions {
            document_id: Some(architecture_id),
            require_all_terms: false,
            ..FullTextSearchOptions::default()
        };
        let results = index.search("metadata prefix", &options);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Storage");
    }

    // ── Code-identifier tokenization ──────────────────────────────────────────

    #[test]
    fn camel_case_identifier_is_searchable_by_component_words() {
        let workspace = TestWorkspace::new();
        workspace.write(
            "api.md",
            "# API\n\nThe `getHttpResponse` function returns a response.\n",
        );
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = FullTextIndex::build(&state);

        // Searching "getHttpResponse" exact form
        let r = index.search("gethttpresponse", &FullTextSearchOptions::default());
        assert!(!r.is_empty(), "whole camelCase token must match");

        // Searching component word "http"
        let r = index.search("http", &FullTextSearchOptions::default());
        assert!(!r.is_empty(), "sub-word 'http' from camelCase must match");

        // Searching component word "response"
        let r = index.search("response", &FullTextSearchOptions::default());
        assert!(
            !r.is_empty(),
            "sub-word 'response' from camelCase must match"
        );
    }

    #[test]
    fn pascal_case_identifier_is_searchable_by_component_words() {
        let workspace = TestWorkspace::new();
        workspace.write(
            "types.md",
            "# Types\n\nThe `DocumentStore` manages all documents.\n",
        );
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = FullTextIndex::build(&state);

        let r = index.search("document", &FullTextSearchOptions::default());
        assert!(
            !r.is_empty(),
            "'document' from PascalCase DocumentStore must match"
        );

        let r = index.search("store", &FullTextSearchOptions::default());
        assert!(
            !r.is_empty(),
            "'store' from PascalCase DocumentStore must match"
        );
    }

    #[test]
    fn acronym_run_is_split_from_following_word() {
        let workspace = TestWorkspace::new();
        workspace.write(
            "net.md",
            "# Networking\n\nUse `XMLParser` to process input.\n",
        );
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = FullTextIndex::build(&state);

        let r = index.search("xml", &FullTextSearchOptions::default());
        assert!(!r.is_empty(), "'xml' from XMLParser must match");

        let r = index.search("parser", &FullTextSearchOptions::default());
        assert!(!r.is_empty(), "'parser' from XMLParser must match");

        // Keep workspace alive until assertions complete
        drop(workspace);
    }

    #[test]
    fn camel_split_ranges_produces_correct_boundaries() {
        // camelCase: [0..5, 5..9]
        let ranges = camel_split_ranges("camelCase");
        assert_eq!(ranges, vec![(0, 5), (5, 9)]);

        // PascalCase: [0..6, 6..10]
        let ranges = camel_split_ranges("PascalCase");
        assert_eq!(ranges, vec![(0, 6), (6, 10)]);

        // XMLParser: [0..3, 3..9] — acronym run split before last upper
        let ranges = camel_split_ranges("XMLParser");
        assert_eq!(ranges, vec![(0, 3), (3, 9)]);

        // Single word — no split
        let ranges = camel_split_ranges("constant");
        assert_eq!(ranges, vec![(0, 8)]);

        // Already lowercase — no split
        let ranges = camel_split_ranges("word");
        assert_eq!(ranges, vec![(0, 4)]);
    }

    // ── Quoted phrase queries ─────────────────────────────────────────────────

    #[test]
    fn quoted_phrase_matches_contiguous_words() {
        let workspace = TestWorkspace::new();
        workspace.write(
            "guide.md",
            "# Guide\n\nThe quick brown fox jumps over the lazy dog.\n",
        );
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = FullTextIndex::build(&state);

        // Exact phrase should match
        let r = index.search("\"quick brown\"", &FullTextSearchOptions::default());
        assert!(!r.is_empty(), "exact phrase 'quick brown' should match");

        // Words in wrong order should not match as a phrase
        let r = index.search("\"brown quick\"", &FullTextSearchOptions::default());
        assert!(
            r.is_empty(),
            "'brown quick' is not contiguous in that order"
        );
    }

    #[test]
    fn quoted_phrase_in_mixed_query() {
        let workspace = TestWorkspace::new();
        workspace.write(
            "doc.md",
            "# Doc\n\nThe filesystem authority provides durable storage.\n",
        );
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = FullTextIndex::build(&state);

        let r = index.search(
            "\"filesystem authority\" durable",
            &FullTextSearchOptions::default(),
        );
        assert!(!r.is_empty(), "phrase + term query should match");
    }

    #[test]
    fn quoted_phrase_no_match_returns_empty() {
        let workspace = TestWorkspace::new();
        workspace.write("doc.md", "# Doc\n\nHello world.\n");
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let index = FullTextIndex::build(&state);

        let r = index.search(
            "\"nonexistent phrase xyz\"",
            &FullTextSearchOptions::default(),
        );
        assert!(r.is_empty(), "phrase not in any document must return empty");
    }

    #[test]
    fn query_atoms_parses_phrases_and_terms() {
        let atoms = query_atoms("foo \"bar baz\" qux*");
        assert_eq!(atoms.len(), 3);
        assert!(matches!(&atoms[0], QueryAtom::Term { term, .. } if term == "foo"));
        assert!(matches!(&atoms[1], QueryAtom::Phrase { raw, .. } if raw == "bar baz"));
        assert!(matches!(&atoms[2], QueryAtom::Term { term, prefix: true } if term == "qux"));
    }

    // ── Incremental index update ──────────────────────────────────────────────

    #[test]
    fn remove_and_add_document_replaces_postings() {
        let workspace = TestWorkspace::new();
        workspace.write("doc.md", "# Doc\n\nOriginal content here.\n");
        let state = WorkspaceState::load(&workspace.root).unwrap();
        let mut index = FullTextIndex::build(&state);

        // "original" matches before removal
        let before = index.search("original", &FullTextSearchOptions::default());
        assert!(!before.is_empty(), "should match before remove");

        let doc = state.document_by_path("doc.md").unwrap();
        let doc_id = doc.document.id.clone();

        index.remove_document(&doc_id);

        let after_remove = index.search("original", &FullTextSearchOptions::default());
        assert!(
            after_remove.is_empty(),
            "should not match after remove_document"
        );

        // Simulate a changed document by writing new content and reloading.
        workspace.write("doc.md", "# Doc\n\nUpdated content here.\n");
        let new_state = WorkspaceState::load(&workspace.root).unwrap();
        let new_doc = new_state.document_by_path("doc.md").unwrap();
        let new_sections_by_id = new_doc
            .sections
            .iter()
            .map(|s| (s.section_id.clone(), s))
            .collect::<BTreeMap<_, _>>();

        index.add_document(new_doc, &new_sections_by_id);

        let after_add = index.search("updated", &FullTextSearchOptions::default());
        assert!(
            !after_add.is_empty(),
            "should match new content after add_document"
        );

        // Old content must be gone.
        let old_term = index.search("original", &FullTextSearchOptions::default());
        assert!(
            old_term.is_empty(),
            "old postings must not remain after add_document"
        );
    }
}
