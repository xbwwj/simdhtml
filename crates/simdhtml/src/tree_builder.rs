use std::ops::Range;

use crate::document::Document;
use crate::document::attr::AttrTable;
use crate::document::flags::{DiagnosticKind, DocumentFlags};
use crate::document::id::{DOC_ROOT, NO_NODE, NO_TAG, NodeId, TagId};
use crate::document::namespace::Namespace;
use crate::document::node::NodeKind;
use crate::document::tag::TagTable;
use crate::document::tape::{NodeRecordInit, NodeTape};
use crate::markup::MarkupFlags;
use crate::parse::RepairMode;

mod spec;
mod tag_sets;
use crate::markup::MarkupIndex;
use crate::markup::text_runs::{TextRun, TextRuns};
use crate::tokenizer::{TagKind, TagToken, Tokens};
/// SVG names the specification adjusts to camelCase; the 1-based index is what
/// [`crate::document::Document::tag_name_override`] stores.
pub(crate) const SVG_TAG_NAMES: [(&str, &str); 37] = [
    ("altglyph", "altGlyph"),
    ("altglyphdef", "altGlyphDef"),
    ("altglyphitem", "altGlyphItem"),
    ("animatecolor", "animateColor"),
    ("animatemotion", "animateMotion"),
    ("animatetransform", "animateTransform"),
    ("clippath", "clipPath"),
    ("feblend", "feBlend"),
    ("fecolormatrix", "feColorMatrix"),
    ("fecomponenttransfer", "feComponentTransfer"),
    ("fecomposite", "feComposite"),
    ("feconvolvematrix", "feConvolveMatrix"),
    ("fediffuselighting", "feDiffuseLighting"),
    ("fedisplacementmap", "feDisplacementMap"),
    ("fedistantlight", "feDistantLight"),
    ("fedropshadow", "feDropShadow"),
    ("feflood", "feFlood"),
    ("fefunca", "feFuncA"),
    ("fefuncb", "feFuncB"),
    ("fefuncg", "feFuncG"),
    ("fefuncr", "feFuncR"),
    ("fegaussianblur", "feGaussianBlur"),
    ("feimage", "feImage"),
    ("femerge", "feMerge"),
    ("femergenode", "feMergeNode"),
    ("femorphology", "feMorphology"),
    ("feoffset", "feOffset"),
    ("fepointlight", "fePointLight"),
    ("fespecularlighting", "feSpecularLighting"),
    ("fespotlight", "feSpotLight"),
    ("fetile", "feTile"),
    ("feturbulence", "feTurbulence"),
    ("foreignobject", "foreignObject"),
    ("glyphref", "glyphRef"),
    ("lineargradient", "linearGradient"),
    ("radialgradient", "radialGradient"),
    ("textpath", "textPath"),
];

/// Attribute names the specification adjusts on foreign elements: the SVG list
/// followed by MathML's `definitionurl` -> `definitionURL`.  1-based index, `0`
/// keeps the source spelling.
/// Number of SVG entries at the front of [`ADJUSTED_ATTR_NAMES`].
pub(crate) const SVG_ATTRIBUTE_COUNT: usize = 58;

