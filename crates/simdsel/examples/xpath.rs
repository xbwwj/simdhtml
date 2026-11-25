//! Dumps each document's tree plus XPath results as JSON for debugging.
//!
//! Usage: `cargo run --release --example xpath -- <cc|swde|html-dir> <exprs-file> [limit]`
//!
//! A dataset name streams the corpus through `simdhtml-data` (no extraction);
//! a directory is read as a flat set of `.html` files.

use std::{
    env, fs,
    io::{self, Write},
};

use simdhtml::character_reference::decode_character_references;
use simdsel::prelude::*;

fn json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn dump_tree(out: &mut String, doc: &Document<'_>) {
    out.push_str("\"tree\":[");

    for raw in 0..doc.nodes.len() as u32 {
        let id = NodeId::new(raw);
        let node = doc.node(id);

        if raw > 0 {
            out.push(',');
        }

        out.push_str("{\"kind\":");

        match node.kind() {
            NodeKind::Document => out.push_str("\"document\""),
            NodeKind::Element => out.push_str("\"element\""),
            NodeKind::Text => out.push_str("\"text\""),
            NodeKind::Comment => out.push_str("\"comment\""),
        }

        out.push_str(",\"parent\":");
        out.push_str(&node.parent().to_string());

        if node.kind() == NodeKind::Element {
            out.push_str(",\"name\":");
            json_string(out, doc.tag_name(node.tag_token()));
            out.push_str(",\"attrs\":[");

            let mut first = true;

            for attr in doc.tags.token(node.tag_token()).attr_range.clone() {
                let name_start = doc.attrs.name_start[attr as usize] as usize;
                let name_end = name_start + doc.attrs.name_len[attr as usize] as usize;
                let name = std::str::from_utf8(&doc.input[name_start..name_end]).expect("utf-8");
                let value_start = doc.attrs.value_start[attr as usize] as usize;
                let value_end = value_start + doc.attrs.value_len[attr as usize] as usize;
                let raw = std::str::from_utf8(&doc.input[value_start..value_end]).expect("utf-8");
                let value = decode_character_references(raw, true);
                if !first {
                    out.push(',');
                }
                first = false;
                out.push('[');
                json_string(out, name);
                out.push(',');
                json_string(out, &value);
                out.push(']');
            }

            out.push(']');
        }

        if node.kind() == NodeKind::Text {
            out.push_str(",\"text\":");
            json_string(out, &doc.text(id));
        }

        out.push('}');
    }

    out.push(']');
}

/// Times evaluation only: documents and expressions are compiled/parsed once.
/// Load `limit` documents from a dataset name or a flat html directory.
fn load_docs(source: &str, limit: usize) -> Vec<(String, Vec<u8>)> {
    use simdhtml_data::{Corpus, Dataset, SamplePolicy};

    if let Some(dataset) = Dataset::from_name(source) {
        let policy = SamplePolicy {
            limit: limit.max(1),
            bytes: 256 << 20,
            ..SamplePolicy::default()
        };
        let corpus =
            Corpus::load_with(dataset, &policy).unwrap_or_else(|err| panic!("{source}: {err}"));
        return corpus
            .docs
            .into_iter()
            .map(|doc| (doc.id, doc.bytes))
            .collect();
    }

    let mut files: Vec<_> = fs::read_dir(source)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "html" || ext == "htm")
        })
        .collect();
    files.sort();
    files.truncate(limit);
    files
        .into_iter()
        .map(|path| {
            let id = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            (id, fs::read(&path).expect("read doc"))
        })
        .collect()
}

fn main() {
    let mut args = env::args().skip(1);

    let source = args.next().expect("corpus (cc|swde) or html dir");
    let exprs_path = args.next().expect("expressions file");
    let limit: usize = args.next().map_or(40, |n| n.parse().expect("limit"));

    let exprs: Vec<String> = fs::read_to_string(&exprs_path)
        .expect("read exprs")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();

    let docs = load_docs(&source, limit);

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    for (id, bytes) in docs {
        if bytes.len() > 1 << 20 {
            continue;
        }
        let Ok(doc) = Document::parse(&bytes) else {
            continue;
        };

        let mut line = String::new();
        line.push_str("{\"doc\":");
        json_string(&mut line, &id);
        line.push(',');
        dump_tree(&mut line, &doc);
        line.push_str(",\"results\":[");

        for (index, expr) in exprs.iter().enumerate() {
            if index > 0 {
                line.push(',');
            }
            line.push('[');

            match XPath::new(expr).and_then(|path| path.eval(&doc)) {
                Ok(value) => {
                    let mut first = true;
                    match value {
                        Value::Nodes(nodes) => {
                            for node in nodes {
                                if !first {
                                    line.push(',');
                                }
                                first = false;
                                line.push_str("{\"kind\":");
                                match node.kind() {
                                    RefKind::Element => line.push_str("\"element\""),
                                    RefKind::Text => line.push_str("\"text\""),
                                    RefKind::Attribute => line.push_str("\"attribute\""),
                                    RefKind::Document => line.push_str("\"document\""),
                                }
                                line.push_str(",\"name\":");
                                json_string(&mut line, node.name(&doc));
                                line.push_str(",\"value\":");
                                json_string(&mut line, &node.string_value(&doc));
                                line.push('}');
                            }
                        }
                        Value::Number(n) => {
                            // Python's `json` accepts `NaN`/`Infinity`; Rust's
                            // `Display` writes `inf`, which is not JSON.
                            let text = if n.is_nan() {
                                "NaN".to_string()
                            } else if n.is_infinite() {
                                if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
                            } else {
                                format!("{n}")
                            };
                            line.push_str(&format!("{{\"number\":{text}}}"));
                        }
                        Value::Str(s) => {
                            line.push_str("{\"string\":");
                            json_string(&mut line, &s);
                            line.push('}');
                        }
                        Value::Bool(b) => line.push_str(&format!("{{\"bool\":{b}}}")),
                    }
                }
                Err(error) => {
                    line.push_str("{\"error\":");
                    json_string(&mut line, &error.to_string());
                    line.push('}');
                }
            }
            line.push(']');
        }
        line.push_str("]}");
        writeln!(out, "{line}").expect("write");
    }
}
