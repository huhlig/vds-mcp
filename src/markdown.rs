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

//! Markdown import and export helpers.
//!
//! VDS treats Markdown as the serialization format at the boundary. These
//! helpers parse Markdown into a stable section tree for storage, and render a
//! stored section tree back into Markdown in sibling ordinal order.

use std::fs;
use std::path::Path;

use crate::document::{
    Document, DocumentFormat, DocumentId, DocumentMetadata, Section, SectionId, SectionMetadata,
    SectionVersion, VersionId,
};
use crate::storage::{DocumentStore, Result};
use chrono::Utc;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Imports a Markdown file into storage and returns the created document.
///
/// The importer creates a synthetic root section, uses `pulldown_cmark` to find
/// Markdown headings, stores initial section versions, and persists everything
/// in one transaction.
pub fn import_markdown_file(
    store: &DocumentStore,
    name: impl Into<String>,
    path: impl AsRef<Path>,
) -> Result<Document> {
    let path = path.as_ref();
    let markdown = fs::read_to_string(path)?;
    import_markdown_str(
        store,
        name,
        Some(path.to_string_lossy().into_owned()),
        &markdown,
    )
}

/// Imports Markdown content into storage and returns the created document.
///
/// Content before the first heading is stored on the synthetic root section.
/// Heading detection is delegated to `pulldown_cmark`, so headings inside code
/// blocks are ignored and CommonMark heading forms are handled consistently.
/// Section parentage is inferred from heading levels, so an `h3` becomes the
/// nearest descendant of the preceding lower-level heading.
pub fn import_markdown_str(
    store: &DocumentStore,
    name: impl Into<String>,
    source_path: Option<String>,
    markdown: &str,
) -> Result<Document> {
    let name = name.into();
    let imported = parse_markdown(name, source_path, markdown);
    store.store_document_state(
        &imported.document,
        &imported.sections,
        &imported.versions,
        &[],
    )?;
    Ok(imported.document)
}

/// Exports a stored document to a Markdown file.
///
/// Sections are rendered by walking the stored tree from the document root and
/// visiting children in ordinal order.
pub fn export_markdown_file(
    store: &DocumentStore,
    document_id: &DocumentId,
    path: impl AsRef<Path>,
) -> Result<u64> {
    let markdown = export_markdown_string(store, document_id)?;
    fs::write(path, markdown.as_bytes())?;
    Ok(markdown.len() as u64)
}

/// Exports a stored document to a Markdown string.
///
/// The synthetic root section's title is not rendered. Its direct content is
/// emitted first, followed by all child sections recursively.
pub fn export_markdown_string(store: &DocumentStore, document_id: &DocumentId) -> Result<String> {
    let Some(document) = store.get_document(document_id)? else {
        return Ok(String::new());
    };
    let Some(root) = store.get_section(&document.root)? else {
        return Ok(String::new());
    };

    let mut markdown = String::new();
    append_root_content(&mut markdown, &root.content);
    append_children(store, document_id, Some(&document.root), &mut markdown)?;
    Ok(markdown)
}

/// Renders one section subtree as Markdown.
///
/// Unlike whole-document export, the requested section heading is included in
/// the output before its content and descendants.
pub fn render_section_markdown_string(
    store: &DocumentStore,
    document_id: &DocumentId,
    section_id: &SectionId,
    include_children: bool,
) -> Result<String> {
    let Some(section) = store.get_section(section_id)? else {
        return Ok(String::new());
    };

    let mut markdown = String::new();
    append_section(&mut markdown, &section);
    if include_children {
        append_children(store, document_id, Some(section_id), &mut markdown)?;
    }
    Ok(markdown)
}

#[derive(Clone, Debug)]
struct ImportedMarkdown {
    document: Document,
    sections: Vec<Section>,
    versions: Vec<SectionVersion>,
}

#[derive(Clone, Debug)]
struct SectionDraft {
    section_id: SectionId,
    parent_id: Option<SectionId>,
    title: String,
    level: u8,
    content: String,
    ordinal: u32,
    children: Vec<SectionId>,
    anchor: Option<String>,
}