pub(crate) const ADJUSTED_ATTR_NAMES: [(&str, &str); 59] = [
    ("attributename", "attributeName"),
    ("attributetype", "attributeType"),
    ("basefrequency", "baseFrequency"),
    ("baseprofile", "baseProfile"),
    ("calcmode", "calcMode"),
    ("clippathunits", "clipPathUnits"),
    ("diffuseconstant", "diffuseConstant"),
    ("edgemode", "edgeMode"),
    ("filterunits", "filterUnits"),
    ("glyphref", "glyphRef"),
    ("gradienttransform", "gradientTransform"),
    ("gradientunits", "gradientUnits"),
    ("kernelmatrix", "kernelMatrix"),
    ("kernelunitlength", "kernelUnitLength"),
    ("keypoints", "keyPoints"),
    ("keysplines", "keySplines"),
    ("keytimes", "keyTimes"),
    ("lengthadjust", "lengthAdjust"),
    ("limitingconeangle", "limitingConeAngle"),
    ("markerheight", "markerHeight"),
    ("markerunits", "markerUnits"),
    ("markerwidth", "markerWidth"),
    ("maskcontentunits", "maskContentUnits"),
    ("maskunits", "maskUnits"),
    ("numoctaves", "numOctaves"),
    ("pathlength", "pathLength"),
    ("patterncontentunits", "patternContentUnits"),
    ("patterntransform", "patternTransform"),
    ("patternunits", "patternUnits"),
    ("pointsatx", "pointsAtX"),
    ("pointsaty", "pointsAtY"),
    ("pointsatz", "pointsAtZ"),
    ("preservealpha", "preserveAlpha"),
    ("preserveaspectratio", "preserveAspectRatio"),
    ("primitiveunits", "primitiveUnits"),
    ("refx", "refX"),
    ("refy", "refY"),
    ("repeatcount", "repeatCount"),
    ("repeatdur", "repeatDur"),
    ("requiredextensions", "requiredExtensions"),
    ("requiredfeatures", "requiredFeatures"),
    ("specularconstant", "specularConstant"),
    ("specularexponent", "specularExponent"),
    ("spreadmethod", "spreadMethod"),
    ("startoffset", "startOffset"),
    ("stddeviation", "stdDeviation"),
    ("stitchtiles", "stitchTiles"),
    ("surfacescale", "surfaceScale"),
    ("systemlanguage", "systemLanguage"),
    ("tablevalues", "tableValues"),
    ("targetx", "targetX"),
    ("targety", "targetY"),
    ("textlength", "textLength"),
    ("viewbox", "viewBox"),
    ("viewtarget", "viewTarget"),
    ("xchannelselector", "xChannelSelector"),
    ("ychannelselector", "yChannelSelector"),
    ("zoomandpan", "zoomAndPan"),
    // `adjust MathML attributes`.
    ("definitionurl", "definitionURL"),
];

/// Index+1 for an adjusted attribute name, given the element's namespace: the
/// SVG table applies to SVG elements only, the MathML entry to MathML only.
pub(crate) fn adjusted_attr_index(name: &[u8], ns: Namespace) -> Option<u8> {
    let (table, offset): (&[(&str, &str)], usize) = if ns == Namespace::MATHML {
        (
            &ADJUSTED_ATTR_NAMES[SVG_ATTRIBUTE_COUNT..],
            SVG_ATTRIBUTE_COUNT,
        )
    } else {
        (&ADJUSTED_ATTR_NAMES[..SVG_ATTRIBUTE_COUNT], 0)
    };
    table
        .iter()
        .position(|(lower, _)| lower.as_bytes().eq_ignore_ascii_case(name))
        .map(|index| (index + offset + 1) as u8)
}

/// Adjusted attribute name for an [`Document::attr_name_override`] index.
pub(crate) fn adjusted_attr_name(index: u8) -> &'static str {
    ADJUSTED_ATTR_NAMES[(index as usize - 1).min(ADJUSTED_ATTR_NAMES.len() - 1)].1
}

/// The `tag_name_override` index that renames `image` to `img`.
pub(crate) fn image_alias_index() -> u8 {
    SVG_TAG_NAMES.len() as u8 + 1
}

/// Adjusted name for a `tag_name_override` index.
pub(crate) fn adjusted_tag_name(index: u8) -> &'static str {
    if index as usize == SVG_TAG_NAMES.len() + 1 {
        return "img";
    }
    SVG_TAG_NAMES[(index as usize - 1).min(SVG_TAG_NAMES.len() - 1)].1
}

/// Index+1 for a (possibly mixed-case) SVG name that needs adjustment.
pub(crate) fn svg_tag_adjustment_lower(name: &[u8]) -> Option<u8> {
    SVG_TAG_NAMES
        .iter()
        .position(|(lower, _)| lower.as_bytes().eq_ignore_ascii_case(name))
        .map(|index| (index + 1) as u8)
}

