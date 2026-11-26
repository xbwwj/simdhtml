/*!
WPT html5lib tree-construction compatibility test.
The `.dat` files come from the `data/wpt` submodule
(`html/syntax/parsing/resources`, the html5lib tree-construction suite,
maintained in WPT). It compares `simdhtml` against the expected tree.

```sh
cargo test -p simdhtml-tests --test wpt_tree_construction -- --ignored --nocapture
```

The `.data` file looks like:

```text
#data
Test
#errors
(1,0): expected-doctype-but-got-chars
#document
| <html>
|   <head>
|   <body>
|     "Test"

#data
<p>One<p>Two
#errors
(1,3): expected-doctype-but-got-start-tag
#document
| <html>
|   <head>
|   <body>
|     <p>
|       "One"
|     <p>
|       "Two"

... many other tests
```
*/

use std::fs;
use std::path::Path;

use simdhtml::character_reference::decode_character_references;
use simdhtml::document::Document;
use simdhtml::document::id::{DOC_ROOT, NO_NODE, NodeId};
use simdhtml::document::namespace::Namespace;
use simdhtml::document::node::NodeKind;
use simdhtml::document::text::{normalize_newlines, replace_nulls};
use simdhtml::error::ParseError;
use simdhtml::parse::{ParseOptions, RepairMode};

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

