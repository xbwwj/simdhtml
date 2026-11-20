//! Dumps each document's tree plus XPath results as JSON, to test difference with
//! `scripts/xpath_diff.py`.
//!
//! Usage: `cargo run --release --example xpath_dump -- <html-dir> <exprs-file> [limit]`

use std::{
    env, fs,
    io::{self, Write},
};

use simdsel::{Document, NodeKind, RefKind, Value, XPath};

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

    for id in 0..doc.node_tape.len() as u32 {
        let node = doc.node(id);

        if id > 0 {
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

            for attr in doc.tags[node.tag_token() as usize].attr_range.clone() {
                let name_start = doc.attr_name_start[attr as usize] as usize;
                let name_end = name_start + doc.attr_name_len[attr as usize] as usize;
                let name = std::str::from_utf8(&doc.input[name_start..name_end]).expect("utf-8");
                let value_start = doc.attr_value_start[attr as usize] as usize;
                let value_end = value_start + doc.attr_value_len[attr as usize] as usize;
                let raw = std::str::from_utf8(&doc.input[value_start..value_end]).expect("utf-8");
                let value = simdhtml::decode_character_references(raw, true);
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
fn bench(files: &[std::path::PathBuf], exprs: &[String]) {
    let compiled: Vec<XPath> = exprs
        .iter()
        .map(|expr| XPath::new(expr).unwrap_or_else(|error| panic!("{expr}: {error}")))
        .collect();

    // Parse in place so the documents keep borrowing their buffers.
    let mut docs: Vec<Document<'_>> = Vec::new();
    let mut buffers: Vec<Vec<u8>> = Vec::new();

    for path in files {
        let bytes = fs::read(path).expect("read doc");
        if bytes.len() > 1 << 20 {
            continue;
        }
        buffers.push(bytes);
    }

    let mut lengths = Vec::new();

    for bytes in &buffers {
        if let Ok(doc) = Document::parse(bytes) {
            lengths.push(bytes.len());
            docs.push(doc);
        }
    }

    let bytes_total: usize = lengths.iter().sum();
    let reps = 5;
    let mut hits = 0usize;
    let start = std::time::Instant::now();

    for _ in 0..reps {
        for doc in &docs {
            for path in &compiled {
                if let Ok(Value::Nodes(nodes)) = path.eval(doc) {
                    hits += nodes.len();
                }
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    let queries = (reps * docs.len() * compiled.len()) as f64;

    println!(
        "docs {} exprs {} -> {:.0} ns/query, {:.0} queries/s, {:.1} MB/s of document bytes ({} hits)",
        docs.len(),
        compiled.len(),
        elapsed * 1e9 / queries,
        queries / elapsed,
        (bytes_total * reps) as f64 / elapsed / 1e6,
        hits
    );
}

fn main() {
    let mut args = env::args().skip(1);

    let dir = args.next().expect("html dir");
    let exprs_path = args.next().expect("expressions file");
    let limit: usize = args.next().map_or(40, |n| n.parse().expect("limit"));

    let exprs: Vec<String> = fs::read_to_string(&exprs_path)
        .expect("read exprs")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();

    let mut files: Vec<_> = fs::read_dir(&dir)
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

    if env::var("XPATH_BENCH").is_ok() {
        bench(&files, &exprs);
        return;
    }

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    for path in files {
        let bytes = fs::read(&path).expect("read doc");

        if bytes.len() > 1 << 20 {
            continue;
        }
        let Ok(doc) = Document::parse(&bytes) else {
            continue;
        };

        let mut line = String::new();
        line.push_str("{\"doc\":");
        json_string(&mut line, &path.file_name().unwrap().to_string_lossy());
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
                        Value::Number(n) => line.push_str(&format!("{{\"number\":{n}}}")),
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