/// Inner content span of a `<![CDATA[ ... ]]>` section: the scanner reports the
/// whole section, the tree wants only the content.
fn cdata_inner(input: &[u8], span: &Range<u32>) -> Option<Range<u32>> {
    let body = input.get(span.start as usize..span.end as usize)?;
    let rest = body.strip_prefix(b"<![CDATA[")?;
    // An unterminated section runs to end of file, and its content is what is
    // left (the `]]>` is not required by the "eof-in-cdata" error case).
    let inner_len = match rest.strip_suffix(b"]]>") {
        Some(inner) => inner.len(),
        None => rest.len(),
    };
    let inner_start = span.start + 9;
    Some(inner_start..inner_start + inner_len as u32)
}

/// How a start tag's name affects namespace assignment.  `Spec` decides the
/// namespace itself and hands one byte per node to the tape; the no-repair mode
/// has no integration points.
fn raw_mode_namespace(name: &[u8]) -> Namespace {
    if name.eq_ignore_ascii_case(b"svg") {
        Namespace::SVG
    } else if name.eq_ignore_ascii_case(b"math") {
        Namespace::MATHML
    } else {
        Namespace::HTML
    }
}

pub(crate) fn build_with<'a>(
    input: std::borrow::Cow<'a, [u8]>,
    index: &MarkupIndex,
    mut tokens: Tokens,
    mode: RepairMode,
) -> Document<'a> {
    let input_len = input.len() as u32;
    let node_capacity = tokens.tags.len() + index.text_runs.len() + 1;
    let mut builder = TreeBuilder::new(input_len, node_capacity, tokens.tags.len());

    let (normal_flags, diagnostics, tag_name_override, merges, extra_tag_elements) =
        spec::build(&input, &tokens, index, mode, &mut builder);

    // "Adjust SVG attributes" / "Adjust MathML attributes": the table applies
    // only to foreign elements, so the namespace of the element the token
    // created is what decides.  Attribute columns are per attribute, and the
    // merge step below copies them along with the rest of the row.
    let mut attr_name_override = vec![0u8; tokens.attr_name_start.len()];
    for (tag_index, &element) in builder.tag_element.iter().enumerate() {
        if element == NO_NODE {
            continue;
        }
        let ns = builder.nodes.namespace_of(element);
        if ns != Namespace::SVG && ns != Namespace::MATHML {
            continue;
        }
        for attr in tokens.tags[tag_index].attr_range.clone() {
            let start = tokens.attr_name_start[attr as usize] as usize;
            let len = tokens.attr_name_len[attr as usize] as usize;
            if let Some(index) = adjusted_attr_index(&input[start..start + len], ns) {
                attr_name_override[attr as usize] = index;
            }
        }
    }

    // `html`/`body` start tags merge their attributes into the existing element.
    // The attribute columns are append-only and a token's range must stay
    // contiguous, so the merged element gets a fresh run: its own attributes
    // followed by the missing ones.
    let source_tag_count_now = tokens.tags.len();
    for (dest_node, src) in merges {
        let dest_node = NodeId::new(dest_node);
        // The destination is a node id: for an implied `html`/`body` element
        // there is no tag token of its own, so find the tag token that created
        // it (`tag_element` also covers the implied ones).
        let Some(dest) = builder
            .tag_element
            .iter()
            .position(|&node| node == dest_node)
        else {
            continue;
        };
        if dest >= source_tag_count_now {
            // Implied element: no attributes of its own, so the source token's
            // attribute run can be shared outright.
            let range = tokens.tags[src as usize].attr_range.clone();
            if !range.is_empty() {
                builder.implied_tags[dest - source_tag_count_now].attr_range = range;
            }
            continue;
        }
        let dest = dest as u32;
        let dest_range = tokens.tags[dest as usize].attr_range.clone();
        let src_range = tokens.tags[src as usize].attr_range.clone();
        let mut missing: Vec<u32> = Vec::new();
        for attr in src_range.clone() {
            let start = tokens.attr_name_start[attr as usize] as usize;
            let len = tokens.attr_name_len[attr as usize] as usize;
            let name = input[start..start + len].to_vec();
            let present = dest_range.clone().any(|existing| {
                let es = tokens.attr_name_start[existing as usize] as usize;
                let el = tokens.attr_name_len[existing as usize] as usize;
                input[es..es + el].eq_ignore_ascii_case(&name)
            });
            if !present {
                missing.push(attr);
            }
        }
        if missing.is_empty() {
            continue;
        }
        let start = tokens.attr_tag.len() as u32;
        let mut all: Vec<u32> = dest_range.clone().collect();
        all.extend(missing);
        for attr in all {
            tokens.attr_tag.push(TagId::new(dest));
            tokens
                .attr_name_start
                .push(tokens.attr_name_start[attr as usize]);
            tokens
                .attr_name_len
                .push(tokens.attr_name_len[attr as usize]);
            tokens
                .attr_value_start
                .push(tokens.attr_value_start[attr as usize]);
            tokens
                .attr_value_len
                .push(tokens.attr_value_len[attr as usize]);
            tokens.attr_quote.push(tokens.attr_quote[attr as usize]);
            attr_name_override.push(attr_name_override[attr as usize]);
        }
        let end = tokens.attr_tag.len() as u32;
        tokens.tags[dest as usize].attr_range = start..end;
    }

    let mut flags = normal_flags | map_markup_flags(index.flags);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnexpectedEndTag)
    {
        flags |= DocumentFlags::UNEXPECTED_END_TAG;
    }
    builder.finish(input_len);
    let source_tag_count = tokens.tags.len();
    let mut tag_code: Vec<u8> = tokens.tag_code.iter().map(|&c| c as u8).collect();
    for name in &builder.implied_literals {
        tag_code.push(crate::tag_code::code_of(
            name.expect("implied name").as_bytes(),
        ));
    }
    let mut tag_element = std::mem::take(&mut builder.tag_element);
    tokens.tags.extend(builder.implied_tags);
    tag_element.resize(tokens.tags.len(), NO_NODE);
    for _ in 0..builder.implied_literals.len() {
        tokens.tag_name_start.push(0);
        tokens.tag_name_len.push(0);
    }
    let tags = TagTable {
        tokens: tokens.tags,
        element: tag_element,
        name_override: tag_name_override,
        name_start: tokens.tag_name_start,
        name_len: tokens.tag_name_len,
        code: tag_code,
        source_count: source_tag_count as u32,
        implied_names: builder
            .implied_literals
            .into_iter()
            .map(|name| name.expect("implied name"))
            .collect(),
    };
    let attrs = AttrTable {
        tag: tokens.attr_tag,
        name_start: tokens.attr_name_start,
        name_len: tokens.attr_name_len,
        value_start: tokens.attr_value_start,
        value_len: tokens.attr_value_len,
        quote: tokens.attr_quote,
        name_override: attr_name_override,
    };
    Document {
        input,
        encoding: encoding_rs::UTF_8,
        had_encoding_errors: false,
        nodes: builder.nodes,
        tags,
        attrs,
        flags,
        diagnostics,
        extra_tag_elements,
    }
}