#[derive(Clone, Debug)]
struct HeadingSpan {
    level: u8,
    title: String,
    anchor: Option<String>,
    start: usize,
    body_start: usize,
}

#[derive(Clone, Debug)]
struct HeadingCapture {
    level: u8,
    title: String,
    anchor: Option<String>,
    start: usize,
    end: usize,
}

fn parse_markdown(name: String, source_path: Option<String>, markdown: &str) -> ImportedMarkdown {
    let now = Utc::now();
    let document_id = new_document_id();
    let root_id = new_section_id();
    let document_version = new_version_id();
    let root_version = new_version_id();
    let headings = heading_spans(markdown);
    let mut drafts = vec![SectionDraft {
        section_id: root_id.clone(),
        parent_id: None,
        title: name.clone(),
        level: 0,
        content: root_content(markdown, &headings),
        ordinal: 0,
        children: Vec::new(),
        anchor: None,
    }];
    let mut stack: Vec<usize> = vec![0];

    for (index, heading) in headings.iter().enumerate() {
        while let Some(&ancestor) = stack.last() {
            if drafts[ancestor].level < heading.level {
                break;
            }
            stack.pop();
        }

        let parent = stack.last().copied().unwrap_or(0);
        let section_id = new_section_id();
        let ordinal = drafts[parent].children.len() as u32;
        drafts[parent].children.push(section_id.clone());
        drafts.push(SectionDraft {
            section_id,
            parent_id: Some(drafts[parent].section_id.clone()),
            title: heading.title.clone(),
            level: heading.level,
            content: section_content(markdown, &headings, index),
            ordinal,
            children: Vec::new(),
            anchor: heading.anchor.clone(),
        });
        stack.push(drafts.len() - 1);
    }

    let title = first_heading_title(&drafts).or_else(|| Some(name.clone()));
    let sections = drafts
        .into_iter()
        .map(|draft| {
            let is_root = draft.parent_id.is_none();
            let current_version = if draft.parent_id.is_none() {
                root_version.clone()
            } else {
                new_version_id()
            };
            (
                Section {
                    section_id: draft.section_id,
                    document_id: document_id.clone(),
                    parent_id: draft.parent_id,
                    children: draft.children,
                    title: draft.title,
                    level: draft.level,
                    content: trim_trailing_blank_lines(&draft.content),
                    ordinal: draft.ordinal,
                    current_version,
                    metadata: SectionMetadata {
                        anchor: draft.anchor,
                        tags: Vec::new(),
                        summary: None,
                        locked: false,
                    },
                    embedding: None,
                    created_at: now,
                    updated_at: now,
                },
                is_root,
            )
        })
        .collect::<Vec<_>>();

    let mut materialized_sections = Vec::with_capacity(sections.len());
    let mut versions = Vec::with_capacity(sections.len());
    for (section, is_root) in sections {
        versions.push(SectionVersion {
            version_id: section.current_version.clone(),
            section_id: section.section_id.clone(),
            title: section.title.clone(),
            content: section.content.clone(),
            metadata: section.metadata.clone(),
            embedding: section.embedding.clone(),
            created_at: now,
            author: None,
            change_summary: Some(if is_root {
                "Imported Markdown root".to_owned()
            } else {
                "Imported Markdown section".to_owned()
            }),
        });
        materialized_sections.push(section);
    }

    let document = Document {
        id: document_id,
        name,
        root: root_id,
        current_version: document_version,
        metadata: DocumentMetadata {
            title,
            description: None,
            tags: Vec::new(),
            source_path,
            format: DocumentFormat::Markdown,
        },
        embedding: None,
        created_at: now,
        updated_at: now,
    };

    ImportedMarkdown {
        document,
        sections: materialized_sections,
        versions,
    }
}

