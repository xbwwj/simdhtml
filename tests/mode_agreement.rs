//! Preorder invariants and cross-mode sanity on the CommonCrawl corpus.
//!
//! The specification-shaped constructor builds a mutable tree and flattens it,
//! so a bug easily produces an ill-formed preorder tape (which every downstream
//! pass assumes). This walks the corpus and checks the invariants directly:
//! children ids are ascending, `subtree_end` contains the subtree, parents agree,
//! and running both modes never panics.
//!
//! ```text
//! cargo test --release -p simdhtml-tests --test mode_agreement -- --ignored --nocapture
//! cargo test -p simdhtml-tests --features quick --test mode_agreement -- --ignored
//! ```

use simdhtml::document::Document;
use simdhtml::document::id::{NO_NODE, NodeId};
use simdhtml::document::node::NodeKind;
use simdhtml::parse::{ParseOptions, RepairMode};
use simdhtml_data::{Dataset, SamplePolicy};

fn policy() -> SamplePolicy {
    if cfg!(feature = "quick") {
        return SamplePolicy::quick();
    }
    SamplePolicy {
        limit: 500,
        bytes: 256 * 1024 * 1024,
        ..SamplePolicy::default()
    }
}

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
#[ignore = "needs data/commoncrawl"]
fn corpus_preorder_invariants() {
    let start = std::time::Instant::now();
    let corpus = simdhtml_data::Corpus::load_with(Dataset::CommonCrawl, &policy())
        .unwrap_or_else(|err| panic!("{err}"));
    println!(
        "corpus: {} docs, {:.1} MiB loaded in {:.1}s",
        corpus.len(),
        corpus.mib(),
        start.elapsed().as_secs_f64()
    );
    let parse_start = std::time::Instant::now();

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
    println!(
        "documents: {parsed} (preorder invariants hold in both modes) in {:.1}s",
        parse_start.elapsed().as_secs_f64()
    );
}
