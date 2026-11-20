//! WPT html5lib tree-construction compatibility test.
//!
//! This is an ignored integration test because it requires an external WPT
//! checkout under `data/wpt`. It compares `simdhtml` against the expected
//! tree only; no external parser is used as an oracle.
//!
//! ```text
//! cargo test -p simdhtml --test wpt_tree_construction -- --ignored --nocapture
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use simdhtml::{FlatDoc, NO_NODE, NodeKind, ParseMode, parse};

#[derive(Clone, Debug, Eq, PartialEq)]
enum TNode {
    Element {
        name: String,
        attrs: Vec<(String, String)>,
        children: Vec<TNode>,
    },
    Text(String),
    Comment(String),
    Doctype(String),
}

#[derive(Clone, Debug)]
struct Test {
    data: String,
    expected: Vec<TNode>,
    fragment: bool,
    script_dependent: bool,
}

fn unescape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('/') => out.push('/'),
            Some('u') => {
                let mut hex = String::new();
                for _ in 0..4 {
                    if let Some(&h) = chars.peek()
                        && h.is_ascii_hexdigit()
                    {
                        hex.push(h);
                        chars.next();
                    }
                }
                if let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(ch) = char::from_u32(code)
                {
                    out.push(ch);
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn parse_tree_lines(lines: &[String]) -> Vec<TNode> {
    let mut parsed = Vec::new();
    for line in lines {
        let Some(rest) = line.strip_prefix('|') else {
            continue;
        };
        let spaces = rest.chars().take_while(|c| *c == ' ').count();
        let depth = spaces.saturating_sub(1) / 2;
        let content = rest.trim_start();
        if content.is_empty() {
            continue;
        }
        parsed.push((depth, content.to_string()));
    }
    let mut idx = 0usize;
    parse_nodes(&parsed, &mut idx, 0)
}

fn parse_nodes(lines: &[(usize, String)], idx: &mut usize, depth: usize) -> Vec<TNode> {
    let mut out = Vec::new();
    while *idx < lines.len() {
        let (line_depth, content) = &lines[*idx];
        if *line_depth < depth {
            break;
        }
        if *line_depth > depth {
            // Attribute lines are consumed by their element below.
            *idx += 1;
            continue;
        }

        if let Some(text) = content.strip_prefix('"') {
            let text = text.strip_suffix('"').unwrap_or(text);
            out.push(TNode::Text(unescape(text)));
            *idx += 1;
        } else if let Some(comment) = content.strip_prefix("<!--") {
            let comment = comment.strip_suffix("-->").unwrap_or(comment);
            let comment = comment.strip_prefix(' ').unwrap_or(comment);
            let comment = comment.strip_suffix(' ').unwrap_or(comment);
            out.push(TNode::Comment(comment.to_string()));
            *idx += 1;
        } else if content.starts_with("<!DOCTYPE") || content.starts_with("<!doctype") {
            let name = content
                .strip_prefix("<!DOCTYPE")
                .or_else(|| content.strip_prefix("<!doctype"))
                .unwrap_or(content)
                .trim()
                .trim_end_matches('>')
                .trim();
            out.push(TNode::Doctype(name.to_string()));
            *idx += 1;
        } else if let Some(name) = content.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
            let name = name.to_string();
            *idx += 1;
            let mut attrs = Vec::new();
            while *idx < lines.len()
                && lines[*idx].0 == depth + 1
                && !lines[*idx].1.starts_with('<')
                && !lines[*idx].1.starts_with('"')
                && !lines[*idx].1.starts_with("<!--")
                && !lines[*idx].1.starts_with("<!DOCTYPE")
            {
                let attr = &lines[*idx].1;
                if let Some((n, v)) = attr.split_once('=') {
                    let v = v.trim();
                    let v = v
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                        .unwrap_or(v);
                    attrs.push((n.to_string(), unescape(v)));
                }
                *idx += 1;
            }
            attrs.sort();
            let children = parse_nodes(lines, idx, depth + 1);
            out.push(TNode::Element {
                name,
                attrs,
                children,
            });
        } else {
            *idx += 1;
        }
    }
    out
}

fn parse_dat(path: &Path) -> std::io::Result<Vec<Test>> {
    let text = fs::read_to_string(path)?;
    let mut tests = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    let mut document_lines: Vec<String> = Vec::new();
    let mut fragment = false;
    let mut script_dependent = false;
    let mut in_data = false;
    let mut in_document = false;

    let finish = |tests: &mut Vec<Test>,
                  data: &mut Vec<String>,
                  doc: &mut Vec<String>,
                  fragment: bool,
                  script: bool| {
        if !data.is_empty() || !doc.is_empty() {
            tests.push(Test {
                data: data.join("\n"),
                expected: parse_tree_lines(doc),
                fragment,
                script_dependent: script,
            });
        }
        data.clear();
        doc.clear();
    };

    for line in text.lines() {
        if line == "#data" {
            if in_document {
                finish(
                    &mut tests,
                    &mut data_lines,
                    &mut document_lines,
                    fragment,
                    script_dependent,
                );
                fragment = false;
                script_dependent = false;
            }
            in_data = true;
            in_document = false;
            continue;
        }
        if line == "#document" {
            in_data = false;
            in_document = true;
            continue;
        }
        if line == "#document-fragment" {
            fragment = true;
            in_data = false;
            in_document = false;
            continue;
        }
        if line == "#script-on" || line == "#script-off" {
            script_dependent = true;
            in_data = false;
            in_document = false;
            continue;
        }
        if line.starts_with('#') {
            in_data = false;
            in_document = false;
            continue;
        }
        if in_data {
            data_lines.push(line.to_string());
        } else if in_document && line.starts_with('|') {
            document_lines.push(line.to_string());
        } else if in_document && line.is_empty() {
            finish(
                &mut tests,
                &mut data_lines,
                &mut document_lines,
                fragment,
                script_dependent,
            );
            fragment = false;
            script_dependent = false;
            in_document = false;
        }
    }
    finish(
        &mut tests,
        &mut data_lines,
        &mut document_lines,
        fragment,
        script_dependent,
    );
    Ok(tests)
}

fn merge_text(children: Vec<TNode>) -> Vec<TNode> {
    let mut out: Vec<TNode> = Vec::new();
    for child in children {
        if let TNode::Text(text) = &child
            && let Some(TNode::Text(prev)) = out.last_mut()
        {
            prev.push_str(text);
            continue;
        }
        out.push(child);
    }
    out
}

fn normalize(node: TNode, v1: bool) -> Vec<TNode> {
    match node {
        TNode::Comment(_) | TNode::Doctype(_) if v1 => Vec::new(),
        TNode::Element {
            name,
            attrs,
            children,
        } => {
            let children = merge_text(
                children
                    .into_iter()
                    .flat_map(|c| normalize(c, v1))
                    .collect(),
            );
            if v1 && matches!(name.as_str(), "html" | "head" | "body" | "tbody") {
                children
            } else {
                vec![TNode::Element {
                    name,
                    attrs,
                    children,
                }]
            }
        }
        other => vec![other],
    }
}

fn normalize_root(nodes: Vec<TNode>, v1: bool) -> Vec<TNode> {
    merge_text(nodes.into_iter().flat_map(|n| normalize(n, v1)).collect())
}

fn simdhtml_node(doc: &FlatDoc, id: u32) -> TNode {
    let node = doc.node(id);
    match node.kind {
        NodeKind::Element => {
            let mut attrs: Vec<(String, String)> = Vec::new();
            let mut seen: Vec<std::ops::Range<u32>> = Vec::new();
            for attr in doc.attr_iter(id) {
                let name = doc.raw_attr_name(&attr);
                if seen
                    .iter()
                    .any(|span| doc.raw_attr_name_span(span).eq_ignore_ascii_case(name))
                {
                    continue;
                }
                seen.push(attr.name.clone());
                attrs.push((
                    String::from_utf8_lossy(name).into_owned(),
                    doc.attr_value(&attr).into_owned(),
                ));
            }
            attrs.sort();
            let mut children = Vec::new();
            let mut child = node.first_child;
            while child != NO_NODE {
                children.push(simdhtml_node(doc, child));
                child = doc.node(child).next_sibling;
            }
            TNode::Element {
                name: doc.tag_name(node.tag_token).to_string(),
                attrs,
                children,
            }
        }
        NodeKind::Text => TNode::Text(doc.text_of(id).into_owned()),
        NodeKind::Root => unreachable!(),
    }
}

fn simdhtml_tree(doc: &FlatDoc) -> Vec<TNode> {
    let mut out = Vec::new();
    let mut child = doc.node(doc.root).first_child;
    while child != NO_NODE {
        out.push(simdhtml_node(doc, child));
        child = doc.node(child).next_sibling;
    }
    merge_text(out)
}

struct FileStats {
    name: String,
    total: usize,
    skipped: usize,
    simdhtml_raw: usize,
    simdhtml_v1: usize,
}

fn process_file(file: &Path) -> Option<FileStats> {
    let tests = parse_dat(file).ok()?;
    let mut stats = FileStats {
        name: file.file_name()?.to_string_lossy().into_owned(),
        total: 0,
        skipped: 0,
        simdhtml_raw: 0,
        simdhtml_v1: 0,
    };
    for test in tests {
        if test.fragment || test.script_dependent || test.expected.is_empty() {
            stats.skipped += 1;
            continue;
        }
        stats.total += 1;
        let expected_raw = normalize_root(test.expected.clone(), false);
        let expected_v1 = normalize_root(test.expected.clone(), true);

        let actual = match parse(test.data.as_bytes(), ParseMode::Fast) {
            Ok(doc) => simdhtml_tree(&doc),
            Err(_) => Vec::new(),
        };
        if normalize_root(actual.clone(), false) == expected_raw {
            stats.simdhtml_raw += 1;
        }
        if normalize_root(actual, true) == expected_v1 {
            stats.simdhtml_v1 += 1;
        }
    }
    Some(stats)
}

/// Minimum number of `v1` matches expected for the checked-out WPT tree.
///
/// Update this when the WPT checkout is intentionally updated. It is a
/// regression floor, not a claim that all WPT cases are supported.
const MIN_V1_PASS: usize = 854;

#[test]
#[ignore = "requires data/wpt checkout"]
fn wpt_tree_construction() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/wpt/html/syntax/parsing/resources");
    if !root.is_dir() {
        eprintln!(
            "skip wpt_tree_construction: {} is not present",
            root.display()
        );
        return;
    }
    let mut files: Vec<_> = fs::read_dir(&root)
        .expect("WPT resources directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "dat"))
        .collect();
    files.sort();

    let stats: Vec<FileStats> = files.iter().filter_map(|file| process_file(file)).collect();
    let total: usize = stats.iter().map(|s| s.total).sum();
    let skipped: usize = stats.iter().map(|s| s.skipped).sum();
    let simdhtml_raw: usize = stats.iter().map(|s| s.simdhtml_raw).sum();
    let simdhtml_v1: usize = stats.iter().map(|s| s.simdhtml_v1).sum();
    if total == 0 {
        eprintln!("skip wpt_tree_construction: no usable WPT cases");
        return;
    }
    let mut per_file: Vec<(String, usize, usize)> = stats
        .into_iter()
        .map(|s| (s.name, s.total, s.simdhtml_v1))
        .collect();

    println!(
        "tests={total} skipped={skipped}\n\
         simdhtml raw {:5.1}%  v1 {:5.1}% ({simdhtml_v1}/{total})",
        simdhtml_raw as f64 * 100.0 / total as f64,
        simdhtml_v1 as f64 * 100.0 / total as f64,
    );
    per_file.sort_by_key(|(_, _, v1)| *v1);
    println!("\nlowest simdhtml v1 files:");
    for (name, n, v1) in per_file.iter().take(20) {
        println!("{name:38} v1 {v1:4}/{n:<4}");
    }

    assert!(
        simdhtml_v1 >= MIN_V1_PASS,
        "WPT v1 regression: {simdhtml_v1} < {MIN_V1_PASS} (total={total})"
    );
}
