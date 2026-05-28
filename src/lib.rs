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

//! Versioned Document Service (VDS) is a small MCP Service dedicated to providing complex
//! document management to agents for things like plans, development guides, explainations, etc.
//!
//! Problem: Large documents mess up context. Lots of agents need to add, update, remove, move,
//! sections around and frequently corrupt the result in the process or have to rewrite everything
//! from scratch.
//!
//! VDS Seeks to allow agents to manipulate a document and iteratively update sections,
//! subsections, etc in a structured way. All documents are in markdown format
//!
//! MCP Commands:
//! list_documents ()
//! create_document (name)
//! import_document (name, path ) -> document_id
//! export_document ( document_id, path )
//! delete_document (document_id)
//! table_of_contents ( document_id ) -> toc
//! create_section (document_id, parent, title, content ) -> sectioninfo
//! rename_section(document_id, section_id, title) -> sectioninfo
//! update_section (document_id, section_id, content ) -> (sectioninfo, sectioninfo)
//! remove_section (document_id, section_id ) -> sectioninfo
//! search_sections(document_id, query) -> section_matches
//! move_section(document_id, section_id, new_parent, position) -> sectioninfo
//! section_versions( document_id, section_id ) -> versionlist
//! switch_version( document_id, section_id, version )
//! get_section(document_id, section_id) -> section
//! insert_section_before(document_id, sibling_section_id, title, content)
//! insert_section_after(document_id, sibling_section_id, title, content)
//! diff_section_versions(document_id, section_id, a, b) -> diff
//! validate_document(document_id) -> diagnostics
//! snapshot_document(document_id, label) -> version
//! document_versions(document_id) -> versionlist
//! switch_document_version(document_id, version)

pub mod document;
pub mod markdown;
pub mod mcp;
pub mod service;
pub mod storage;
