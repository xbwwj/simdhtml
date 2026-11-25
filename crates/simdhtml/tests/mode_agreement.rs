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

use simdhtml::document::Document;
use simdhtml::document::id::{NO_NODE, NodeId};
use simdhtml::document::node::NodeKind;
use simdhtml::parse::{ParseOptions, RepairMode};
use simdhtml_corpus::{Dataset, SamplePolicy};

fn check_preorder(doc: &Document<'_>, path: &std::path::Path) {
    for raw in 0..doc.nodes.len() as u32 {
        let id = NodeId::new(raw);
        let node = doc.node(id);
        let end = node.subtree_end();
        assert!(
            end.index() <= doc.nodes.len(),
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
    let limit: usize = std::env::var("AGREEMENT_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500);
    let policy = SamplePolicy {
        limit,
        bytes: 256 * 1024 * 1024,
        ..SamplePolicy::default()
    };
    let Ok(corpus) = simdhtml_corpus::Corpus::load_with(Dataset::CommonCrawl, &policy) else {
        simdhtml_corpus::skip("mode_agreement", None);
        return;
    };

    let mut parsed = 0usize;
    for source in &corpus.docs {
        let bytes = &source.bytes;
        if bytes.len() > 1 << 20 {
            continue;
        }
        let path = std::path::Path::new(&source.id);
        for repair in [RepairMode::Spec, RepairMode::None] {
            if let Ok(doc) = Document::parse_with(
                bytes,
                ParseOptions {
                    repair,
                    ..Default::default()
                },
            ) {
                check_preorder(&doc, path);
            }
        }
        parsed += 1;
    }
    println!("documents: {parsed} (preorder invariants hold in both modes)");
}
