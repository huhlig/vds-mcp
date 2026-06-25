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

use crate::document::{
    Document, DocumentFormat, DocumentId, DocumentMetadata, Section, SectionId, SectionMetadata,
    SectionVersion, VersionId,
};

use chrono::Utc;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

// ── VDS 2 Filesystem-authoritative types ──────────────────────────────────────

/// Byte offsets locating one section within its source Markdown file.
///
/// These spans are used to apply surgical edits to content and headings
/// without requiring a full document re-render for every mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SectionSourceSpan {
    /// Byte offset where the heading line begins (e.g. `## Title\n`).
    pub heading_start: usize,
    /// Byte offset where section body content begins (after heading and blank line).
    pub content_start: usize,
    /// Byte offset where this section's direct content ends
    /// (start of the next heading in document order, or end of file).
    pub content_end: usize,
}

#[derive(Clone, Debug)]
pub struct ParsedMarkdown {
    /// Parsed document metadata and root identity.
    pub document: Document,
    /// Current section tree, including the synthetic root section.
    pub sections: Vec<Section>,
    /// Initial immutable version for each parsed section.
    pub versions: Vec<SectionVersion>,
    /// Byte-offset spans locating each section within the source file.
    /// The root section is not included (it has no heading line).
    pub source_spans: std::collections::BTreeMap<crate::document::SectionId, SectionSourceSpan>,
}

// ── VDS 2 Filesystem-authoritative functions ─────────────────────────────────

/// Applies a content replacement to one section in raw Markdown.
///
/// Replaces the section's body between `span.content_start` and
/// `span.content_end` with `new_content`, preserving the heading and all
/// other sections. Returns `None` if the span is out of range.
pub fn apply_content_edit(
    markdown: &str,
    span: &SectionSourceSpan,
    new_content: &str,
) -> Option<String> {
    if span.content_start > markdown.len() || span.content_end > markdown.len() {
        return None;
    }
    let before = &markdown[..span.content_start];
    let after = &markdown[span.content_end..];
    let new_content = new_content.trim_end();
    let mut result = String::with_capacity(before.len() + new_content.len() + after.len() + 4);
    result.push_str(before);
    if !new_content.is_empty() {
        result.push_str(new_content);
        result.push('\n');
        if !after.is_empty() && !after.starts_with('\n') {
            result.push('\n');
        }
    }
    result.push_str(after);
    Some(result)
}

/// Applies a heading rename to one section in raw Markdown.
///
/// Replaces only the title text within the heading line at `span.heading_start`,
/// leaving the heading prefix (`## `) and following content unchanged.
pub fn apply_heading_rename(
    markdown: &str,
    span: &SectionSourceSpan,
    new_title: &str,
) -> Option<String> {
    if span.heading_start >= span.content_start || span.content_start > markdown.len() {
        return None;
    }
    let heading_slice = &markdown[span.heading_start..span.content_start];
    // heading_slice is like "## Title\n\n" — find the space after "##"
    let prefix_end = heading_slice.find(' ')? + 1;
    let prefix = &heading_slice[..prefix_end];
    // find the end of the title text (before the first newline)
    let title_end_in_slice = heading_slice.find('\n').unwrap_or(heading_slice.len());
    // everything after the title line (e.g. blank line before content)
    let after_title_in_slice = &heading_slice[title_end_in_slice..];

    let mut result = String::with_capacity(markdown.len());
    result.push_str(&markdown[..span.heading_start]);
    result.push_str(prefix);
    result.push_str(new_title.trim());
    result.push_str(after_title_in_slice);
    result.push_str(&markdown[span.content_start..]);
    Some(result)
}

/// Renders a section tree to Markdown.
///
/// Walks the tree from `root_id` in ordinal order, emitting headings and
/// content. The root section's title is not rendered; its content is emitted
/// first as preamble.
pub fn render_sections_to_markdown(
    sections: &std::collections::BTreeMap<crate::document::SectionId, Section>,
    root_id: &crate::document::SectionId,
) -> String {
    let Some(root) = sections.get(root_id) else {
        return String::new();
    };
    let mut markdown = String::new();
    let content = root.content.trim_end();
    if !content.is_empty() {
        markdown.push_str(content);
        markdown.push('\n');
    }
    render_section_children(&mut markdown, root, sections);
    markdown
}

fn render_section_children(
    markdown: &mut String,
    parent: &Section,
    sections: &std::collections::BTreeMap<crate::document::SectionId, Section>,
) {
    for child_id in &parent.children {
        if let Some(child) = sections.get(child_id) {
            render_one_section(markdown, child);
            render_section_children(markdown, child, sections);
        }
    }
}

