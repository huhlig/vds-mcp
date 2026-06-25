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

use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use uuid::Uuid;
use vds::markdown::{export_markdown_string, import_markdown_str};
use vds::mcp::{
    CreateDocumentParams, RenderDocumentMarkdownParams,
    SearchOptions, SearchSectionsParams, VdsMcpSurface,
};
use vds::search::{FullTextIndex, FullTextSearchOptions};
use vds::service::VdsServer;
use vds::storage::DocumentStore;
use vds::workspace::WorkspaceState;

const OVERVIEW: &str = include_str!("../docs/overview.md");

fn bench_db_path(name: &str) -> PathBuf {
    let dir = std::env::current_dir()
        .expect("current dir")
        .join("target")
        .join("bench-dbs");
    fs::create_dir_all(&dir).expect("bench db dir");
    dir.join(format!("{name}-{}.redb", Uuid::now_v7()))
}

fn measure(name: &str, iterations: usize, mut run: impl FnMut()) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        run();
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64;
    println!("{name}: {iterations} iterations in {elapsed:?} ({per_iter:.2} us/iter)");
    elapsed
}

fn bench_import_export() {
    measure("markdown import plus export", 50, || {
        let store = DocumentStore::open(bench_db_path("import-export")).expect("open store");
        let document = import_markdown_str(&store, "overview", None, black_box(OVERVIEW))
            .expect("import overview");
        let rendered =
            export_markdown_string(&store, black_box(&document.id)).expect("export overview");
        black_box(rendered);
    });
}

fn bench_service_render_and_search() {
    let server = VdsServer::open(bench_db_path("service")).expect("open server");
    let document = server
        .create_document(CreateDocumentParams {
            relative_path: Some("overview.md".to_owned()),
            name: Some("overview".to_owned()),
            title: None,
            initial_content: Some(OVERVIEW.to_owned()),
        })
        .expect("create document");

    measure("service render_document_markdown", 500, || {
        let rendered = server
            .render_document_markdown(RenderDocumentMarkdownParams {
                document_id: document.id.clone(),
            })
            .expect("render document");
        black_box(rendered);
    });

    measure("service search_sections", 500, || {
        let results = server
            .search_sections(SearchSectionsParams {
                document_id: document.id.clone(),
                query: "document".to_owned(),
                options: Some(SearchOptions {
                    include_content: true,
                    include_titles: true,
                    fuzzy_titles: false,
                    max_results: None,
                }),
            })
            .expect("search sections");
        black_box(results);
    });
}

fn bench_workspace_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::current_dir()
        .expect("current dir")
        .join("target")
        .join("bench-workspaces");
    fs::create_dir_all(&dir).expect("bench workspace dir");
    let workspace = dir.join(format!("{name}-{nonce}"));
    fs::create_dir_all(&workspace).expect("create workspace");
    workspace
}

fn bench_vds2_workspace_operations() {
    let workspace = bench_workspace_path("vds2-ops");

    // Create test markdown file
    fs::write(workspace.join("overview.md"), OVERVIEW).expect("write overview");

    measure("VDS 2.0 workspace load", 100, || {
        let state = WorkspaceState::load(black_box(&workspace)).expect("load workspace");
        black_box(state);
    });

    // Load once for subsequent operations
    let state = WorkspaceState::load(&workspace).expect("load workspace");

    measure("VDS 2.0 full-text index build", 100, || {
        let index = FullTextIndex::build(black_box(&state));
        black_box(index);
    });

    let index = FullTextIndex::build(&state);

    measure("VDS 2.0 full-text search", 1000, || {
        let results = index.search(
            black_box("document filesystem"),
            &FullTextSearchOptions::default(),
        );
        black_box(results);
    });

    measure("VDS 2.0 phrase search", 1000, || {
        let results = index.search(
            black_box("\"section tree\""),
            &FullTextSearchOptions::default(),
        );
        black_box(results);
    });

    measure("VDS 2.0 prefix search", 1000, || {
        let results = index.search(
            black_box("document*"),
            &FullTextSearchOptions::default(),
        );
        black_box(results);
    });
}

fn main() {
    println!("\n=== VDS 1.0 (Legacy) Benchmarks ===\n");
    bench_import_export();
    bench_service_render_and_search();

    println!("\n=== VDS 2.0 (Filesystem) Benchmarks ===\n");
    bench_vds2_workspace_operations();
}