/// Reads the `#document` block of a `.dat` case.
///
/// There is no unescaping: the reference runner compares the raw expected text
/// against its own serialisation, so a text node or attribute value is printed
/// verbatim (a newline inside a text node is a literal newline, which is why a
/// text node can span several lines).
fn parse_tree_lines(lines: &[String]) -> Vec<TNode> {
    let mut parsed: Vec<(usize, String)> = Vec::new();
    for line in lines {
        // A quoted text node may contain a literal newline; the continuation is
        // either an unprefixed line or another `|` line whose content does not
        // open a quote of its own (`| "foo` then `| bar"`).
        // A line that is just `"` opens a quoted text node whose content starts
        // with a newline; it must not count as terminated by its own quote.
        let unterminated = parsed.last().is_some_and(|(_, previous)| {
            previous.starts_with('"') && !(previous.len() > 1 && previous.ends_with('"'))
        });
        let Some(rest) = line.strip_prefix('|') else {
            if unterminated {
                let (_, previous) = parsed.last_mut().expect("checked above");
                previous.push('\n');
                previous.push_str(line);
            }
            continue;
        };
        let spaces = rest.chars().take_while(|c| *c == ' ').count();
        let depth = spaces.saturating_sub(1) / 2;
        let content = rest.trim_start();
        if content.is_empty() {
            continue;
        }
        if unterminated {
            let (_, previous) = parsed.last_mut().expect("checked above");
            previous.push('\n');
            previous.push_str(content);
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
            out.push(TNode::Text(text.to_string()));
            *idx += 1;
        } else if let Some(pi) = content.strip_prefix("<?") {
            // The expected-tree files spell a processing instruction as
            // `<?data ?>`, `<?data?>` or `<?data>` depending on the file, while
            // the token's data is always the text between `<?` and the closing
            // `>`.  Canonicalise the printed form here so both sides agree.
            let pi = pi.trim_end();
            let pi = pi
                .strip_suffix("?>")
                .map(|rest| rest.trim_end())
                .unwrap_or(pi);
            out.push(TNode::Comment(format!("?{pi}")));
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
        } else if content == "content" {
            // Template contents.  The expected-tree format writes them under a
            // literal `content` line; our DOM keeps them as the template's
            // children, so the marker is transparent for the comparison.
            *idx += 1;
            out.extend(parse_nodes(lines, idx, depth + 1));
        } else if let Some(name) = content.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
            let name = name.to_string();
            *idx += 1;
            let mut attrs = Vec::new();
            while *idx < lines.len()
                && lines[*idx].0 == depth + 1
                && lines[*idx].1 != "content"
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
                    attrs.push((n.to_string(), v.to_string()));
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
        } else if in_document {
            // Continuation lines of a quoted text node (which may contain a
            // literal newline) have no `|` prefix and must reach the parser.
            document_lines.push(line.to_string());
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

/// The comparison normaliser.
///
/// There is exactly one caliber: the tree the standard's algorithms produce,
/// with adjacent text merged (a parser may split a run anywhere).  Comments,
/// processing instructions, doctypes and the `html`/`head`/`body`/`tbody`
/// wrappers are all part of that tree - the old "v1" caliber that dropped them
/// was scaffolding from the period when we could not produce them, and it was
/// retired once the full-tree score reached the reference implementation's.
fn normalize_root(nodes: Vec<TNode>) -> Vec<TNode> {
    merge_text(nodes.into_iter().map(normalize).collect())
}

/// Merges adjacent text nodes recursively; everything else is kept.
fn normalize(node: TNode) -> TNode {
    match node {
        TNode::Element {
            name,
            attrs,
            children,
        } => TNode::Element {
            name,
            attrs,
            children: merge_text(children.into_iter().map(normalize).collect()),
        },
        other => other,
    }
}

/// Decoded attribute value.
fn attr_value<'a>(doc: &'a Document<'_>, attr: u32) -> std::borrow::Cow<'a, str> {
    let start = doc.attrs.value_start[attr as usize] as usize;
    let end = start + doc.attrs.value_len[attr as usize] as usize;
    let raw = std::str::from_utf8(&doc.input[start..end]).expect("valid utf-8");
    normalize_newlines(replace_nulls(decode_character_references(raw, true)))
}

/// Attribute name as the expected-tree format prints it.
///
/// The DOM keeps `xlink:href` in one string; the reference serialiser prints a
/// namespaced attribute as `prefix local`.  That is a presentation difference,
/// so it is resolved here rather than by splitting the parser's name spans.
fn attr_display_name(name: &str, foreign: bool) -> String {
    if !foreign {
        return name.to_owned();
    }
    if name == "xmlns" {
        return "xmlns xmlns".to_owned();
    }
    // Only the attributes in the standard's "adjust foreign attributes" table
    // become namespaced; `xml:base` stays a single local name.
    if matches!(
        name,
        "xlink:actuate"
            | "xlink:arcrole"
            | "xlink:href"
            | "xlink:role"
            | "xlink:show"
            | "xlink:title"
            | "xlink:type"
            | "xml:lang"
            | "xml:space"
            | "xmlns:xlink"
    ) {
        let (prefix, local) = name.split_once(':').expect("checked above");
        return format!("{prefix} {local}");
    }
    name.to_owned()
}

/// The expected-tree spelling of a processing instruction, if the node is one.
///
/// The reference serialiser writes `<?target data?>` with one space between
/// the target and the data, and the expected files were produced by it; the
/// tokenizer's own states decide where the target ends (whitespace after it is
/// a separator, not data) and where the data ends (`?>` or a bare `>`).
fn processing_instruction(input: &[u8], span: core::ops::Range<u32>) -> Option<String> {
    use simdhtml::token_data::{PiForm, comment_data, pi_form};
    let body = &input[span.start as usize..span.end as usize];
    if !body.starts_with(b"<?") {
        return None;
    }
    let slice = |range: core::ops::Range<u32>| {
        std::str::from_utf8(&input[range.start as usize..range.end as usize])
            .expect("valid utf-8")
            .to_owned()
    };
    match pi_form(input, span.clone()) {
        PiForm::Instruction { target, data } => {
            let printed = format!("?{} {}", slice(target), slice(data));
            Some(printed.trim_end().to_owned())
        }
        // A bogus comment: its data is what follows the `<`, so it keeps the
        // `?` that the expected trees print.
        _ => Some(format!("?{}", slice(comment_data(input, span)))),
    }
}

fn simdhtml_node(doc: &Document, id: NodeId) -> TNode {
    let node = doc.node(id);
    match node.kind() {
        NodeKind::Element => {
            let mut attrs: Vec<(String, String)> = Vec::new();
            let mut seen: Vec<String> = Vec::new();
            let tag = node.tag_token();
            let foreign = node.namespace() != Namespace::HTML;
            for attr in doc.tags.token(tag).attr_range.clone() {
                let name = attr_display_name(&doc.attr_name(attr), foreign);
                if seen.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                    continue;
                }
                seen.push(name.clone());
                attrs.push((name, attr_value(doc, attr).into_owned()));
            }
            attrs.sort();
            let mut children = Vec::new();
            let mut child = node.first_child();
            while child != NO_NODE {
                children.push(simdhtml_node(doc, child));
                child = doc.node(child).next_sibling();
            }
            // Element names are ASCII-lowercased by the HTML tokenizer, and
            // foreign elements are reported with their namespace, as the WPT
            // expected trees do (`svg svg`, `math math`).
            let name = doc.tag_name_lower(node.tag_token());
            let name = match node.namespace() {
                Namespace::SVG => format!("svg {name}"),
                Namespace::MATHML => format!("math {name}"),
                _ => name.into_owned(),
            };
            TNode::Element {
                name,
                attrs,
                children,
            }
        }
        NodeKind::Text => TNode::Text(doc.text(id).into_owned()),
        NodeKind::Comment => {
            let span = node.span();
            let input: &[u8] = &doc.input;
            let text = |r: core::ops::Range<u32>| {
                let raw = std::str::from_utf8(&input[r.start as usize..r.end as usize])
                    .expect("valid utf-8");
                normalize_newlines(replace_nulls(std::borrow::Cow::Borrowed(raw))).into_owned()
            };
            if node.is_doctype() {
                // The expected-tree format writes the name plus the quoted
                // public/system identifiers: exactly the DOCTYPE token's
                // three fields.
                let parts = simdhtml::token_data::doctype_parts(input, span);
                // The DOM reports the doctype name lowercased.
                let mut out = text(parts.name).to_ascii_lowercase();
                // The reference serialiser writes both identifiers as soon as
                // one of them exists, with `""` for the missing one.
                if parts.public.is_some() || parts.system.is_some() {
                    let public = parts.public.map(text).unwrap_or_default();
                    let system = parts.system.map(text).unwrap_or_default();
                    out.push_str(&format!(" \"{public}\" \"{system}\""));
                }
                TNode::Doctype(out)
            } else if let Some(printed) = processing_instruction(input, span.clone()) {
                TNode::Comment(printed)
            } else {
                let data = simdhtml::token_data::comment_data(input, span);
                TNode::Comment(text(data))
            }
        }
        NodeKind::Document => unreachable!(),
    }
}

fn simdhtml_tree(doc: &Document) -> Vec<TNode> {
    let mut out = Vec::new();
    let mut child = doc.node(DOC_ROOT).first_child();
    while child != NO_NODE {
        out.push(simdhtml_node(doc, child));
        child = doc.node(child).next_sibling();
    }
    merge_text(out)
}

/// A one-line description of where two trees first disagree.
///
/// The WPT score says *how many* cases fail; this says *which rule* fails
/// first, by walking both trees in preorder and naming the node at the first
/// difference: "expected `form` where we had `body`" is the form-pointer rule,
/// "expected nothing where we had `div`" is an extra element, and so on.
#[derive(Clone, Debug)]
enum Divergence {
    /// Expected a child list to end where ours continues.
    Extra(String),
    /// Our child list ended where the expected one continues.
    Missing(String),
    /// Same position, different node.
    Different(String, String),
    /// Same element, different attributes.
    Attributes(String),
}

impl Divergence {
    fn bucket(&self) -> String {
        match self {
            Self::Extra(ours) => format!("extra <{ours}>"),
            Self::Missing(want) => format!("missing <{want}>"),
            Self::Different(want, ours) => format!("<{want}> vs <{ours}>"),
            Self::Attributes(name) => format!("attrs of <{name}>"),
        }
    }
}

fn node_label(node: &TNode) -> String {
    match node {
        TNode::Element { name, .. } => name.clone(),
        TNode::Text(_) => "#text".to_owned(),
        TNode::Comment(_) => "#comment".to_owned(),
        TNode::Doctype(_) => "#doctype".to_owned(),
    }
}

fn divergence(expected: &[TNode], actual: &[TNode]) -> Option<Divergence> {
    for (index, want) in expected.iter().enumerate() {
        let Some(ours) = actual.get(index) else {
            return Some(Divergence::Missing(node_label(want)));
        };
        match (want, ours) {
            (
                TNode::Element {
                    name: want_name,
                    attrs: want_attrs,
                    children: want_children,
                },
                TNode::Element {
                    name: our_name,
                    attrs: our_attrs,
                    children: our_children,
                },
            ) => {
                if want_name != our_name {
                    return Some(Divergence::Different(want_name.clone(), our_name.clone()));
                }
                if want_attrs != our_attrs {
                    return Some(Divergence::Attributes(want_name.clone()));
                }
                if let Some(inner) = divergence(want_children, our_children) {
                    return Some(inner);
                }
            }
            (TNode::Text(want), TNode::Text(ours)) => {
                if want != ours {
                    return Some(Divergence::Different(
                        "#text".to_owned(),
                        "#text".to_owned(),
                    ));
                }
            }
            (TNode::Comment(want), TNode::Comment(ours)) => {
                if want != ours {
                    return Some(Divergence::Different(
                        "#comment".to_owned(),
                        "#comment".to_owned(),
                    ));
                }
            }
            (TNode::Doctype(want), TNode::Doctype(ours)) => {
                if want != ours {
                    return Some(Divergence::Different(
                        "#doctype".to_owned(),
                        "#doctype".to_owned(),
                    ));
                }
            }
            _ => {
                return Some(Divergence::Different(node_label(want), node_label(ours)));
            }
        }
    }
    actual
        .get(expected.len())
        .map(|ours| Divergence::Extra(node_label(ours)))
}

struct FileStats {
    name: String,
    total: usize,
    skipped: usize,
    passed: usize,
}

/// Parses under the only caliber there is: the tree the standard's algorithms
/// produce.
fn parse_mode(data: &str) -> Result<Document<'_>, ParseError> {
    Document::parse_with(
        data.as_bytes(),
        ParseOptions {
            repair: RepairMode::Spec,
            ..Default::default()
        },
    )
}