fn render_one_section(markdown: &mut String, section: &Section) {
    if !markdown.is_empty() && !markdown.ends_with("\n\n") {
        if !markdown.ends_with('\n') {
            markdown.push('\n');
        }
        markdown.push('\n');
    }
    let level = section.level.clamp(1, 6) as usize;
    markdown.push_str(&"#".repeat(level));
    markdown.push(' ');
    markdown.push_str(&section.title);
    // Preserve the heading ID attribute so structural mutations don't silently
    // drop custom anchors that other documents or external links depend on.
    if let Some(anchor) = &section.metadata.anchor
        && !anchor.is_empty()
    {
        markdown.push_str(" {#");
        markdown.push_str(anchor);
        markdown.push('}');
    }
    markdown.push('\n');
    let content = section.content.trim_end();
    if !content.is_empty() {
        markdown.push('\n');
        markdown.push_str(content);
        markdown.push('\n');
    }
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

/// Parses Markdown into the VDS domain model without persisting it.
///
/// This is the filesystem-authoritative materialization boundary. Callers may
/// build an in-memory workspace generation from the returned records, while
/// the legacy import path can continue storing the same records in `redb`.
pub fn parse_markdown_str(
    name: impl Into<String>,
    source_path: Option<String>,
    markdown: &str,
) -> ParsedMarkdown {
    let name = name.into();
    let now = Utc::now();
    let document_id = new_document_id();
    let root_id = new_section_id();
    let document_version = new_version_id();
    let root_version = new_version_id();
    let headings = heading_spans(markdown);
    let mut draft_section_ids: Vec<crate::document::SectionId> = vec![root_id.clone()];
    let mut draft_heading_indices: Vec<Option<usize>> = vec![None];
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
        draft_section_ids.push(section_id.clone());
        draft_heading_indices.push(Some(index));
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
        .enumerate()
        .map(|(draft_index, draft)| {
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
                draft_index,
            )
        })
        .collect::<Vec<_>>();

    let mut materialized_sections = Vec::with_capacity(sections.len());
    let mut versions = Vec::with_capacity(sections.len());
    let mut source_spans = std::collections::BTreeMap::new();
    for (section, is_root, draft_index) in sections {
        if let Some(heading_index) = draft_heading_indices[draft_index] {
            let heading = &headings[heading_index];
            let content_end = headings
                .get(heading_index + 1)
                .map(|next| next.start)
                .unwrap_or(markdown.len());
            source_spans.insert(
                section.section_id.clone(),
                SectionSourceSpan {
                    heading_start: heading.start,
                    content_start: heading.body_start,
                    content_end,
                },
            );
        }
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

    ParsedMarkdown {
        document,
        sections: materialized_sections,
        versions,
        source_spans,
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

fn trim_trailing_blank_lines(value: &str) -> String {
    value.trim_end_matches(['\n', '\r']).to_owned()
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
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_parse_markdown_basic() {
        let markdown =
            "# Title\n\nIntro\n\n## A\n\nA body\n\n### A.1\n\nNested\n\n## B\n\nB body\n";
        let imported = parse_markdown_str("guide", None, markdown);
        let sections_map: std::collections::HashMap<_, _> = imported
            .sections
            .iter()
            .map(|section| (section.section_id.clone(), section))
            .collect();
        let root = sections_map.get(&imported.document.root).unwrap();

        assert_eq!(root.children.len(), 1);
        let title = sections_map.get(&root.children[0]).unwrap();
        assert_eq!(title.title, "Title");
        assert_eq!(title.children.len(), 2);

        let first_child = sections_map.get(&title.children[0]).unwrap();
        assert_eq!(first_child.title, "A");
        assert_eq!(first_child.children.len(), 1);
        assert_eq!(imported.versions.len(), imported.sections.len());
    }

    #[test]
    fn ignores_headings_inside_fenced_code() {
        let markdown = "# Real\n\n```md\n# Not a heading\n```\n";
        let imported = parse_markdown_str("guide", None, markdown);
        assert_eq!(imported.sections.len(), 2);
        assert!(imported.sections[1].content.contains("# Not a heading"));
    }

    #[test]
    fn imports_overview_fixture_as_addressable_sections() {
        let overview = include_str!("../docs/overview.md");
        let imported =
            parse_markdown_str("overview", Some("docs/overview.md".to_owned()), overview);
        let sections_map: std::collections::HashMap<_, _> = imported
            .sections
            .iter()
            .map(|section| (section.section_id.clone(), section))
            .collect();
        let root = sections_map.get(&imported.document.root).unwrap();

        assert_eq!(imported.document.name, "overview");
        assert_eq!(
            imported.document.metadata.title.as_deref(),
            Some("Versioned Document Service \u{2014} Architecture Overview")
        );
        assert_eq!(
            imported.document.metadata.source_path.as_deref(),
            Some("docs/overview.md")
        );
        assert_eq!(root.children.len(), 1);

        let title = sections_map.get(&root.children[0]).unwrap();
        assert_eq!(
            title.title,
            "Versioned Document Service \u{2014} Architecture Overview"
        );
        assert!(title.children.iter().any(|child| {
            sections_map
                .get(child)
                .is_some_and(|section| section.title == "Document Model")
        }));
        assert_eq!(imported.versions.len(), imported.sections.len());
    }

    // ── ATX heading variants ──────────────────────────────────────────────────

    #[test]
    fn atx_headings_with_trailing_hashes_are_parsed() {
        // ATX headings may optionally close with trailing `#` characters.
        let md = "# Intro #\n\nBody.\n\n## Sub ##\n\nSub body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let sects: Vec<_> = parsed
            .sections
            .iter()
            .filter(|s| s.parent_id.is_some())
            .collect();
        assert_eq!(sects.len(), 2);
        assert!(sects.iter().any(|s| s.title == "Intro"), "title stripped");
        assert!(sects.iter().any(|s| s.title == "Sub"), "sub-title stripped");
    }

    #[test]
    fn atx_headings_skip_levels_become_children_of_nearest_ancestor() {
        // A jump from H1 directly to H3 is common in the wild.
        // H3 should be a child of H1, not a grandchild of a phantom H2.
        let md = "# Root\n\nIntro.\n\n### Deep\n\nDeep body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_id: std::collections::HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.section_id.clone(), s))
            .collect();
        let root_node = by_id.get(&parsed.document.root).unwrap();
        let h1 = by_id.get(&root_node.children[0]).unwrap();
        assert_eq!(h1.title, "Root");
        assert_eq!(h1.level, 1);
        // The H3 must be a direct child of H1 (no phantom H2 inserted).
        assert_eq!(h1.children.len(), 1);
        let h3 = by_id.get(&h1.children[0]).unwrap();
        assert_eq!(h3.title, "Deep");
        assert_eq!(h3.level, 3);
    }

    // ── Setext headings ───────────────────────────────────────────────────────

    #[test]
    fn setext_h1_and_h2_are_treated_as_headings() {
        let md = "Title\n=====\n\nIntro.\n\nSubsection\n----------\n\nSub body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_id: std::collections::HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.section_id.clone(), s))
            .collect();
        let root_node = by_id.get(&parsed.document.root).unwrap();
        assert_eq!(root_node.children.len(), 1);
        let h1 = by_id.get(&root_node.children[0]).unwrap();
        assert_eq!(h1.title, "Title");
        assert_eq!(h1.level, 1);
        assert_eq!(h1.children.len(), 1);
        let h2 = by_id.get(&h1.children[0]).unwrap();
        assert_eq!(h2.title, "Subsection");
        assert_eq!(h2.level, 2);
        assert_eq!(h2.content, "Sub body.");
    }

    #[test]
    fn setext_content_is_preserved_through_source_span_edit() {
        let md = "Title\n=====\n\nIntro.\n\nSubsection\n----------\n\nOriginal body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_id: std::collections::HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.section_id.clone(), s))
            .collect();
        let root_node = by_id.get(&parsed.document.root).unwrap();
        let h1 = by_id.get(&root_node.children[0]).unwrap();
        let h2 = by_id.get(&h1.children[0]).unwrap();

        let span = parsed
            .source_spans
            .get(&h2.section_id)
            .expect("source span must exist for setext H2");
        let updated =
            apply_content_edit(md, span, "Updated body.").expect("content edit on setext heading");
        assert!(updated.contains("Updated body."), "new content present");
        assert!(
            updated.contains("Title\n====="),
            "H1 setext heading preserved"
        );
        assert!(
            updated.contains("Subsection\n----------"),
            "H2 setext heading preserved"
        );
        assert!(!updated.contains("Original body."), "old content removed");
    }

    // ── Duplicate headings ────────────────────────────────────────────────────

    #[test]
    fn duplicate_heading_titles_produce_distinct_sections() {
        let md = "# Doc\n\n## Results\n\nFirst.\n\n## Results\n\nSecond.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let results: Vec<_> = parsed
            .sections
            .iter()
            .filter(|s| s.title == "Results")
            .collect();
        assert_eq!(results.len(), 2, "both duplicate headings become sections");
        assert_ne!(
            results[0].section_id, results[1].section_id,
            "each duplicate gets a unique ID"
        );
        assert_eq!(results[0].content, "First.");
        assert_eq!(results[1].content, "Second.");
    }

    #[test]
    fn duplicate_headings_each_have_distinct_source_spans() {
        let md = "# Doc\n\n## Dup\n\nFirst.\n\n## Dup\n\nSecond.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let dups: Vec<_> = parsed
            .sections
            .iter()
            .filter(|s| s.title == "Dup")
            .collect();
        assert_eq!(dups.len(), 2);
        let span0 = parsed
            .source_spans
            .get(&dups[0].section_id)
            .expect("span for first Dup");
        let span1 = parsed
            .source_spans
            .get(&dups[1].section_id)
            .expect("span for second Dup");
        assert!(
            span0.heading_start < span1.heading_start,
            "spans are in order"
        );

        // Content edit on first duplicate must not touch the second.
        let updated = apply_content_edit(md, span0, "Replaced.").expect("edit on first dup");
        assert!(updated.contains("Replaced."), "first replaced");
        assert!(updated.contains("Second."), "second untouched");
    }

    // ── Unicode headings ──────────────────────────────────────────────────────

    #[test]
    fn unicode_heading_titles_and_content_are_preserved() {
        let md = "# 文档标题\n\n这是正文。\n\n## Ärger mit Umlauten\n\nUmlauts: äöüß.\n\n## 🚀 Rockets\n\nEmoji content: 🎉\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: std::collections::HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        assert!(by_title.contains_key("文档标题"), "CJK title parsed");
        assert_eq!(by_title["文档标题"].content, "这是正文。");
        assert!(
            by_title.contains_key("Ärger mit Umlauten"),
            "umlaut title parsed"
        );
        assert_eq!(by_title["Ärger mit Umlauten"].content, "Umlauts: äöüß.");
        assert!(by_title.contains_key("🚀 Rockets"), "emoji title parsed");
        assert_eq!(by_title["🚀 Rockets"].content, "Emoji content: 🎉");
    }

    #[test]
    fn unicode_content_source_spans_are_byte_accurate() {
        // Byte offsets must be correct for multi-byte Unicode characters.
        let md = "# 标题\n\n原始内容。\n\n## Sub\n\nSub body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let cjk = parsed
            .sections
            .iter()
            .find(|s| s.title == "标题")
            .expect("CJK section");
        let span = parsed
            .source_spans
            .get(&cjk.section_id)
            .expect("span for CJK section");
        let updated = apply_content_edit(md, span, "新内容。").expect("edit on CJK section");
        assert!(updated.contains("新内容。"), "new CJK content written");
        assert!(!updated.contains("原始内容。"), "old CJK content removed");
        assert!(updated.contains("## Sub"), "other section untouched");
    }

    // ── Empty sections ────────────────────────────────────────────────────────

    #[test]
    fn empty_sections_parse_and_round_trip_without_content() {
        let md = "# Doc\n\n## Empty\n\n## HasContent\n\nBody.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: std::collections::HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        assert_eq!(
            by_title["Empty"].content, "",
            "empty section has no content"
        );
        assert_eq!(by_title["HasContent"].content, "Body.");

        // Source span edit on empty section should insert content cleanly.
        let empty_span = parsed
            .source_spans
            .get(&by_title["Empty"].section_id)
            .expect("span for empty section");
        let updated = apply_content_edit(md, empty_span, "Now has content.")
            .expect("insert into empty section");
        assert!(
            updated.contains("## Empty\n\nNow has content."),
            "content inserted after empty heading"
        );
        assert!(
            updated.contains("## HasContent"),
            "following section untouched"
        );
    }

    #[test]
    fn consecutive_empty_sections_all_get_distinct_spans() {
        let md = "# Doc\n\n## A\n\n## B\n\n## C\n\nOnly C has content.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: std::collections::HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        assert_eq!(by_title["A"].content, "");
        assert_eq!(by_title["B"].content, "");
        assert_eq!(by_title["C"].content, "Only C has content.");

        let span_a = parsed
            .source_spans
            .get(&by_title["A"].section_id)
            .expect("span A");
        let span_b = parsed
            .source_spans
            .get(&by_title["B"].section_id)
            .expect("span B");
        let span_c = parsed
            .source_spans
            .get(&by_title["C"].section_id)
            .expect("span C");

        assert!(span_a.heading_start < span_b.heading_start);
        assert!(span_b.heading_start < span_c.heading_start);

        // Edit B (empty) without disturbing A or C.
        let updated = apply_content_edit(md, span_b, "B content.").expect("insert into empty B");
        assert!(updated.contains("## B\n\nB content."), "B content inserted");
        assert!(updated.contains("## A\n\n## B"), "A still empty");
        assert!(updated.contains("Only C has content."), "C untouched");
    }

    // ── Golden: apply_content_edit exact output ───────────────────────────────

    #[test]
    fn golden_content_edit_middle_section() {
        let md = "# Doc\n\nPreamble.\n\n## Alpha\n\nOriginal alpha.\n\n## Beta\n\nBeta body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        let span = parsed
            .source_spans
            .get(&by_title["Alpha"].section_id)
            .unwrap();
        let result = apply_content_edit(md, span, "Replaced alpha.").unwrap();
        assert_eq!(
            result,
            "# Doc\n\nPreamble.\n\n## Alpha\n\nReplaced alpha.\n\n## Beta\n\nBeta body.\n"
        );
    }

    #[test]
    fn golden_content_edit_last_section() {
        let md = "# Doc\n\n## First\n\nFirst body.\n\n## Last\n\nOriginal last.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        let span = parsed
            .source_spans
            .get(&by_title["Last"].section_id)
            .unwrap();
        let result = apply_content_edit(md, span, "New last body.").unwrap();
        assert_eq!(
            result,
            "# Doc\n\n## First\n\nFirst body.\n\n## Last\n\nNew last body.\n"
        );
    }

    #[test]
    fn golden_content_edit_empty_to_nonempty() {
        let md = "# Doc\n\n## Empty\n\n## After\n\nAfter body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        let span = parsed
            .source_spans
            .get(&by_title["Empty"].section_id)
            .unwrap();
        let result = apply_content_edit(md, span, "Now has content.").unwrap();
        assert_eq!(
            result,
            "# Doc\n\n## Empty\n\nNow has content.\n\n## After\n\nAfter body.\n"
        );
    }

    #[test]
    fn golden_content_edit_nonempty_to_empty() {
        let md = "# Doc\n\n## Section\n\nHas content.\n\n## After\n\nAfter body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        let span = parsed
            .source_spans
            .get(&by_title["Section"].section_id)
            .unwrap();
        let result = apply_content_edit(md, span, "").unwrap();
        assert_eq!(result, "# Doc\n\n## Section\n\n## After\n\nAfter body.\n");
    }

    // ── Golden: apply_heading_rename exact output ─────────────────────────────

    #[test]
    fn golden_heading_rename_middle_section() {
        let md = "# Doc\n\n## Alpha\n\nAlpha body.\n\n## Beta\n\nBeta body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        let span = parsed
            .source_spans
            .get(&by_title["Alpha"].section_id)
            .unwrap();
        let result = apply_heading_rename(md, span, "Renamed").unwrap();
        assert_eq!(
            result,
            "# Doc\n\n## Renamed\n\nAlpha body.\n\n## Beta\n\nBeta body.\n"
        );
    }

    #[test]
    fn golden_heading_rename_only_heading_line_changes() {
        // The content below the renamed heading must be byte-for-byte identical.
        let md =
            "# Title\n\n## Section\n\nContent line 1.\nContent line 2.\n\nStill same section.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        let span = parsed
            .source_spans
            .get(&by_title["Section"].section_id)
            .unwrap();
        let result = apply_heading_rename(md, span, "New Name").unwrap();
        assert_eq!(
            result,
            "# Title\n\n## New Name\n\nContent line 1.\nContent line 2.\n\nStill same section.\n"
        );
    }

    // ── Golden: fenced code preserved through surgical edit ───────────────────

    #[test]
    fn golden_fenced_code_block_preserved_through_adjacent_edit() {
        let md = concat!(
            "# Doc\n\n",
            "## Code Section\n\n",
            "```rust\n",
            "fn main() {\n",
            "    println!(\"hello\");\n",
            "}\n",
            "```\n\n",
            "## Other\n\n",
            "Original other.\n",
        );
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        let span = parsed
            .source_spans
            .get(&by_title["Other"].section_id)
            .unwrap();
        let result = apply_content_edit(md, span, "New other.").unwrap();

        // Fenced code block must survive byte-for-byte.
        assert!(result.contains("```rust\nfn main() {\n    println!(\"hello\");\n}\n```"));
        // New content in the other section.
        assert!(result.contains("## Other\n\nNew other.\n"));
    }

    #[test]
    fn golden_fenced_code_content_is_stored_verbatim() {
        // No outer heading — just a single section so sections = [root, Snippet].
        let md = concat!(
            "## Snippet\n\n",
            "```python\n",
            "# This is a comment, not a heading\n",
            "def foo():\n",
            "    pass\n",
            "```\n",
        );
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        // The "# This is a comment" inside the fence must NOT become a section.
        assert_eq!(
            parsed.sections.len(),
            2,
            "root + Snippet only — no phantom heading"
        );
        assert!(
            by_title["Snippet"]
                .content
                .contains("# This is a comment, not a heading"),
            "code comment stored verbatim in section content"
        );
    }

    // ── Golden: HTML comments preserved through surgical edit ─────────────────

    #[test]
    fn golden_html_comment_preserved_through_adjacent_edit() {
        let md = concat!(
            "# Doc\n\n",
            "<!-- This is a top-level comment -->\n\n",
            "## Section A\n\n",
            "<!-- inline comment -->\nBody text.\n\n",
            "## Section B\n\n",
            "Original B.\n",
        );
        let parsed = parse_markdown_str("doc", None, md);
        let by_title: HashMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.title.as_str(), s))
            .collect();
        let span = parsed
            .source_spans
            .get(&by_title["Section B"].section_id)
            .unwrap();
        let result = apply_content_edit(md, span, "New B.").unwrap();

        assert!(
            result.contains("<!-- This is a top-level comment -->"),
            "top-level comment preserved"
        );
        assert!(
            result.contains("<!-- inline comment -->"),
            "inline comment preserved"
        );
        assert!(result.contains("## Section B\n\nNew B.\n"), "edit applied");
    }

    // ── Golden: heading attributes preserved through structural rendering ──────

    #[test]
    fn golden_render_preserves_heading_id_attribute() {
        let md = "# Doc {#doc-anchor}\n\nPreamble.\n\n## Sub {#sub-anchor}\n\nSub body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let sections_map: std::collections::BTreeMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.section_id.clone(), s.clone()))
            .collect();

        let rendered = render_sections_to_markdown(&sections_map, &parsed.document.root);

        assert!(
            rendered.contains("# Doc {#doc-anchor}"),
            "H1 anchor preserved in canonical render:\n{rendered}"
        );
        assert!(
            rendered.contains("## Sub {#sub-anchor}"),
            "H2 anchor preserved in canonical render:\n{rendered}"
        );
    }

    #[test]
    fn golden_render_sections_exact_output() {
        // Simple 3-section tree; confirm byte-exact canonical render output.
        let md = "# Title\n\nIntro.\n\n## Alpha\n\nAlpha body.\n\n## Beta\n\nBeta body.\n";
        let parsed = parse_markdown_str("doc", None, md);
        let sections_map: std::collections::BTreeMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.section_id.clone(), s.clone()))
            .collect();
        let rendered = render_sections_to_markdown(&sections_map, &parsed.document.root);
        assert_eq!(
            rendered,
            "# Title\n\nIntro.\n\n## Alpha\n\nAlpha body.\n\n## Beta\n\nBeta body.\n"
        );
    }

    #[test]
    fn golden_render_sections_fenced_code_exact_output() {
        let md = concat!(
            "# Guide\n\n",
            "## Install\n\n",
            "Run this:\n\n",
            "```sh\ncargo install vds\n```\n\n",
            "## Usage\n\n",
            "Just run `vds`.\n",
        );
        let parsed = parse_markdown_str("doc", None, md);
        let sections_map: std::collections::BTreeMap<_, _> = parsed
            .sections
            .iter()
            .map(|s| (s.section_id.clone(), s.clone()))
            .collect();
        let rendered = render_sections_to_markdown(&sections_map, &parsed.document.root);
        assert_eq!(
            rendered,
            concat!(
                "# Guide\n\n",
                "## Install\n\n",
                "Run this:\n\n",
                "```sh\ncargo install vds\n```\n\n",
                "## Usage\n\n",
                "Just run `vds`.\n",
            )
        );
    }
}
