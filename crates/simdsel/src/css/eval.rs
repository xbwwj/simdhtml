use std::cell::OnceCell;

use simdhtml::document::Document;
use simdhtml::document::id::{DOC_ROOT, NO_NODE, NO_TAG, NodeId};
use simdhtml::document::node::NodeKind;

use crate::bitset::BitSet;
use crate::error::{CssError, Error, Result};
use crate::record::{MatchKind, MatchRecord};

use super::ast::{
    AttrOp, AttrPred, Combinator, Complex, Compound, Pseudo, PseudoElement, RelativeSelector,
    SelectorList,
};
use super::cache::{ChildIndex, QueryCache};

const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy)]
enum OfType {
    First,
    Last,
    Nth(i32, i32),
    NthLast(i32, i32),
}

impl OfType {
    fn keep(self, at: u32, total: u32) -> bool {
        match self {
            Self::First => at == 1,
            Self::Last => at == total,
            Self::Nth(a, b) => nth(at, a, b),
            Self::NthLast(a, b) => nth_i64(total as i64 - at as i64 + 1, a, b),
        }
    }
}

/// One sibling-position pseudo, evaluated against `(index, total)`.
#[derive(Clone, Copy, Debug)]
enum Position {
    FirstChild,
    LastChild,
    OnlyChild,
    Nth(i32, i32),
    NthLast(i32, i32),
}

impl Position {
    fn keep(self, at: u32, total: u32) -> bool {
        match self {
            Self::FirstChild => at == 1,
            Self::LastChild => at == total,
            Self::OnlyChild => total == 1,
            Self::Nth(a, b) => nth(at, a, b),
            Self::NthLast(a, b) => nth_i64(total as i64 - at as i64 + 1, a, b),
        }
    }
}

/// Evaluation context borrowing a document and its progressive cache.
pub(super) struct Eval<'a> {
    doc: &'a Document<'a>,
    n: usize,
    cache: &'a QueryCache,
    scope: OnceCell<BitSet>,
    root_element: Option<NodeId>,
}

impl<'a> Eval<'a> {
    pub(super) fn new(doc: &'a Document<'_>, cache: &'a QueryCache) -> Self {
        let n = doc.nodes.len();

        let mut root_element = None;
        let mut child = doc.node(DOC_ROOT).first_child();
        while child != NO_NODE {
            let node = doc.node(child);
            if node.is_element() {
                root_element = Some(child);
                break;
            }
            child = node.next_sibling();
        }

        Self {
            doc,
            n,
            cache,
            scope: OnceCell::new(),
            root_element,
        }
    }

    fn child_index(&self) -> &ChildIndex {
        self.cache.child_index(self.doc, self.n)
    }

    /// Computes the selector mask and applies an optional root-subtree
    /// restriction.  The document root needs no restriction mask.
    fn scope_mask(&self) -> &BitSet {
        self.scope.get_or_init(|| {
            let mut mask = BitSet::new(self.n);
            if let Some(root) = self.root_element {
                mask.set(root);
            }
            mask
        })
    }

    fn set_scope(&self, roots: Option<&[NodeId]>) {
        let mut mask = BitSet::new(self.n);
        match roots {
            None => {
                if let Some(root) = self.root_element {
                    mask.set(root);
                }
            }
            Some(roots) => {
                for &root in roots {
                    let node = self.doc.node(root);
                    if node.is_element() {
                        mask.set(root);
                    } else if node.kind() == NodeKind::Document
                        && let Some(root_element) = self.root_element
                    {
                        mask.set(root_element);
                    }
                }
            }
        }
        let _ = self.scope.set(mask);
    }

    pub(super) fn mask_for(
        &self,
        selector: &SelectorList,
        roots: Option<&[NodeId]>,
    ) -> Result<BitSet> {
        self.set_scope(roots);
        let mut mask = self.selector_mask(selector, 0)?;
        if let Some(roots) = roots
            && !(roots.len() == 1 && roots[0] == DOC_ROOT)
        {
            let include_scope =
                selector_has_scope(selector) || selector_starts_with_implied_universal(selector);
            let mut allowed = BitSet::new(self.n);
            for &root in roots {
                let node = self.doc.node(root);
                if node.kind() == NodeKind::Document {
                    for id in 1..self.n as u32 {
                        allowed.set(NodeId::new(id));
                    }
                } else if node.is_element() {
                    // Chained selectors search descendants, not the root itself.
                    if include_scope {
                        allowed.set(root);
                    }
                    for id in
                        root.get() + 1..node.subtree_end().min(NodeId::new(self.n as u32)).get()
                    {
                        let id = NodeId::new(id);
                        allowed.set(id);
                    }
                } else if node.parent() != NO_NODE {
                    let parent = self.doc.node(node.parent());
                    if parent.is_element() {
                        for id in node.parent().get() + 1
                            ..parent.subtree_end().min(NodeId::new(self.n as u32)).get()
                        {
                            let id = NodeId::new(id);
                            allowed.set(id);
                        }
                    }
                }
            }
            mask.and_assign(&allowed);
        }
        Ok(mask)
    }

    /// Turns an element mask into records, applying a trailing pseudo-element.
    pub(super) fn records_for(&self, scope: &SelectorList, mask: &BitSet) -> Vec<MatchRecord> {
        let mut out = Vec::new();
        for id in mask.iter_set() {
            out.push(element_record(self.doc, id));
        }

        // Pseudo-elements are a projection of the element result set.  Keep
        // the element records for plain selectors and expand only when the
        // selector text asked for one.
        if let Some(pseudo) = common_pseudo(scope) {
            let mut projected = Vec::new();
            for record in out {
                self.project(record, &pseudo, &mut projected);
            }
            // Projection walks elements, so nested elements group their text
            // children together.  Results are a node set: document order, i.e.
            // ascending preorder id (`::attr` keeps the element id and is
            // already sorted).
            projected.sort_unstable_by_key(|record| record.node);
            projected
        } else {
            out
        }
    }

    pub(super) fn select(
        &self,
        selector: &SelectorList,
        roots: Option<&[NodeId]>,
    ) -> Result<Vec<MatchRecord>> {
        let mask = self.mask_for(selector, roots)?;
        Ok(self.records_for(selector, &mask))
    }