fn process_file(
    file: &Path,
    buckets: &mut std::collections::BTreeMap<String, (usize, Vec<String>)>,
) -> Option<FileStats> {
    let tests = parse_dat(file).ok()?;
    let mut stats = FileStats {
        name: file.file_name()?.to_string_lossy().into_owned(),
        total: 0,
        skipped: 0,
        passed: 0,
    };
    for test in tests {
        if test.fragment || test.script_dependent || test.expected.is_empty() {
            stats.skipped += 1;
            continue;
        }
        stats.total += 1;
        let expected = normalize_root(test.expected.clone());
        let actual = normalize_root(match parse_mode(&test.data) {
            Ok(doc) => simdhtml_tree(&doc),
            Err(_) => Vec::new(),
        });
        let simdhtml_ok = actual == expected;
        if simdhtml_ok {
            stats.passed += 1;
        } else {
            let bucket = match divergence(&expected, &actual) {
                Some(found) => found.bucket(),
                None => "same tree (normaliser?)".to_owned(),
            };
            let entry = buckets.entry(bucket).or_default();
            entry.0 += 1;
            if entry.1.len() < 3 {
                entry.1.push(test.data.clone());
            }
        }
    }
    Some(stats)
}

/// Minimum number of passing cases expected for the checked-out WPT tree.
///
/// Update this when the WPT checkout is intentionally updated.  It is a
/// regression floor, not a claim that all WPT cases are supported.
const MIN_PASS: usize = 1691;

