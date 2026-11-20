use std::env;

use simdhtml::{Document, NodeKind};

fn main() {
    let input = env::args().nth(1).expect("usage: probe <raw html>");
    let doc = Document::parse(input.as_bytes()).expect("parse");

    for id in 0..doc.node_tape.len() as u32 {
        let node = doc.node(id);
        let depth = "  ".repeat(node.depth() as usize);

        let kind = match node.kind() {
            NodeKind::Element => {
                let tag = node.tag_token();
                let attrs: Vec<String> = doc.tags[tag as usize]
                    .attr_range
                    .clone()
                    .map(|a| format!("{}={}", doc.attr_name(a), a))
                    .collect();
                format!(
                    "<{}> ns={} attrs[{}]",
                    doc.tag_name_lower(tag),
                    node.namespace(),
                    attrs.join(" ")
                )
            }
            NodeKind::Text => format!("text {:?}", doc.text(id)),
            NodeKind::Comment => {
                let span = node.span();
                format!(
                    "comment raw {:?} data {:?}",
                    String::from_utf8_lossy(&doc.input[span.start as usize..span.end as usize]),
                    simdhtml::token_data::comment_data(&doc.input, span)
                )
            }
            NodeKind::Document => "#document".to_owned(),
        };

        println!("{id} {depth}{kind}");
    }
}