    pub(super) fn count(&self, selector: &SelectorList, roots: Option<&[NodeId]>) -> Result<usize> {
        let mask = self.mask_for(selector, roots)?;
        if common_pseudo(selector).is_some() {
            Ok(self.records_for(selector, &mask).len())
        } else {
            Ok(mask.count_ones() as usize)
        }
    }

    fn selector_mask(&self, selector: &SelectorList, depth: usize) -> Result<BitSet> {
        if depth > MAX_DEPTH {
            return Err(Error::Css(CssError::DepthLimit));
        }
        let mut out = BitSet::new(self.n);
        for complex in &selector.complexes {
            let mask = self.complex_mask(complex, depth)?;
            out.or_assign(&mask);
        }
        Ok(out)
    }

    /// Evaluates a complex selector left-to-right: the final bitset equals the
    /// rightmost compound's matches, without per-element ancestor walks.
    fn complex_mask(&self, complex: &Complex, depth: usize) -> Result<BitSet> {
        if depth > MAX_DEPTH {
            return Err(Error::Css(CssError::DepthLimit));
        }
        // No combinator: the compound mask is the answer.
        if complex.combinators.is_empty() {
            return self.compound_mask(&complex.compounds[0], depth);
        }
        let mut current: Vec<NodeId> = self
            .compound_mask(&complex.compounds[0], depth)?
            .iter_set()
            .collect();
        for (i, combinator) in complex.combinators.iter().enumerate() {
            // A pseudo-element with no compound of its own (`A ::text`) hangs
            // off an implied `*`; after a descendant combinator that implied
            // universal is `descendant-or-self`, so A's own text/attributes are
            // included, as in parsel/cssselect.
            let include_self =
                *combinator == Combinator::Descendant && complex.compounds[i + 1].implied_universal;
            let windows = self.relation_windows(&current, *combinator, include_self);
            current =
                self.compound_points_in_windows(&complex.compounds[i + 1], &windows, depth)?;
        }
        let mut mask = BitSet::new(self.n);
        for id in current {
            mask.set(id);
        }
        Ok(mask)
    }

    /// Expands a node set into preorder windows for the next combinator, straight
    /// from `subtree_end` and the child links (no global bitmaps).
    fn relation_windows(
        &self,
        set: &[NodeId],
        combinator: Combinator,
        include_self: bool,
    ) -> Vec<(NodeId, NodeId)> {
        let mut windows = Vec::new();
        match combinator {
            Combinator::Descendant => {
                for &id in set {
                    let end = self
                        .doc
                        .node(id)
                        .subtree_end()
                        .min(NodeId::new(self.n as u32));
                    let start = if include_self { id } else { id + 1 };
                    if start < end {
                        windows.push((start, end));
                    }
                }
            }
            Combinator::Child => {
                for &id in set {
                    let mut child = self.doc.node(id).first_child();
                    while child != NO_NODE {
                        windows.push((child, child + 1));
                        child = self.doc.node(child).next_sibling();
                    }
                }
            }
            Combinator::Adjacent => {
                for &id in set {
                    let mut next = self.doc.node(id).next_sibling();
                    while next != NO_NODE && !self.doc.node(next).is_element() {
                        next = self.doc.node(next).next_sibling();
                    }
                    if next != NO_NODE {
                        windows.push((next, next + 1));
                    }
                }
            }
            Combinator::GeneralSibling => {
                for &id in set {
                    let mut next = self.doc.node(id).next_sibling();
                    while next != NO_NODE {
                        windows.push((next, next + 1));
                        next = self.doc.node(next).next_sibling();
                    }
                }
            }
        }
        windows
    }

    fn compound_points_in_windows(
        &self,
        compound: &Compound,
        windows: &[(NodeId, NodeId)],
        depth: usize,
    ) -> Result<Vec<NodeId>> {
        let candidates = compound
            .tag
            .as_ref()
            .map(|tag| self.tag_mask(tag))
            .or_else(|| self.attr_candidates(compound));
        let skip_tag = compound.tag.is_some();
        let mut points = Vec::new();
        for &(start, end) in windows {
            for id in start.get()..end.get() {
                let id = NodeId::new(id);
                if candidates.as_ref().is_some_and(|c| !c.get(id)) {
                    continue;
                }
                let node = self.doc.node(id);
                if node.is_element() && self.matches_static(id, compound, skip_tag) {
                    points.push(id);
                }
            }
        }
        points.sort_unstable();
        points.dedup();
        if !compound.pseudos.is_empty() {
            let mut mask = BitSet::new(self.n);
            for &id in &points {
                mask.set(id);
            }
            for pseudo in &compound.pseudos {
                self.apply_pseudo(&mut mask, pseudo, compound.tag.as_deref(), depth)?;
            }
            points = mask.iter_set().collect();
        }
        Ok(points)
    }

    fn tag_mask(&self, name: &str) -> BitSet {
        if let Some(mask) = self.cache.get_tag_mask(name) {
            return mask;
        }
        let mut mask = BitSet::new(self.n);
        let code = simdhtml::tag_code::code_of(name.as_bytes());
        if code == 0 {
            // No dedicated code: fall back to folding and comparing name spans.
            key_mask(
                self.doc,
                &self.doc.tags.name_start,
                &self.doc.tags.name_len,
                self.doc.tags.len(),
                name,
                |i| self.doc.tags.element[i],
                &mut mask,
            );
        } else {
            tag_code_mask(self.doc, code, &mut mask);
        }
        self.cache.put_tag_mask(name.to_string(), mask.clone());
        mask
    }

    /// Elements carrying an attribute key: the query-side attribute index, so a
    /// `[data-x]` selector does not re-cut every start tag per candidate.
    fn attr_mask(&self, name: &str) -> BitSet {
        if let Some(mask) = self.cache.get_attr_mask(name) {
            return mask;
        }
        let mut mask = BitSet::new(self.n);
        key_mask(
            self.doc,
            &self.doc.attrs.name_start,
            &self.doc.attrs.name_len,
            self.doc.attrs.name_start.len(),
            name,
            |a| {
                let tag = self.doc.attrs.tag[a].index();
                self.doc.tags.element[tag]
            },
            &mut mask,
        );
        self.cache.put_attr_mask(name.to_string(), mask.clone());
        mask
    }

