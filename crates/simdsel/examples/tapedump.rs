fn main() {
    let path = std::env::args().nth(1).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let doc = simdhtml::Document::parse_with(&bytes, simdhtml::ParseOptions::new().repair(simdhtml::RepairMode::Spec)).unwrap();
    for id in 0..doc.node_tape.len() as u32 {
        let n = doc.node(id);
        let kind = match n.kind() {
            simdhtml::NodeKind::Document => "doc",
            simdhtml::NodeKind::Element => "E",
            simdhtml::NodeKind::Text => "T",
            simdhtml::NodeKind::Comment => "C",
        };
        let name = if n.kind() == simdhtml::NodeKind::Element { doc.tag_name(n.tag_token()).to_string() } else { String::new() };
        let span = n.span();
        let src = &bytes[span.start as usize..(n.outer_end() as usize).min(bytes.len())];
        println!("{id} {kind} {name} parent={} src={:?}", n.parent(), String::from_utf8_lossy(src));
    }
}