fn map_markup_flags(flags: MarkupFlags) -> DocumentFlags {
    let mut out = DocumentFlags::empty();
    if flags.contains(MarkupFlags::UNBALANCED_TAG) {
        out |= DocumentFlags::UNBALANCED_TAG;
    }
    if flags.contains(MarkupFlags::NEEDS_FALLBACK) {
        out |= DocumentFlags::NEEDS_FALLBACK;
    }
    if flags.contains(MarkupFlags::EOF_IN_TAG) {
        out |= DocumentFlags::EOF_IN_TAG;
    }
    if flags.contains(MarkupFlags::EOF_IN_COMMENT) {
        out |= DocumentFlags::EOF_IN_COMMENT;
    }
    if flags.contains(MarkupFlags::EOF_IN_DOCTYPE) {
        out |= DocumentFlags::EOF_IN_DOCTYPE;
    }
    if flags.contains(MarkupFlags::EOF_IN_RAW_TEXT) {
        out |= DocumentFlags::EOF_IN_RAW_TEXT;
    }
    if flags.contains(MarkupFlags::EOF_IN_CDATA) {
        out |= DocumentFlags::EOF_IN_CDATA;
    }
    out
}

struct TreeBuilder {
    nodes: NodeTape,
    stack: Vec<NodeId>,
    last_child: Vec<NodeId>,
    tag_element: Vec<NodeId>,
    implied_tags: Vec<TagToken>,
    implied_literals: Vec<Option<&'static str>>,
    source_tag_count: usize,
    /// `(tag token, final node)` for the second and later elements emitted from
    /// one start tag (adoption agency, formatting reconstruction).
    extra_tag_elements: Vec<(TagId, NodeId)>,
}

