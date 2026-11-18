//! Builds the preorder [`FlatDoc`] from normalized events.

use std::ops::Range;

use crate::{
    DocFlags, FlatDoc, MarkupFlags, NO_NODE, NO_TAG, Node, NodeId, NodeKind, TagKind, TagToken,
};

use crate::normalize::{EventSink, TagRef, normalize_with};
use crate::tokenizer::{TextRun, TextRuns, Tokenized};

const DEFAULT_DEPTH_LIMIT: u16 = 1024;

pub(crate) fn build<'a>(
    input: &'a [u8],
    index: &crate::MarkupIndex,
    mut tokens: Tokenized,
) -> FlatDoc<'a> {
    let input_len = input.len() as u32;
    let node_capacity = tokens.tags.len() + index.text_runs.len() + 1;
    let mut builder = Builder::new(input_len, node_capacity, tokens.tags.len());

    let text_runs = TextRuns::new(input, index);
    // Normalization and node construction are one pass: the normalizer drives
    // the builder directly instead of materializing an event vector.
    let (normal_flags, warnings) =
        normalize_with(input, &tokens, input_len, text_runs, &mut builder);

    let mut flags = normal_flags | map_markup_flags(index.flags);
    builder.finish(input_len, &mut flags);
    for (i, &node) in builder.tag_node.iter().enumerate() {
        tokens.tags[i].node = node;
    }

    let base_tags = tokens.tags.len();
    tokens.tags.extend(builder.synthetic_tags);
    for _ in 0..builder.synthetic_literals.len() {
        tokens.tag_name_start.push(0);
        tokens.tag_name_len.push(0);
    }
    FlatDoc {
        input,
        nodes: builder.nodes,
        tags: tokens.tags,
        tag_name_start: tokens.tag_name_start,
        tag_name_len: tokens.tag_name_len,
        base_tag_count: base_tags as u32,
        synthetic_names: builder
            .synthetic_literals
            .into_iter()
            .map(|name| name.expect("synthetic name"))
            .collect(),
        attr_tag: tokens.attr_tag,
        attr_key_start: tokens.attr_key_start,
        attr_key_len: tokens.attr_key_len,
        attr_value_start: tokens.attr_value_start,
        attr_value_len: tokens.attr_value_len,
        attr_quote: tokens.attr_quote,
        root: 0,
        flags,
        warnings,
    }
}

fn map_markup_flags(flags: MarkupFlags) -> DocFlags {
    let mut out = DocFlags::empty();
    if flags.contains(MarkupFlags::UNBALANCED_TAG) {
        out |= DocFlags::UNBALANCED_TAG;
    }
    if flags.intersects(
        MarkupFlags::NEEDS_FALLBACK
            | MarkupFlags::UNCLOSED_TAG
            | MarkupFlags::UNCLOSED_COMMENT
            | MarkupFlags::UNCLOSED_DECLARATION
            | MarkupFlags::UNCLOSED_RAWTEXT,
    ) {
        out |= DocFlags::NEEDS_FALLBACK;
    }
    out
}

impl EventSink for Builder {
    fn start(&mut self, token: u32, span: Range<u32>) {
        self.start_tag(token, span);
    }

    fn start_synthetic(&mut self, name: &'static str, span: Range<u32>) {
        self.start_synthetic_tag(name, span);
    }

    fn end(&mut self, _tag: TagRef, span: Range<u32>, synthetic: bool) {
        self.close_top(span, synthetic);
    }

    fn text(&mut self, run: TextRun) {
        Builder::text(self, run);
    }
}

struct Builder {
    nodes: Vec<Node>,
    stack: Vec<NodeId>,
    last_child: Vec<NodeId>,
    last_element_child: Vec<NodeId>,
    tag_node: Vec<NodeId>,
    synthetic_tags: Vec<TagToken>,
    synthetic_literals: Vec<Option<&'static str>>,
    base_tag_count: usize,
    depth_limit_hit: bool,
}

impl Builder {
    fn new(input_len: u32, node_capacity: usize, tag_count: usize) -> Self {
        let root = Node::root(input_len);
        let mut nodes = Vec::with_capacity(node_capacity);
        nodes.push(root);
        let mut stack = Vec::with_capacity(64);
        stack.push(0);
        // Only open elements need a "last child" cursor.  Keeping these
        // parallel to the open stack (instead of one slot per node) avoids two
        // O(nodes) vector pushes and random writes for every text node.
        let mut last_child = Vec::with_capacity(64);
        last_child.push(NO_NODE);
        let mut last_element_child = Vec::with_capacity(64);
        last_element_child.push(NO_NODE);
        Self {
            nodes,
            stack,
            last_child,
            last_element_child,
            tag_node: vec![NO_NODE; tag_count],
            synthetic_tags: Vec::new(),
            synthetic_literals: Vec::new(),
            base_tag_count: tag_count,
            depth_limit_hit: false,
        }
    }