fn heading_spans(markdown: &str) -> Vec<HeadingSpan> {
    let parser = Parser::new_ext(markdown, Options::ENABLE_HEADING_ATTRIBUTES);
    let mut current: Option<HeadingCapture> = None;
    let mut headings = Vec::new();

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Heading {
                level,
                id,
                classes: _,
                attrs: _,
            }) => {
                current = Some(HeadingCapture {
                    level: heading_level(level),
                    title: String::new(),
                    anchor: id.map(|id| id.to_string()),
                    start: range.start,
                    end: range.end,
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(mut heading) = current.take() {
                    heading.end = heading.end.max(range.end);
                    headings.push(HeadingSpan {
                        level: heading.level,
                        title: heading.title.trim().to_owned(),
                        anchor: heading.anchor,
                        start: heading.start,
                        body_start: skip_leading_line_breaks(markdown, heading.end),
                    });
                }
            }
            Event::Text(text)
            | Event::Code(text)
            | Event::Html(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => {
                if let Some(heading) = current.as_mut() {
                    heading.end = heading.end.max(range.end);
                    heading.title.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(heading) = current.as_mut() {
                    heading.end = heading.end.max(range.end);
                    heading.title.push(' ');
                }
            }
            _ => {
                if let Some(heading) = current.as_mut() {
                    heading.end = heading.end.max(range.end);
                }
            }
        }
    }

    headings
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn root_content(markdown: &str, headings: &[HeadingSpan]) -> String {
    let end = headings
        .first()
        .map(|heading| heading.start)
        .unwrap_or(markdown.len());
    trim_trailing_blank_lines(&markdown[..end])
}

fn section_content(markdown: &str, headings: &[HeadingSpan], index: usize) -> String {
    let start = headings[index].body_start;
    let end = headings
        .get(index + 1)
        .map(|heading| heading.start)
        .unwrap_or(markdown.len());
    trim_boundary_line_breaks(&markdown[start..end])
}

fn append_children(
    store: &DocumentStore,
    document_id: &DocumentId,
    parent_id: Option<&SectionId>,
    markdown: &mut String,
) -> Result<()> {
    for child in store.list_child_sections(document_id, parent_id)? {
        append_section(markdown, &child);
        append_children(store, document_id, Some(&child.section_id), markdown)?;
    }
    Ok(())
}

fn append_section(markdown: &mut String, section: &Section) {
    ensure_blank_line(markdown);
    let level = section.level.clamp(1, 6) as usize;
    markdown.push_str(&"#".repeat(level));
    markdown.push(' ');
    markdown.push_str(&section.title);
    markdown.push_str("\n\n");

    let content = trim_trailing_blank_lines(&section.content);
    if !content.is_empty() {
        markdown.push_str(&content);
        markdown.push('\n');
    }
}

fn append_root_content(markdown: &mut String, content: &str) {
    let content = trim_trailing_blank_lines(content);
    if !content.is_empty() {
        markdown.push_str(&content);
        markdown.push('\n');
    }
}

fn ensure_blank_line(markdown: &mut String) {
    if markdown.is_empty() {
        return;
    }
    if !markdown.ends_with('\n') {
        markdown.push('\n');
    }
    if !markdown.ends_with("\n\n") {
        markdown.push('\n');
    }
}

fn trim_trailing_blank_lines(value: &str) -> String {
    value
        .trim_end_matches(|char| char == '\n' || char == '\r')
        .to_owned()
}

fn trim_boundary_line_breaks(value: &str) -> String {
    value
        .trim_matches(|char| char == '\n' || char == '\r')
        .to_owned()
}

fn skip_leading_line_breaks(markdown: &str, index: usize) -> usize {
    let mut cursor = index;
    while let Some(rest) = markdown.get(cursor..) {
        if rest.starts_with("\r\n") {
            cursor += 2;
        } else if rest.starts_with('\n') || rest.starts_with('\r') {
            cursor += 1;
        } else {
            break;
        }
    }
    cursor
}

fn first_heading_title(drafts: &[SectionDraft]) -> Option<String> {
    drafts
        .iter()
        .find(|draft| draft.level == 1 && !draft.title.is_empty())
        .map(|draft| draft.title.clone())
}

fn new_document_id() -> DocumentId {
    DocumentId::new_v7()
}

fn new_section_id() -> SectionId {
    SectionId::new_v7()
}

fn new_version_id() -> VersionId {
    VersionId::new_v7()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use uuid::Uuid;

    #[test]
    fn imports_headings_into_tree_and_exports_in_order() {
        let markdown =
            "# Title\n\nIntro\n\n## A\n\nA body\n\n### A.1\n\nNested\n\n## B\n\nB body\n";
        let imported = parse_markdown("guide".to_owned(), None, markdown);
        let sections_by_id = imported
            .sections
            .iter()
            .map(|section| (section.section_id.clone(), section))
            .collect::<HashMap<_, _>>();
        let root = sections_by_id.get(&imported.document.root).unwrap();

        assert_eq!(root.children.len(), 1);
        let title = sections_by_id.get(&root.children[0]).unwrap();
        assert_eq!(title.title, "Title");
        assert_eq!(title.children.len(), 2);

        let first_child = sections_by_id.get(&title.children[0]).unwrap();
        assert_eq!(first_child.title, "A");
        assert_eq!(first_child.children.len(), 1);
        assert_eq!(imported.versions.len(), imported.sections.len());
    }

    #[test]
    fn ignores_headings_inside_fenced_code() {
        let markdown = "# Real\n\n```md\n# Not a heading\n```\n";
        let imported = parse_markdown("guide".to_owned(), None, markdown);
        assert_eq!(imported.sections.len(), 2);
        assert!(imported.sections[1].content.contains("# Not a heading"));
    }

    #[test]
    fn imports_to_storage_and_exports_markdown_in_tree_order() {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test-dbs")
            .join(format!("{}.redb", Uuid::now_v7()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let store = DocumentStore::open(path).unwrap();
        let document = import_markdown_str(
            &store,
            "guide",
            None,
            "# Title\n\nIntro\n\n## B\n\nB body\n\n## A\n\nA body\n",
        )
        .unwrap();

        let markdown = export_markdown_string(&store, &document.id).unwrap();

        assert_eq!(
            markdown,
            "# Title\n\nIntro\n\n## B\n\nB body\n\n## A\n\nA body\n"
        );
    }

    #[test]
    fn imports_overview_fixture_as_addressable_sections() {
        let overview = include_str!("../docs/overview.md");
        let imported = parse_markdown(
            "overview".to_owned(),
            Some("docs/overview.md".to_owned()),
            overview,
        );
        let sections_by_id = imported
            .sections
            .iter()
            .map(|section| (section.section_id.clone(), section))
            .collect::<HashMap<_, _>>();
        let root = sections_by_id.get(&imported.document.root).unwrap();

        assert_eq!(imported.document.name, "overview");
        assert_eq!(
            imported.document.metadata.title.as_deref(),
            Some("Versioned Document Service Overview")
        );
        assert_eq!(
            imported.document.metadata.source_path.as_deref(),
            Some("docs/overview.md")
        );
        assert_eq!(root.children.len(), 1);

        let title = sections_by_id.get(&root.children[0]).unwrap();
        assert_eq!(title.title, "Versioned Document Service Overview");
        assert!(title.children.iter().any(|child| {
            sections_by_id
                .get(child)
                .is_some_and(|section| section.title == "Basic Architecture")
        }));
        assert_eq!(imported.versions.len(), imported.sections.len());
    }

    #[test]
    fn overview_fixture_round_trips_key_markdown_sections() {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test-dbs")
            .join(format!("{}.redb", Uuid::now_v7()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let store = DocumentStore::open(path).unwrap();
        let document = import_markdown_str(
            &store,
            "overview",
            Some("docs/overview.md".to_owned()),
            include_str!("../docs/overview.md"),
        )
        .unwrap();

        let markdown = export_markdown_string(&store, &document.id).unwrap();

        assert!(markdown.contains("# Versioned Document Service Overview"));
        assert!(markdown.contains("## What It Does"));
        assert!(markdown.contains("### Storage Layer"));
        assert!(markdown.contains("## Current Status"));
    }
}