impl TreeBuilder {
    fn new(input_len: u32, node_capacity: usize, tag_count: usize) -> Self {
        let mut nodes = NodeTape::with_capacity(node_capacity);
        nodes.push(NodeRecordInit {
            kind: NodeKind::Document,
            ns: Namespace::HTML,
            parent: NO_NODE,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            subtree_end: NO_NODE,
            span: 0..0,
            tag_token: NO_TAG,
            outer_end: input_len,
            raw_text: false,
            doctype: false,
            normalize_newlines: false,
        });
        let mut stack = Vec::with_capacity(64);
        stack.push(DOC_ROOT);
        // Only open elements need a "last child" cursor.  Keeping these
        // parallel to the open stack (instead of one slot per node) avoids two
        // O(nodes) vector pushes and random writes for every text node.
        let mut last_child = Vec::with_capacity(64);
        last_child.push(NO_NODE);
        Self {
            nodes,
            stack,
            last_child,
            tag_element: vec![NO_NODE; tag_count],
            implied_tags: Vec::new(),
            implied_literals: Vec::new(),
            source_tag_count: tag_count,
            extra_tag_elements: Vec::new(),
        }
    }

    fn start_tag(&mut self, tag_token: TagId, span: Range<u32>, ns: Namespace) -> NodeId {
        let id = self.insert_element(tag_token, span, ns);
        self.tag_element[tag_token.index()] = id;
        id
    }

    fn insert_implied_element(
        &mut self,
        name: &'static str,
        span: Range<u32>,
        ns: Namespace,
    ) -> NodeId {
        let tag_token = TagId::new((self.source_tag_count + self.implied_tags.len()) as u32);
        let id = self.insert_element(tag_token, span.clone(), ns);
        self.implied_tags.push(TagToken {
            span,
            kind: TagKind::Start,
            attr_range: 0..0,
            self_closing: false,
        });
        self.tag_element.push(id);
        self.implied_literals.push(Some(name));
        id
    }

    /// Pops the current node off the stack of open elements: ends arrive in
    /// stack order, so there is no lookup or search.
    fn pop_open_element(&mut self, span: Range<u32>, implied: bool) {
        let Some(id) = self.stack.pop() else {
            debug_assert!(false, "tree construction emitted an unbalanced end");
            return;
        };
        debug_assert!(
            id != DOC_ROOT,
            "tree construction must not close the Document node"
        );
        let subtree_end = NodeId::new(self.nodes.len() as u32);
        self.nodes.set_subtree_end(id, subtree_end);
        // An element's `outer_end` is only written here, once, so there is
        // nothing to keep the maximum with: reading the old value was a random
        // access into the tape for a node far behind the write head.
        self.nodes
            .set_outer_end(id, if implied { span.start } else { span.end });
        self.last_child.pop();
    }

