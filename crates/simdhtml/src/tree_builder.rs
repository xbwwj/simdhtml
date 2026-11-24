use std::ops::Range;

use crate::RepairMode;
use crate::document::{MAX_DEPTH, NodeRecordInit, NodeTape};

mod spec;
use crate::markup::{MarkupIndex, TextRun, TextRuns};
use crate::tokenizer::Tokens;
use crate::{
    Document, DocumentFlags, MarkupDiagnostics, NO_NODE, NO_TAG, NodeId, NodeKind, TagKind,
    TagToken,
};
/// SVG names the specification adjusts to camelCase; the 1-based index is what
/// [`crate::Document::tag_name_override`] stores.
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
pub(crate) fn adjusted_attr_index(name: &[u8], ns: u8) -> Option<u8> {
    let (table, offset): (&[(&str, &str)], usize) = if ns == crate::NS_MATHML {
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
fn raw_mode_namespace(name: &[u8]) -> u8 {
    if name.eq_ignore_ascii_case(b"svg") {
        crate::NS_SVG
    } else if name.eq_ignore_ascii_case(b"math") {
        crate::NS_MATHML
    } else {
        crate::NS_HTML
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

    let (normal_flags, warnings, tag_name_override, merges) =
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
        let ns = builder.node_tape.namespace_of(element);
        if ns != crate::NS_SVG && ns != crate::NS_MATHML {
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
        let mut all: Vec<u32> = dest_range.collect();
        all.extend(missing);
        for attr in all {
            tokens.attr_tag.push(dest);
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

    let mut flags = normal_flags | map_markup_flags(index.diags);
    if !warnings.is_empty() {
        flags |= DocumentFlags::UNEXPECTED_END_TAG;
    }
    builder.finish(input_len, &mut flags);
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
    Document {
        input,
        encoding: encoding_rs::UTF_8,
        had_encoding_errors: false,
        node_tape: builder.node_tape,
        tags: tokens.tags,
        tag_element,
        tag_name_start: tokens.tag_name_start,
        tag_name_len: tokens.tag_name_len,
        tag_code,
        source_tag_count: source_tag_count as u32,
        implied_names: builder
            .implied_literals
            .into_iter()
            .map(|name| name.expect("implied name"))
            .collect(),
        attr_tag: tokens.attr_tag,
        attr_name_start: tokens.attr_name_start,
        attr_name_len: tokens.attr_name_len,
        attr_value_start: tokens.attr_value_start,
        attr_value_len: tokens.attr_value_len,
        attr_quote: tokens.attr_quote,
        attr_name_override,
        tag_name_override,
        flags,
        warnings,
    }
}

fn map_markup_flags(flags: MarkupDiagnostics) -> DocumentFlags {
    let mut out = DocumentFlags::empty();
    if flags.contains(MarkupDiagnostics::UNBALANCED_TAG) {
        out |= DocumentFlags::UNBALANCED_TAG;
    }
    if flags.intersects(
        MarkupDiagnostics::NEEDS_FALLBACK
            | MarkupDiagnostics::EOF_IN_TAG
            | MarkupDiagnostics::EOF_IN_COMMENT
            | MarkupDiagnostics::EOF_IN_DOCTYPE
            | MarkupDiagnostics::EOF_IN_RAW_TEXT,
    ) {
        out |= DocumentFlags::NEEDS_FALLBACK;
    }
    out
}

struct TreeBuilder {
    node_tape: NodeTape,
    stack: Vec<NodeId>,
    last_child: Vec<NodeId>,
    tag_element: Vec<NodeId>,
    implied_tags: Vec<TagToken>,
    implied_literals: Vec<Option<&'static str>>,
    source_tag_count: usize,
    depth_limit_hit: bool,
}

impl TreeBuilder {
    fn new(input_len: u32, node_capacity: usize, tag_count: usize) -> Self {
        let mut node_tape = NodeTape::with_capacity(node_capacity);
        node_tape.push(NodeRecordInit {
            kind: NodeKind::Document,
            ns: crate::NS_HTML,
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
        stack.push(0);
        // Only open elements need a "last child" cursor.  Keeping these
        // parallel to the open stack (instead of one slot per node) avoids two
        // O(nodes) vector pushes and random writes for every text node.
        let mut last_child = Vec::with_capacity(64);
        last_child.push(NO_NODE);
        Self {
            node_tape,
            stack,
            last_child,
            tag_element: vec![NO_NODE; tag_count],
            implied_tags: Vec::new(),
            implied_literals: Vec::new(),
            source_tag_count: tag_count,
            depth_limit_hit: false,
        }
    }

    fn start_tag(&mut self, tag_token: u32, span: Range<u32>, ns: u8) {
        let id = self.insert_element(tag_token, span, ns);
        self.tag_element[tag_token as usize] = id;
    }

    fn insert_implied_element(&mut self, name: &'static str, span: Range<u32>, ns: u8) {
        let tag_token = (self.source_tag_count + self.implied_tags.len()) as u32;
        let id = self.insert_element(tag_token, span.clone(), ns);
        self.implied_tags.push(TagToken {
            span,
            kind: TagKind::Start,
            attr_range: 0..0,
            self_closing: false,
        });
        self.tag_element.push(id);
        self.implied_literals.push(Some(name));
    }

    /// Pops the current node off the stack of open elements: ends arrive in
    /// stack order, so there is no lookup or search.
    fn pop_open_element(&mut self, span: Range<u32>, implied: bool) {
        let Some(id) = self.stack.pop() else {
            debug_assert!(false, "tree construction emitted an unbalanced end");
            return;
        };
        debug_assert!(
            id != 0,
            "tree construction must not close the Document node"
        );
        let subtree_end = self.node_tape.len() as NodeId;
        self.node_tape.set_subtree_end(id, subtree_end);
        // An element's `outer_end` is only written here, once, so there is
        // nothing to keep the maximum with: reading the old value was a random
        // access into the tape for a node far behind the write head.
        self.node_tape
            .set_outer_end(id, if implied { span.start } else { span.end });
        self.last_child.pop();
    }

    fn insert_element(&mut self, tag_token: u32, span: Range<u32>, ns: u8) -> NodeId {
        let parent = *self.stack.last().expect("root is always open");
        if self.stack.len() > MAX_DEPTH as usize {
            self.depth_limit_hit = true;
        }
        let id = self.node_tape.push(NodeRecordInit {
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
        let id = self.node_tape.len() as NodeId;
        self.node_tape.push(NodeRecordInit {
            kind: NodeKind::Comment,
            doctype: is_doctype,
            normalize_newlines: false,
            ns: 0,
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            subtree_end: id + 1,
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
        let id = self.node_tape.len() as NodeId;
        let span = run.span;
        let raw = run.raw_text;
        self.node_tape.push(NodeRecordInit {
            kind: NodeKind::Text,
            ns: crate::NS_HTML,
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            subtree_end: id + 1,
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
            self.node_tape.set_first_child(parent, child);
        } else {
            self.node_tape.set_next_sibling(previous_last, child);
        }
        self.last_child[frame] = child;
    }

    fn finish(&mut self, input_len: u32, flags: &mut DocumentFlags) {
        let subtree_end = self.node_tape.len() as NodeId;
        for &node in self.stack.iter().rev() {
            if self.node_tape.subtree_end_of(node) == NO_NODE {
                self.node_tape.set_subtree_end(node, subtree_end);
            }
            let outer_end = self.node_tape.outer_end_of(node).max(input_len);
            self.node_tape.set_outer_end(node, outer_end);
        }
        self.node_tape.set_outer_end(0, input_len);
        if self.node_tape.subtree_end_of(0) == NO_NODE {
            self.node_tape.set_subtree_end(0, subtree_end);
        }
        if self.depth_limit_hit {
            *flags |= DocumentFlags::DEPTH_LIMIT;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ParseOptions;

    use super::*;

    /// Validates the preorder tape invariants that everything downstream
    /// (selector windows, `subtree_end` ranges, text runs) relies on.
    fn assert_preorder(doc: &crate::Document<'_>) {
        let n = doc.node_tape.len() as u32;
        assert!(n > 0);
        for id in 0..n {
            let node = doc.node(id);
            if id == 0 {
                assert_eq!(node.parent(), crate::NO_NODE);
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
            while child != crate::NO_NODE {
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
        let doc = crate::Document::parse(b"</div>").unwrap();
        assert!(doc.flags.contains(DocumentFlags::UNEXPECTED_END_TAG));
        assert_eq!(doc.warnings.len(), 1);
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
            let none = crate::Document::parse(input.as_bytes()).unwrap();
            let spec = crate::Document::parse_with(
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

        fn nth(doc: &crate::Document<'_>, name: &str, n: usize) -> crate::NodeId {
            doc.node_tape
                .iter()
                .enumerate()
                .filter(|(_, node)| node.is_element() && doc.tag_name(node.tag_token()) == name)
                .nth(n)
                .map(|(id, _)| id as crate::NodeId)
                .expect("element")
        }

        // Unrepaired `<p>a<p>b` nests the second `<p>`; `Spec` makes them siblings.
        let input = "<p>a<p>b";
        let none = crate::Document::parse(input.as_bytes()).unwrap();
        let spec = crate::Document::parse_with(
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
        let none = crate::Document::parse(input.as_bytes()).unwrap();
        let spec = crate::Document::parse_with(
            input.as_bytes(),
            ParseOptions {
                repair: RepairMode::Spec,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(none.implied_names.is_empty());
        assert!(spec.implied_names.contains(&"tbody"));
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
            let doc = crate::Document::parse(&input).unwrap();
            assert_preorder(&doc);
        }
    }
}