    fn start_tag(&mut self, tag_token: u32, span: Range<u32>) {
        let id = self.element(tag_token, span);
        self.tag_node[tag_token as usize] = id;
    }

    fn start_synthetic_tag(&mut self, name: &'static str, span: Range<u32>) {
        let tag_token = (self.base_tag_count + self.synthetic_tags.len()) as u32;
        let id = self.element(tag_token, span.clone());
        self.synthetic_tags.push(TagToken {
            span,
            kind: TagKind::Start,
            attrs: 0..0,
            node: id,
            self_closing: false,
        });
        self.synthetic_literals.push(Some(name));
    }

    /// Closes the current top element.  The normalizer already emits ends in
    /// stack order, so no name lookup or stack search is needed here.
    fn close_top(&mut self, span: Range<u32>, synthetic: bool) {
        let Some(id) = self.stack.pop() else {
            debug_assert!(false, "normalizer emitted an unbalanced end");
            return;
        };
        debug_assert!(id != 0, "normalizer must not close the document root");
        let subtree_end = self.nodes.len() as NodeId;
        let node = &mut self.nodes[id as usize];
        node.subtree_end = subtree_end;
        node.outer_end = if synthetic {
            node.outer_end.max(span.start)
        } else {
            node.outer_end.max(span.end)
        };
        self.last_child.pop();
        self.last_element_child.pop();
    }

    fn element(&mut self, tag_token: u32, span: Range<u32>) -> NodeId {
        let parent = *self.stack.last().expect("root is always open");
        let depth = self.nodes[parent as usize].depth.saturating_add(1);
        let id = self.nodes.len() as NodeId;
        let mut node = Node {
            kind: NodeKind::Element,
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            prev_element_sibling: NO_NODE,
            subtree_end: NO_NODE,
            depth,
            span: span.clone(),
            tag_token,
            outer_end: span.end,
            raw: false,
        };
        if depth > DEFAULT_DEPTH_LIMIT {
            node.depth = DEFAULT_DEPTH_LIMIT;
            self.depth_limit_hit = true;
        }
        self.nodes.push(node);
        self.link_child(parent, id, true);
        self.stack.push(id);
        self.last_child.push(NO_NODE);
        self.last_element_child.push(NO_NODE);
        id
    }

    fn text(&mut self, run: TextRun) {
        let parent = *self.stack.last().expect("root is always open");
        let depth = self.nodes[parent as usize].depth.saturating_add(1);
        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            kind: NodeKind::Text,
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            prev_element_sibling: NO_NODE,
            subtree_end: id + 1,
            depth,
            span: run.span.clone(),
            tag_token: NO_TAG,
            outer_end: run.span.end,
            raw: run.raw,
        });
        self.link_child(parent, id, false);
    }

    fn link_child(&mut self, parent: NodeId, child: NodeId, is_element: bool) {
        debug_assert_eq!(self.stack.last().copied(), Some(parent));
        let frame = self.stack.len() - 1;
        let previous_last = self.last_child[frame];
        if previous_last == NO_NODE {
            self.nodes[parent as usize].first_child = child;
        } else {
            self.nodes[previous_last as usize].next_sibling = child;
        }
        self.last_child[frame] = child;

        if is_element {
            let previous_element = self.last_element_child[frame];
            if previous_element != NO_NODE {
                self.nodes[child as usize].prev_element_sibling = previous_element;
            }
            self.last_element_child[frame] = child;
        }
    }

    fn finish(&mut self, input_len: u32, flags: &mut DocFlags) {
        let subtree_end = self.nodes.len() as NodeId;
        for &node in self.stack.iter().rev() {
            let n = &mut self.nodes[node as usize];
            if n.subtree_end == NO_NODE {
                n.subtree_end = subtree_end;
            }
            n.outer_end = n.outer_end.max(input_len);
        }
        self.nodes[0].outer_end = input_len;
        if self.nodes[0].subtree_end == NO_NODE {
            self.nodes[0].subtree_end = subtree_end;
        }
        if self.depth_limit_hit {
            *flags |= DocFlags::DEPTH_LIMIT;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_builder_compiles() {
        let b = Builder::new(0, 1, 0);
        assert_eq!(b.nodes.len(), 1);
    }
}