    fn insert_element(&mut self, tag_token: TagId, span: Range<u32>, ns: Namespace) -> NodeId {
        let parent = *self.stack.last().expect("root is always open");
        let id = self.nodes.push(NodeRecordInit {
            kind: NodeKind::Element,
            ns,
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            subtree_end: NO_NODE,
            span,
            tag_token,
            outer_end: 0,
            raw_text: false,
            doctype: false,
            normalize_newlines: false,
        });
        self.append_child(parent, id);
        self.stack.push(id);
        self.last_child.push(NO_NODE);
        id
    }

    /// Appends a comment or doctype node at the current position.
    fn insert_comment(&mut self, span: Range<u32>, is_doctype: bool) {
        let parent = *self.stack.last().expect("root is always open");
        let id = NodeId::new(self.nodes.len() as u32);
        self.nodes.push(NodeRecordInit {
            kind: NodeKind::Comment,
            doctype: is_doctype,
            normalize_newlines: false,
            ns: Namespace::HTML,
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            subtree_end: NodeId::new(id.get() + 1),
            span: span.clone(),
            tag_token: NO_TAG,
            outer_end: span.end,
            raw_text: false,
        });
        self.append_child(parent, id);
    }

    /// Appends a text node.
    fn insert_characters(&mut self, run: TextRun) {
        let parent = *self.stack.last().expect("root is always open");
        let id = NodeId::new(self.nodes.len() as u32);
        let span = run.span;
        let raw = run.raw_text;
        self.nodes.push(NodeRecordInit {
            kind: NodeKind::Text,
            ns: Namespace::HTML,
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            subtree_end: NodeId::new(id.get() + 1),
            span: span.clone(),
            tag_token: NO_TAG,
            outer_end: span.end,
            raw_text: raw,
            doctype: false,
            normalize_newlines: false,
        });
        self.append_child(parent, id);
    }

    fn append_child(&mut self, parent: NodeId, child: NodeId) {
        debug_assert_eq!(self.stack.last().copied(), Some(parent));
        let frame = self.stack.len() - 1;
        let previous_last = self.last_child[frame];
        if previous_last == NO_NODE {
            self.nodes.set_first_child(parent, child);
        } else {
            self.nodes.set_next_sibling(previous_last, child);
        }
        self.last_child[frame] = child;
    }