    /// Attribute-key candidate superset for a compound, or `None`.  `class` is
    /// included: the SIMD key mask (~2 us on a 140 KiB page) beats testing every
    /// candidate (~10 ns each) once 45% of elements carry the key.
    fn attr_candidates(&self, compound: &Compound) -> Option<BitSet> {
        if compound.tag.is_some() {
            return None;
        }
        let mut keys: Vec<&str> = Vec::new();
        if compound.id.is_some() {
            keys.push("id");
        }
        if !compound.classes.is_empty() {
            keys.push("class");
        }
        for pred in &compound.attrs {
            keys.push(pred.name.as_str());
        }
        if keys.is_empty() {
            return None;
        }
        keys.sort_unstable();
        keys.dedup();
        let mut candidates: Option<BitSet> = None;
        for key in keys {
            let attr = self.attr_mask(key);
            match &mut candidates {
                Some(current) => {
                    current.and_assign(&attr);
                    if current.is_empty() {
                        break;
                    }
                }
                None => candidates = Some(attr),
            }
        }
        candidates
    }

    /// Evaluates one compound into a bitset: type selectors use the lazy tag
    /// index, attribute keys the query-side index, pseudos filter afterwards.
    fn compound_mask(&self, compound: &Compound, depth: usize) -> Result<BitSet> {
        if depth > MAX_DEPTH {
            return Err(Error::Css(CssError::DepthLimit));
        }
        if compound.pseudos.is_empty()
            && let Some(mask) = self.cache.get_compound(compound)
        {
            return Ok(mask);
        }
        let mut mask = BitSet::new(self.n);

        if let Some(tag) = &compound.tag {
            for id in self.tag_mask(tag).iter_set() {
                if self.matches_static(id, compound, true) {
                    mask.set(id);
                }
            }
        } else if let Some(candidates) = self.attr_candidates(compound) {
            for id in candidates.iter_set() {
                if self.matches_static(id, compound, false) {
                    mask.set(id);
                }
            }
        } else {
            for id in 0..self.n {
                if !self.doc.node(NodeId::new(id as u32)).is_element() {
                    continue;
                }
                if self.matches_static(NodeId::new(id as u32), compound, false) {
                    mask.set(NodeId::new(id as u32));
                }
            }
        }

        for pseudo in &compound.pseudos {
            self.apply_pseudo(&mut mask, pseudo, compound.tag.as_deref(), depth)?;
        }
        if compound.pseudos.is_empty() {
            self.cache.put_compound(compound.clone(), mask.clone());
        }
        Ok(mask)
    }

    fn matches_static(&self, id: NodeId, compound: &Compound, skip_tag: bool) -> bool {
        let node = self.doc.node(id);
        if !skip_tag
            && let Some(tag) = &compound.tag
            && !self
                .doc
                .tag_name(node.tag_token())
                .eq_ignore_ascii_case(tag)
        {
            return false;
        }

        if let Some(id_value) = &compound.id {
            let Some(attr) = crate::dom::find_attr(self.doc, id, "id") else {
                return false;
            };
            let raw = crate::dom::attr_value_raw(self.doc, attr);
            if raw.contains(&b'&') {
                if crate::dom::attr_value(self.doc, attr).as_ref() != id_value.as_str() {
                    return false;
                }
            } else if raw != id_value.as_bytes() {
                return false;
            }
        }

        if !compound.classes.is_empty() {
            let Some(attr) = crate::dom::find_attr(self.doc, id, "class") else {
                return false;
            };
            let raw = crate::dom::attr_value_raw(self.doc, attr);
            if raw.contains(&b'&') {
                // Rare: a class value with character references.
                let value = crate::dom::attr_value(self.doc, attr);
                for class in &compound.classes {
                    if !value.split_ascii_whitespace().any(|c| c == class) {
                        return false;
                    }
                }
            } else {
                for class in &compound.classes {
                    if !raw
                        .split(|byte: &u8| byte.is_ascii_whitespace())
                        .any(|c| c == class.as_bytes())
                    {
                        return false;
                    }
                }
            }
        }

        for pred in &compound.attrs {
            if !matches_attr(self.doc, id, pred) {
                return false;
            }
        }
        true
    }

    /// Case-insensitive tag-name test against source spelling.
    fn is_tag(&self, id: NodeId, name: &[u8]) -> bool {
        let tag = self.doc.node(id).tag_token();
        if tag == NO_TAG {
            return false;
        }
        let actual = self.doc.tag_name(tag);
        actual.len() == name.len() && actual.as_bytes().eq_ignore_ascii_case(name)
    }

    fn has_attr(&self, id: NodeId, name: &str) -> bool {
        crate::dom::find_attr(self.doc, id, name).is_some()
    }

    /// `:link`: `a`, `area` or `link` carrying an `href`.
    fn is_link(&self, id: NodeId) -> bool {
        self.has_attr(id, "href")
            && (self.is_tag(id, b"a") || self.is_tag(id, b"area") || self.is_tag(id, b"link"))
    }

    /// `:checked`: a checked `input`, or a selected `option`.
    fn is_checked(&self, id: NodeId) -> bool {
        (self.is_tag(id, b"input") && self.has_attr(id, "checked"))
            || (self.is_tag(id, b"option") && self.has_attr(id, "selected"))
    }

    /// Elements an HTML `disabled` attribute applies to.
    fn is_disableable(&self, id: NodeId) -> bool {
        self.is_tag(id, b"button")
            || self.is_tag(id, b"fieldset")
            || self.is_tag(id, b"input")
            || self.is_tag(id, b"optgroup")
            || self.is_tag(id, b"option")
            || self.is_tag(id, b"select")
            || self.is_tag(id, b"textarea")
    }

    /// `:disabled`: the `disabled` attribute, or an ancestor `fieldset` with
    /// one (the "first legend child" exception is not modelled).
    fn is_disabled(&self, id: NodeId) -> bool {
        if !self.is_disableable(id) {
            return false;
        }
        if self.has_attr(id, "disabled") {
            return true;
        }
        if self.is_tag(id, b"option")
            && let parent = self.doc.node(id).parent()
            && parent != NO_NODE
            && self.is_tag(parent, b"optgroup")
            && self.has_attr(parent, "disabled")
        {
            return true;
        }
        let mut current = self.doc.node(id).parent();
        while current != NO_NODE {
            let node = self.doc.node(current);
            if !node.is_element() {
                break;
            }
            if self.is_tag(current, b"fieldset") && self.has_attr(current, "disabled") {
                return true;
            }
            current = node.parent();
        }
        false
    }

