//! Preorder invariants and cross-mode sanity on the CommonCrawl corpus.
//!
//! The specification-shaped constructor builds a mutable tree and flattens it,
//! so a bug easily produces an ill-formed preorder tape (which every downstream
//! pass assumes).  This walks the corpus and checks the invariants directly:
//! children ids are ascending, `subtree_end` contains the subtree, parents agree,
//! and running both modes never panics.
//!
//! ```text
//! SIMDHTML_RUN_CORPUS_TESTS=1 cargo test -p simdhtml --test mode_agreement -- --nocapture
//! ```

mod common;

use simdhtml::{Document, NO_NODE, NodeKind, ParseOptions, RepairMode};

fn check_preorder(doc: &Document<'_>, path: &std::path::Path) {
    for id in 0..doc.node_tape.len() as u32 {
        let node = doc.node(id);
        let end = node.subtree_end();
        assert!(
            end as usize <= doc.node_tape.len(),
            "{}: node {id} subtree_end {end} out of range",
            path.display()
        );
        assert!(
            end > id,
            "{}: node {id} subtree_end {end} does not contain itself",
            path.display()
        );
        let mut child = node.first_child();
        let mut previous = None;
        while child != NO_NODE {
            assert!(
                child > id && child < end,
                "{}: child {child} of {id} outside [{}, {end})",
                path.display(),
                id + 1
            );
            assert_eq!(
                doc.node(child).parent(),
                id,
                "{}: child {child} has the wrong parent",
                path.display()
            );
            if let Some(previous) = previous {
                assert!(
                    child > previous,
                    "{}: children not ascending",
                    path.display()
                );
            }
            previous = Some(child);
            child = doc.node(child).next_sibling();
        }
        if node.kind() == NodeKind::Text {
            assert_eq!(
                end,
                id + 1,
                "{}: text node {id} has children",
                path.display()
            );
        }
    }
}

#[test]
fn corpus_preorder_invariants() {
    if std::env::var_os("SIMDHTML_RUN_CORPUS_TESTS").is_none() {
        eprintln!("skip mode_agreement: set SIMDHTML_RUN_CORPUS_TESTS=1 to walk commoncrawl");
        return;
    }
    let Some(root) = common::commoncrawl_dir() else {
        common::skip("mode_agreement", None);
        return;
    };
    let limit: usize = std::env::var("AGREEMENT_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500);
    let mut files: Vec<_> = std::fs::read_dir(&root)
        .expect("corpus")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
        .collect();
    files.sort();
    files.truncate(limit);

    let mut parsed = 0usize;
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.len() > 1 << 20 {
            continue;
        }
        for mode in [RepairMode::Spec, RepairMode::None] {
            if let Ok(doc) = Document::parse_with(&bytes, ParseOptions::new().repair(mode)) {
                check_preorder(&doc, path);
            }
        }
        parsed += 1;
    }
    println!("documents: {parsed} (preorder invariants hold in both modes)");
}