    fn finish(&mut self, input_len: u32) {
        let subtree_end = NodeId::new(self.nodes.len() as u32);
        for &node in self.stack.iter().rev() {
            if self.nodes.subtree_end_of(node) == NO_NODE {
                self.nodes.set_subtree_end(node, subtree_end);
            }
            let outer_end = self.nodes.outer_end_of(node).max(input_len);
            self.nodes.set_outer_end(node, outer_end);
        }
        self.nodes.set_outer_end(DOC_ROOT, input_len);
        if self.nodes.subtree_end_of(DOC_ROOT) == NO_NODE {
            self.nodes.set_subtree_end(DOC_ROOT, subtree_end);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parse::ParseOptions;

    use super::*;

    /// Validates the preorder tape invariants that everything downstream
    /// (selector windows, `subtree_end` ranges, text runs) relies on.
    fn assert_preorder(doc: &crate::document::Document<'_>) {
        let n = doc.nodes.len();
        assert!(n > 0);
        for raw in 0..n {
            let id = NodeId::new(raw as u32);
            let node = doc.node(id);
            if raw == 0 {
                assert_eq!(node.parent(), NO_NODE);
                continue;
            }
            assert!(
                node.parent() < id,
                "node {id}: parent {:?} >= id",
                node.parent()
            );
            assert!(
                node.subtree_end() > id,
                "node {id}: subtree_end {} <= id",
                node.subtree_end()
            );
            assert!(
                node.subtree_end() <= doc.node(node.parent()).subtree_end(),
                "node {id}: subtree_end {} escapes parent {}",
                node.subtree_end(),
                node.parent()
            );
            // Children are contiguous and in id order.
            let mut child = node.first_child();
            let mut last = None;
            while child != NO_NODE {
                assert_eq!(doc.node(child).parent(), id);
                assert!(child > id && child < node.subtree_end());
                if let Some(last) = last {
                    assert!(child > last, "node {id}: children out of order");
                }
                last = Some(child);
                child = doc.node(child).next_sibling();
            }
        }
    }

    #[test]
    fn unexpected_end_is_flagged() {
        let doc = crate::document::Document::parse(b"</div>").unwrap();
        assert!(doc.flags.contains(DocumentFlags::UNEXPECTED_END_TAG));
        assert_eq!(doc.diagnostics.len(), 1);
        assert_eq!(doc.diagnostics[0].kind, DiagnosticKind::UnexpectedEndTag);
    }

    #[test]
    fn markup_eof_flags_map_one_to_one() {
        let doc = crate::document::Document::parse(b"<!-- x").unwrap();
        assert!(doc.flags.contains(DocumentFlags::EOF_IN_COMMENT));
        assert!(!doc.flags.contains(DocumentFlags::NEEDS_FALLBACK));

        let doc = crate::document::Document::parse(b"<![CDATA[x").unwrap();
        assert!(doc.flags.contains(DocumentFlags::EOF_IN_CDATA));
        assert!(doc.flags.contains(DocumentFlags::NEEDS_FALLBACK));
    }

    /// `RepairMode::None` keeps the tape well-formed but does not repair.
    #[test]
    fn repair_mode_none_is_flat_but_well_formed() {
        let cases = [
            "<div><p>a</p></div>",
            "<p>a<p>b",
            "<ul><li>a<li>b</ul>",
            "<table><tr><td>x</td></tr></table>",
            "<div></span></div>",
            "<br><img><hr>",
            "<dl><dt>a<dd>b</dl>",
            "",
            "<a><b><c>",
        ];
        for input in cases {
            let none = crate::document::Document::parse(input.as_bytes()).unwrap();
            let spec = crate::document::Document::parse_with(
                input.as_bytes(),
                ParseOptions {
                    repair: RepairMode::Spec,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_preorder(&none);
            assert_preorder(&spec);
        }

        fn nth(doc: &crate::document::Document<'_>, name: &str, n: usize) -> NodeId {
            doc.nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| node.is_element() && doc.tag_name(node.tag_token()) == name)
                .nth(n)
                .map(|(id, _)| NodeId::new(id as u32))
                .expect("element")
        }

        // Unrepaired `<p>a<p>b` nests the second `<p>`; `Spec` makes them siblings.
        let input = "<p>a<p>b";
        let none = crate::document::Document::parse(input.as_bytes()).unwrap();
        let spec = crate::document::Document::parse_with(
            input.as_bytes(),
            ParseOptions {
                repair: RepairMode::Spec,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            none.node(nth(&none, "p", 1)).parent(),
            nth(&none, "p", 0),
            "no repair: nested"
        );
        assert_ne!(
            spec.node(nth(&spec, "p", 1)).parent(),
            nth(&spec, "p", 0),
            "repaired: siblings"
        );

        // No implied `tbody` without repair.
        let input = "<table><tr><td>x</td></tr></table>";
        let none = crate::document::Document::parse(input.as_bytes()).unwrap();
        let spec = crate::document::Document::parse_with(
            input.as_bytes(),
            ParseOptions {
                repair: RepairMode::Spec,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(none.tags.implied_names.is_empty());
        assert!(spec.tags.implied_names.contains(&"tbody"));
    }

    #[test]
    fn repair_mode_none_never_panics_on_random_bytes() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..2048 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let len = (state as usize) & 0x1FF;
            let alphabet = b"<>/!\"' =-\n\tabcdepivrtlsu0123";
            let mut input = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                input.push(alphabet[(state as usize) % alphabet.len()]);
            }
            let doc = crate::document::Document::parse(&input).unwrap();
            assert_preorder(&doc);
        }
    }
}