    /// `:lang(x)`: the nearest ancestor-or-self `lang` is `x` or `x-*`.
    fn lang_matches(&self, id: NodeId, value: &str) -> bool {
        let mut current = id;
        while current != NO_NODE {
            let node = self.doc.node(current);
            if node.is_element()
                && let Some(attr) = crate::dom::find_attr(self.doc, current, "lang")
            {
                let actual = crate::dom::attr_value(self.doc, attr);
                let actual = actual.trim().to_ascii_lowercase();
                return actual == value
                    || (actual.len() > value.len()
                        && actual.starts_with(value)
                        && actual.as_bytes()[value.len()] == b'-');
            }
            current = node.parent();
        }
        false
    }

    fn apply_pseudo(
        &self,
        mask: &mut BitSet,
        pseudo: &Pseudo,
        tag: Option<&str>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Error::Css(CssError::DepthLimit));
        }
        match pseudo {
            Pseudo::FirstChild => self.filter_position(mask, Position::FirstChild),
            Pseudo::LastChild => self.filter_position(mask, Position::LastChild),
            Pseudo::OnlyChild => self.filter_position(mask, Position::OnlyChild),
            Pseudo::NthChild(a, b) => self.filter_position(mask, Position::Nth(*a, *b)),
            Pseudo::NthLastChild(a, b) => self.filter_position(mask, Position::NthLast(*a, *b)),
            Pseudo::FirstOfType => self.filter_of_type(mask, tag, OfType::First),
            Pseudo::LastOfType => self.filter_of_type(mask, tag, OfType::Last),
            Pseudo::NthOfType(a, b) => self.filter_of_type(mask, tag, OfType::Nth(*a, *b)),
            Pseudo::NthLastOfType(a, b) => self.filter_of_type(mask, tag, OfType::NthLast(*a, *b)),
            Pseudo::Not(list) => {
                let nested = self.complex_list_mask(list, depth + 1)?;
                mask.and_not_assign(&nested);
            }
            Pseudo::Is(list) => {
                let nested = self.complex_list_mask(list, depth + 1)?;
                mask.and_assign(&nested);
            }
            Pseudo::Has(relatives) => {
                let mut matches = BitSet::new(self.n);
                for relative in relatives {
                    let nested = self.relative_mask(relative, depth + 1)?;
                    matches.or_assign(&nested);
                }
                mask.and_assign(&matches);
            }
            Pseudo::Root => {
                self.filter(mask, |id| Some(id) == self.root_element);
            }
            Pseudo::Empty => self.filter(mask, |id| self.doc.node(id).first_child() == NO_NODE),
            Pseudo::Scope => {
                let scope = self.scope_mask().clone();
                mask.and_assign(&scope);
            }
            Pseudo::Contains(text) => {
                self.filter(mask, |id| {
                    crate::dom::text_content(self.doc, id).contains(text)
                });
            }
            Pseudo::OnlyOfType => {
                self.filter_of_type(mask, tag, OfType::First);
                self.filter_of_type(mask, tag, OfType::Last);
            }
            Pseudo::Link => self.filter(mask, |id| self.is_link(id)),
            Pseudo::Checked => self.filter(mask, |id| self.is_checked(id)),
            Pseudo::Disabled => self.filter(mask, |id| self.is_disabled(id)),
            Pseudo::Enabled => {
                self.filter(mask, |id| self.is_disableable(id) && !self.is_disabled(id))
            }
            Pseudo::Lang(value) => self.filter(mask, |id| self.lang_matches(id, value)),
            Pseudo::Never => {
                mask.clear_all();
            }
        }
        Ok(())
    }

    fn complex_list_mask(&self, list: &[Complex], depth: usize) -> Result<BitSet> {
        let mut out = BitSet::new(self.n);
        for complex in list {
            out.or_assign(&self.complex_mask(complex, depth)?);
        }
        Ok(out)
    }

    fn relative_mask(&self, relative: &RelativeSelector, depth: usize) -> Result<BitSet> {
        let nested = self.complex_mask(&relative.complex, depth)?;
        Ok(match relative.relation {
            Combinator::Descendant => self.ancestors(&nested),
            Combinator::Child => self.parents(&nested),
            Combinator::Adjacent => self.prev_element_sibling(&nested),
            Combinator::GeneralSibling => self.earlier_element_siblings(&nested),
        })
    }

    /// Candidate count below which the per-candidate sibling walk is tried
    /// before building the child index.
    const SPARSE_CANDIDATES: u32 = 64;

    /// Position of `id` among its parent's element children, charging the walk
    /// against `budget`; `None` means the walk did not fit.
    fn sibling_position_within(
        &self,
        id: NodeId,
        of_type: bool,
        budget: &mut usize,
    ) -> Option<(u32, u32)> {
        let node = self.doc.node(id);
        let parent = node.parent();
        if parent == id || parent.index() >= self.n {
            return Some((0, 0));
        }
        let want = of_type.then(|| {
            let tag = node.tag_token();
            (self.doc.tags.code[tag.index()], self.doc.tag_name(tag))
        });
        let (mut at, mut total) = (0u32, 0u32);
        let mut child = self.doc.node(parent).first_child();
        while child != NO_NODE {
            if *budget == 0 {
                return None;
            }
            *budget -= 1;
            let child_node = self.doc.node(child);
            let hit = child_node.is_element()
                && match &want {
                    None => true,
                    Some((code, name)) => {
                        let tag = child_node.tag_token();
                        if *code == 0 {
                            self.doc.tag_name(tag).eq_ignore_ascii_case(name)
                        } else {
                            self.doc.tags.code[tag.index()] == *code
                        }
                    }
                };
            if hit {
                total += 1;
                if child == id {
                    at = total;
                }
            }
            child = child_node.next_sibling();
        }
        Some((at, total))
    }

    /// Tries the per-candidate sibling walk; `false` when the step budget (one
    /// document pass) runs out and the caller must build the child index.  The
    /// budget is required: a page that is one huge list makes this
    /// O(candidates * siblings).
    fn filter_sparse(
        &self,
        mask: &mut BitSet,
        of_type: bool,
        keep: impl Fn(u32, u32) -> bool,
    ) -> bool {
        if mask.count_ones() > Self::SPARSE_CANDIDATES {
            return false;
        }
        let mut budget = self.n;
        let mut plan: Vec<(NodeId, u32, u32)> = Vec::with_capacity(mask.count_ones() as usize);
        for id in mask.iter_set() {
            match self.sibling_position_within(id, of_type, &mut budget) {
                Some((at, total)) => plan.push((id, at, total)),
                None => return false,
            }
        }
        for (id, at, total) in plan {
            if !keep(at, total) {
                mask.clear(id);
            }
        }
        true
    }

    /// Filters `mask` by a sibling-position pseudo.
    fn filter_position(&self, mask: &mut BitSet, position: Position) {
        if self.filter_sparse(mask, false, |at, total| position.keep(at, total)) {
            return;
        }
        let index = self.child_index();
        let filter = |id: NodeId| {
            let at = index.element_index[id.index()];
            let total = index.element_child_count[self.doc.node(id).parent().index()];
            position.keep(at, total)
        };
        self.filter(mask, filter);
    }

    fn filter_of_type(&self, mask: &mut BitSet, tag: Option<&str>, mode: OfType) {
        if tag.is_some() && self.filter_sparse(mask, true, |at, total| mode.keep(at, total)) {
            return;
        }
        if tag.is_none() {
            let ids: Vec<NodeId> = mask.iter_set().collect();
            for id in ids {
                let (index, total) = self.type_index_by_name(id);
                let keep = match mode {
                    OfType::First => index == 1,
                    OfType::Last => index == total,
                    OfType::Nth(a, b) => nth(index, a, b),
                    OfType::NthLast(a, b) => nth_i64(total as i64 - index as i64 + 1, a, b),
                };
                if !keep {
                    mask.clear(id);
                }
            }
            return;
        }
        let index = self.child_index();
        let type_mask = tag.map(|name| self.tag_mask(name));
        for parent in 0..self.n {
            let start = index.child_start[parent] as usize;
            let end = index.child_start[parent + 1] as usize;
            let total = if matches!(mode, OfType::Last | OfType::NthLast(..)) {
                index.children[start..end]
                    .iter()
                    .filter(|&&child| {
                        self.doc.node(child).is_element()
                            && type_mask.as_ref().is_none_or(|m| m.get(child))
                    })
                    .count() as u32
            } else {
                0
            };
            let mut count = 0u32;
            for &child in &index.children[start..end] {
                if !self.doc.node(child).is_element() {
                    continue;
                }
                if !type_mask.as_ref().is_some_and(|m| m.get(child)) {
                    continue;
                }
                count += 1;
                if !mask.get(child) {
                    continue;
                }
                let keep = match mode {
                    OfType::First => count == 1,
                    OfType::Last => count == total,
                    OfType::Nth(a, b) => nth(count, a, b),
                    OfType::NthLast(a, b) => nth_i64(total as i64 - count as i64 + 1, a, b),
                };
                if !keep {
                    mask.clear(child);
                }
            }
        }
    }

    fn type_index_by_name(&self, id: NodeId) -> (u32, u32) {
        let node = self.doc.node(id);
        let name = self.doc.tag_name(node.tag_token());
        let mut index = 0u32;
        let mut total = 0u32;
        let mut child = self.doc.node(node.parent()).first_child();
        while child != NO_NODE {
            let child_node = self.doc.node(child);
            if child_node.is_element()
                && self
                    .doc
                    .tag_name(child_node.tag_token())
                    .eq_ignore_ascii_case(name)
            {
                total += 1;
                if child <= id {
                    index += 1;
                }
            }
            child = child_node.next_sibling();
        }
        (index, total)
    }

    fn filter(&self, mask: &mut BitSet, mut predicate: impl FnMut(NodeId) -> bool) {
        let ids: Vec<NodeId> = mask.iter_set().collect();
        for id in ids {
            if !predicate(id) {
                mask.clear(id);
            }
        }
    }

    /// `ancestors(S)` with preorder prefix counts over `subtree_end`.
    fn ancestors(&self, set: &BitSet) -> BitSet {
        let mut prefix = vec![0u32; self.n + 1];
        for id in 0..self.n {
            prefix[id + 1] = prefix[id] + u32::from(set.get(NodeId::from(id)));
        }

        let mut out = BitSet::new(self.n);
        for id in 0..self.n {
            let node = self.doc.node(NodeId::from(id));
            if !node.is_element() {
                continue;
            }
            let end = (node.subtree_end().index()).min(self.n);
            if prefix[end] > prefix[id + 1] {
                out.set(NodeId::from(id));
            }
        }
        out
    }

    /// `parents(S)` as a scatter over the set bits.
    fn parents(&self, set: &BitSet) -> BitSet {
        let mut out = BitSet::new(self.n);
        for id in set.iter_set() {
            let parent = self.doc.node(id).parent();
            if parent != NO_NODE {
                out.set(parent);
            }
        }
        out
    }

    /// `prev_element_sibling(S)` as one pass over the child index.
    fn prev_element_sibling(&self, set: &BitSet) -> BitSet {
        let index = self.child_index();
        let mut out = BitSet::new(self.n);
        for parent in 0..self.n {
            let start = index.child_start[parent] as usize;
            let end = index.child_start[parent + 1] as usize;
            let mut prev = NO_NODE;
            for &child in &index.children[start..end] {
                if !self.doc.node(child).is_element() {
                    continue;
                }
                if set.get(child) && prev != NO_NODE {
                    out.set(prev);
                }
                prev = child;
            }
        }
        out
    }

    /// `earlier_element_siblings(S)` as a reverse segmented OR per parent.
    fn earlier_element_siblings(&self, set: &BitSet) -> BitSet {
        let index = self.child_index();
        let mut out = BitSet::new(self.n);
        for parent in 0..self.n {
            let start = index.child_start[parent] as usize;
            let end = index.child_start[parent + 1] as usize;
            let mut seen = false;
            for &child in index.children[start..end].iter().rev() {
                if !self.doc.node(child).is_element() {
                    continue;
                }
                if seen {
                    out.set(child);
                }
                if set.get(child) {
                    seen = true;
                }
            }
        }
        out
    }

    /// Projects matched elements to their direct text children or attributes.
    fn project(&self, record: MatchRecord, pseudo: &PseudoElement, out: &mut Vec<MatchRecord>) {
        match pseudo {
            PseudoElement::Text => {
                let mut child = self.doc.node(record.node).first_child();
                while child != NO_NODE {
                    if self.doc.node(child).is_text() {
                        out.push(MatchRecord {
                            node: child,
                            kind: MatchKind::Text,
                            span: self.doc.node(child).span(),
                            outer_end: self.doc.node(child).outer_end(),
                            attr: None,
                        });
                    }
                    child = self.doc.node(child).next_sibling();
                }
            }
            PseudoElement::Attr(name) => {
                if let Some(attr) = crate::dom::find_attr(self.doc, record.node, name) {
                    let start = self.doc.attrs.value_start[attr as usize];
                    let span = start..start + self.doc.attrs.value_len[attr as usize];
                    out.push(MatchRecord {
                        node: record.node,
                        kind: MatchKind::Attr,
                        span: span.clone(),
                        outer_end: span.end,
                        attr: Some(span),
                    });
                }
            }
        }
    }
}