#[test]
#[ignore = "needs data/wpt"]
fn wpt_tree_construction() {
    let Some(root) = simdhtml_data::wpt_resources() else {
        panic!("data/wpt missing; run `cargo xtask data`");
    };
    let mut files: Vec<_> = fs::read_dir(&root)
        .expect("WPT resources directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "dat"))
        .collect();
    files.sort();

    let mut buckets: std::collections::BTreeMap<String, (usize, Vec<String>)> =
        std::collections::BTreeMap::new();
    let stats: Vec<FileStats> = files
        .iter()
        .filter_map(|file| process_file(file, &mut buckets))
        .collect();
    let total: usize = stats.iter().map(|s| s.total).sum();
    let skipped: usize = stats.iter().map(|s| s.skipped).sum();
    let passed: usize = stats.iter().map(|s| s.passed).sum();
    assert!(total > 0, "no usable WPT cases");
    let mut per_file: Vec<(String, usize, usize)> = stats
        .into_iter()
        .map(|s| (s.name, s.total, s.passed))
        .collect();

    println!(
        "tests={total} skipped={skipped}\n\
         simdhtml {:5.1}% ({passed}/{total})",
        passed as f64 * 100.0 / total as f64,
    );
    per_file.sort_by_key(|(_, _, pass)| *pass);
    println!("\nsimdhtml by file (lowest first):");
    for (name, n, pass) in per_file.iter() {
        println!("{name:38} {pass:4}/{n:<4}");
    }

    let mut rows: Vec<(String, usize, Vec<String>)> = buckets
        .into_iter()
        .map(|(key, (count, examples))| (key, count, examples))
        .collect();
    rows.sort_by_key(|(_, count, _)| std::cmp::Reverse(*count));
    if !rows.is_empty() {
        println!("\nfailures by first divergence ({} buckets):", rows.len());
        for (key, count, examples) in rows.iter().take(40) {
            println!("{count:5}  {key}");
            for example in examples {
                let short: String = example.chars().take(90).collect();
                println!("         {short}");
            }
        }
    }

    assert!(
        passed >= MIN_PASS,
        "WPT regression: {passed} < {MIN_PASS} (total={total})"
    );
}