/// Scalar key mask used when AVX2 is unavailable or the key is too long.
fn key_mask_scalar(
    doc: &Document<'_>,
    starts: &[u32],
    lens: &[u16],
    count: usize,
    needle: &[u8],
    resolve: impl Fn(usize) -> NodeId,
    mask: &mut BitSet,
) {
    for i in 0..count {
        let len = lens[i] as usize;
        if len != needle.len() {
            continue;
        }
        let start = starts[i] as usize;
        if !doc.input[start..start + len].eq_ignore_ascii_case(needle) {
            continue;
        }
        let node = resolve(i);
        if node != NO_NODE {
            mask.set(node);
        }
    }
}

/// Builds a node mask for a short, case-insensitive key (tag or attribute name).
/// `starts`/`lens` are the key spans; `resolve` maps a key index to its element
/// (`NO_NODE` for keys that own none, such as end tags).
/// Sets every element whose tag code is `code`: one byte per tag token makes this
/// a dense `cmpeq`+`movemask` scan (measured 0.22 vs 7.90 cycles per tag).
fn tag_code_mask(doc: &Document<'_>, code: u8, mask: &mut BitSet) {
    #[cfg(target_arch = "x86_64")]
    {
        if doc.tags.code.len() >= 64 && simdhtml::simd::avx512_available() {
            // SAFETY: AVX-512 checked above; the kernel bounds-checks its tail.
            unsafe { tag_code_mask_avx512(doc, code, mask) };
            return;
        }
        if doc.tags.code.len() >= 32 && simdhtml::simd::avx2_available() {
            // SAFETY: AVX2 checked above; the kernel bounds-checks its tail.
            unsafe { tag_code_mask_avx2(doc, code, mask) };
            return;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if doc.tags.code.len() >= 16 && simdhtml::simd::neon_available() {
            // SAFETY: NEON is part of the base aarch64 ISA.
            unsafe { tag_code_mask_neon(doc, code, mask) };
            return;
        }
    }
    tag_code_mask_scalar(doc, code, mask);
}

fn tag_code_mask_scalar(doc: &Document<'_>, code: u8, mask: &mut BitSet) {
    for (i, &c) in doc.tags.code.iter().enumerate() {
        if c == code {
            let node = doc.tags.element[i];
            if node != NO_NODE {
                mask.set(node);
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn tag_code_mask_avx512(doc: &Document<'_>, code: u8, mask: &mut BitSet) {
    use std::arch::x86_64::{
        __m512i, _mm512_cmpeq_epi8_mask, _mm512_loadu_si512, _mm512_set1_epi8,
    };
    unsafe {
        let codes = &doc.tags.code;
        let needle = _mm512_set1_epi8(code as i8);
        let mut i = 0usize;
        while i + 64 <= codes.len() {
            let v = _mm512_loadu_si512(codes.as_ptr().add(i).cast::<__m512i>());
            let mut bits = _mm512_cmpeq_epi8_mask(v, needle);
            while bits != 0 {
                let j = bits.trailing_zeros() as usize;
                let node = doc.tags.element[i + j];
                if node != NO_NODE {
                    mask.set(node);
                }
                bits &= bits - 1;
            }
            i += 64;
        }
        for (k, item) in codes.iter().enumerate().skip(i) {
            if *item == code {
                let node = doc.tags.element[k];
                if node != NO_NODE {
                    mask.set(node);
                }
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn tag_code_mask_neon(doc: &Document<'_>, code: u8, mask: &mut BitSet) {
    use std::arch::aarch64::{vceqq_u8, vdupq_n_u8, vld1q_u8};

    use simdhtml::simd::neon::movemask_u16;

    unsafe {
        let codes = &doc.tags.code;
        let needle = vdupq_n_u8(code);
        let mut i = 0usize;
        while i + 16 <= codes.len() {
            let v = vld1q_u8(codes.as_ptr().add(i));
            let mut bits = movemask_u16(vceqq_u8(v, needle));
            while bits != 0 {
                let j = bits.trailing_zeros() as usize;
                let node = doc.tags.element[i + j];
                if node != NO_NODE {
                    mask.set(node);
                }
                bits &= bits - 1;
            }
            i += 16;
        }
        for (k, item) in codes.iter().enumerate().skip(i) {
            if *item == code {
                let node = doc.tags.element[k];
                if node != NO_NODE {
                    mask.set(node);
                }
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn tag_code_mask_avx2(doc: &Document<'_>, code: u8, mask: &mut BitSet) {
    use std::arch::x86_64::{
        __m256i, _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_set1_epi8,
    };
    unsafe {
        let codes = &doc.tags.code;
        let needle = _mm256_set1_epi8(code as i8);
        let mut i = 0usize;
        while i + 32 <= codes.len() {
            let v = _mm256_loadu_si256(codes.as_ptr().add(i).cast::<__m256i>());
            let mut bits = _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, needle)) as u32;
            while bits != 0 {
                let j = bits.trailing_zeros() as usize;
                let node = doc.tags.element[i + j];
                if node != NO_NODE {
                    mask.set(node);
                }
                bits &= bits - 1;
            }
            i += 32;
        }
        for (k, item) in codes.iter().enumerate().skip(i) {
            if *item == code {
                let node = doc.tags.element[k];
                if node != NO_NODE {
                    mask.set(node);
                }
            }
        }
    }
}

fn key_mask(
    doc: &Document<'_>,
    starts: &[u32],
    lens: &[u16],
    count: usize,
    name: &str,
    resolve: impl Fn(usize) -> NodeId + Copy,
    mask: &mut BitSet,
) {
    let needle = name.as_bytes();
    if count >= 64 && !needle.is_empty() && needle.len() <= 8 {
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 checked above; the kernel bounds-checks its tail.
                unsafe { key_mask_avx2(doc, starts, lens, count, needle, resolve, mask) };
                return;
            }
        }
    }
    key_mask_scalar(doc, starts, lens, count, needle, resolve, mask);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn key_mask_avx2(
    doc: &Document<'_>,
    starts: &[u32],
    lens: &[u16],
    count: usize,
    needle: &[u8],
    resolve: impl Fn(usize) -> NodeId + Copy,
    mask: &mut BitSet,
) {
    unsafe {
        if needle.len() <= 4 {
            key_mask_avx2_4(doc, starts, lens, count, needle, resolve, mask);
        } else {
            key_mask_avx2_8(doc, starts, lens, count, needle, resolve, mask);
        }
    }

    #[target_feature(enable = "avx2")]
    unsafe fn key_mask_avx2_4(
        doc: &Document<'_>,
        starts: &[u32],
        lens: &[u16],
        count: usize,
        needle: &[u8],
        resolve: impl Fn(usize) -> NodeId + Copy,
        mask: &mut BitSet,
    ) {
        unsafe {
            use std::arch::x86_64::{
                __m128i, __m256, __m256i, _mm_loadu_si128, _mm256_and_si256, _mm256_cmpeq_epi32,
                _mm256_cvtepu16_epi32, _mm256_i32gather_epi32, _mm256_loadu_si256,
                _mm256_movemask_ps, _mm256_or_si256, _mm256_set1_epi32,
            };

            let len = needle.len();
            let mut expected = 0u32;
            for (i, &b) in needle.iter().enumerate() {
                expected |= u32::from(b.to_ascii_lowercase()) << (i * 8);
            }
            let byte_mask: u32 = if len == 4 {
                u32::MAX
            } else {
                (1u32 << (len * 8)) - 1
            };
            let expected = _mm256_set1_epi32(expected as i32);
            let byte_mask = _mm256_set1_epi32(byte_mask as i32);
            let fold = _mm256_set1_epi32(0x2020_2020);
            let needle_len = _mm256_set1_epi32(len as i32);
            let input = doc.input.as_ptr();
            let starts_ptr = starts.as_ptr();
            let lens_ptr = lens.as_ptr();
            let mut i = 0usize;

            while i + 8 <= count {
                let mut max_start = 0usize;
                for j in 0..8 {
                    max_start = max_start.max(starts[i + j] as usize);
                }
                if max_start + 4 > doc.input.len() {
                    break;
                }
                let idx = _mm256_loadu_si256(starts_ptr.add(i).cast::<__m256i>());
                let gathered = _mm256_i32gather_epi32(input.cast::<i32>(), idx, 1);
                let folded = _mm256_or_si256(gathered, fold);
                let masked = _mm256_and_si256(folded, byte_mask);
                let bytes_ok = _mm256_cmpeq_epi32(masked, expected);
                let lenv =
                    _mm256_cvtepu16_epi32(_mm_loadu_si128(lens_ptr.add(i).cast::<__m128i>()));
                let len_ok = _mm256_cmpeq_epi32(lenv, needle_len);
                let mut bits =
                    (_mm256_movemask_ps(std::mem::transmute::<__m256i, __m256>(bytes_ok))
                        & _mm256_movemask_ps(std::mem::transmute::<__m256i, __m256>(len_ok)))
                        as u32;
                while bits != 0 {
                    let j = bits.trailing_zeros() as usize;
                    let node = resolve(i + j);
                    if node != NO_NODE {
                        mask.set(node);
                    }
                    bits &= bits - 1;
                }
                i += 8;
            }
            key_mask_scalar(
                doc,
                &starts[i..],
                &lens[i..],
                count - i,
                needle,
                |j| resolve(i + j),
                mask,
            );
        }
    }

    #[target_feature(enable = "avx2")]
    unsafe fn key_mask_avx2_8(
        doc: &Document<'_>,
        starts: &[u32],
        lens: &[u16],
        count: usize,
        needle: &[u8],
        resolve: impl Fn(usize) -> NodeId + Copy,
        mask: &mut BitSet,
    ) {
        unsafe {
            use std::arch::x86_64::{
                __m128i, __m256, __m256d, __m256i, _mm_loadu_si128, _mm256_and_si256,
                _mm256_cmpeq_epi32, _mm256_cmpeq_epi64, _mm256_cvtepu16_epi32,
                _mm256_i32gather_epi64, _mm256_movemask_pd, _mm256_movemask_ps, _mm256_or_si256,
                _mm256_set1_epi32, _mm256_set1_epi64x,
            };

            let len = needle.len();
            let mut expected = 0u64;
            for (i, &b) in needle.iter().enumerate() {
                expected |= u64::from(b.to_ascii_lowercase()) << (i * 8);
            }
            let byte_mask: u64 = if len == 8 {
                u64::MAX
            } else {
                (1u64 << (len * 8)) - 1
            };
            let expected = _mm256_set1_epi64x(expected as i64);
            let byte_mask = _mm256_set1_epi64x(byte_mask as i64);
            let fold = _mm256_set1_epi64x(0x2020_2020_2020_2020);
            let needle_len = _mm256_set1_epi32(len as i32);
            let input = doc.input.as_ptr();
            let starts_ptr = starts.as_ptr();
            let lens_ptr = lens.as_ptr();
            let mut i = 0usize;

            while i + 8 <= count {
                let mut max_start = 0usize;
                for j in 0..8 {
                    max_start = max_start.max(starts[i + j] as usize);
                }
                if max_start + 8 > doc.input.len() {
                    break;
                }
                let idx = _mm_loadu_si128(starts_ptr.add(i).cast::<__m128i>());
                let gathered = _mm256_i32gather_epi64(input.cast::<i64>(), idx, 1);
                let folded = _mm256_or_si256(gathered, fold);
                let masked = _mm256_and_si256(folded, byte_mask);
                let bytes_ok = _mm256_cmpeq_epi64(masked, expected);
                let lenv =
                    _mm256_cvtepu16_epi32(_mm_loadu_si128(lens_ptr.add(i).cast::<__m128i>()));
                let len_ok = _mm256_cmpeq_epi32(lenv, needle_len);
                let mut bits =
                    (_mm256_movemask_pd(std::mem::transmute::<__m256i, __m256d>(bytes_ok))
                        & (_mm256_movemask_ps(std::mem::transmute::<__m256i, __m256>(len_ok))
                            & 0xF)) as u32;
                while bits != 0 {
                    let j = bits.trailing_zeros() as usize;
                    let node = resolve(i + j);
                    if node != NO_NODE {
                        mask.set(node);
                    }
                    bits &= bits - 1;
                }
                i += 4;
            }
            key_mask_scalar(
                doc,
                &starts[i..],
                &lens[i..],
                count - i,
                needle,
                |j| resolve(i + j),
                mask,
            );
        }
    }
}

/// Whether the selector starts with the implied universal of a bare
/// pseudo-element (`::text`), which must match the scope root itself.
fn selector_starts_with_implied_universal(selector: &SelectorList) -> bool {
    selector
        .complexes
        .iter()
        .any(|complex| complex.compounds[0].implied_universal)
}

fn selector_has_scope(selector: &SelectorList) -> bool {
    let complexes = |list: &[Complex]| list.iter().any(complex_has_scope);
    selector.complexes.iter().any(|complex| {
        complex.compounds.iter().any(|compound| {
            compound.pseudos.iter().any(|pseudo| match pseudo {
                Pseudo::Scope => true,
                Pseudo::Not(list) | Pseudo::Is(list) => complexes(list),
                Pseudo::Has(rels) => rels.iter().any(|r| complex_has_scope(&r.complex)),
                _ => false,
            })
        })
    })
}

fn complex_has_scope(complex: &Complex) -> bool {
    complex.compounds.iter().any(|compound| {
        compound.pseudos.iter().any(|pseudo| match pseudo {
            Pseudo::Scope => true,
            Pseudo::Not(list) | Pseudo::Is(list) => list.iter().any(complex_has_scope),
            Pseudo::Has(rels) => rels.iter().any(|r| complex_has_scope(&r.complex)),
            _ => false,
        })
    })
}

/// The document's root element (first element child of the document node).
pub fn root_element(doc: &Document<'_>) -> Option<NodeId> {
    let mut child = doc.node(DOC_ROOT).first_child();
    while child != NO_NODE {
        let node = doc.node(child);
        if node.is_element() {
            return Some(child);
        }
        child = node.next_sibling();
    }
    None
}

fn element_record(doc: &Document<'_>, id: NodeId) -> MatchRecord {
    let node = doc.node(id);
    MatchRecord {
        node: id,
        kind: MatchKind::Element,
        span: node.span(),
        outer_end: node.outer_end(),
        attr: None,
    }
}

pub(super) fn common_pseudo(selector: &SelectorList) -> Option<PseudoElement> {
    let first = selector.complexes.first()?.pseudo.clone();
    if selector
        .complexes
        .iter()
        .all(|complex| complex.pseudo == first)
    {
        first
    } else {
        None
    }
}

#[inline]
fn matches_attr(doc: &Document<'_>, node: NodeId, pred: &AttrPred) -> bool {
    let Some(attr) = crate::dom::find_attr(doc, node, &pred.name) else {
        return false;
    };
    // Presence needs no value, and a value without references needs no decoding,
    // so neither goes through `find_attr_value` (the hot path here).
    if pred.op == AttrOp::Exists {
        return true;
    }
    let raw = crate::dom::attr_value_raw(doc, attr);
    if !raw.contains(&b'&') {
        let value = std::str::from_utf8(raw).expect("Document input is valid UTF-8");
        return matches_attr_value(value, pred);
    }
    let value = crate::dom::attr_value(doc, attr);
    matches_attr_value(value.as_ref(), pred)
}

fn matches_attr_value(value: &str, pred: &AttrPred) -> bool {
    let expected = pred.value.as_deref().unwrap_or("");
    let case_insensitive = pred.case_insensitive;

    match pred.op {
        AttrOp::Exists => true,
        AttrOp::NotEquals => !eq(value, expected, case_insensitive),
        AttrOp::Equals => eq(value, expected, case_insensitive),
        AttrOp::Includes => value
            .split_ascii_whitespace()
            .any(|part| eq(part, expected, case_insensitive)),
        AttrOp::DashMatch => {
            eq(value, expected, case_insensitive)
                || (value.len() > expected.len()
                    && value.as_bytes().get(expected.len()) == Some(&b'-')
                    && value
                        .get(..expected.len())
                        .is_some_and(|prefix| eq(prefix, expected, case_insensitive)))
        }
        AttrOp::Prefix => value
            .get(..expected.len())
            .is_some_and(|prefix| eq(prefix, expected, case_insensitive)),
        AttrOp::Suffix => value
            .get(value.len().saturating_sub(expected.len())..)
            .is_some_and(|suffix| eq(suffix, expected, case_insensitive)),
        AttrOp::Substring => {
            if case_insensitive {
                value
                    .to_ascii_lowercase()
                    .contains(&expected.to_ascii_lowercase())
            } else {
                value.contains(expected)
            }
        }
    }
}

fn eq(left: &str, right: &str, case_insensitive: bool) -> bool {
    if case_insensitive {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn nth(index: u32, a: i32, b: i32) -> bool {
    nth_i64(i64::from(index), a, b)
}

fn nth_i64(index: i64, a: i32, b: i32) -> bool {
    let diff = index - i64::from(b);
    if a == 0 {
        diff == 0
    } else {
        diff % i64::from(a) == 0 && diff / i64::from(a) >= 0
    }
}
