use super::*;
use crate::{Warning, WarningKind};

/// Insertion modes, named as in the specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    AfterBody,
    AfterAfterBody,
    InTable,
    InTableBody,
    InRow,
    InCell,
    InCaption,
    InColumnGroup,
    InTableText,
    InFrameset,
    AfterFrameset,
    AfterAfterFrameset,
    InTemplate,
}

impl Mode {
    /// The mode a `<template>` re-enters after its contents (simplified).
    fn after_template(self) -> Mode {
        match self {
            Mode::InTable | Mode::InTableBody | Mode::InRow | Mode::InCell | Mode::InCaption => {
                self
            }
            _ => Mode::InBody,
        }
    }
}

/// What a handler wants to happen to the token.
pub(super) enum Step {
    Done,
    Reprocess,
}

/// Sentinel for the intrusive child links (`u32::MAX` is also "no parent").
const NO_LINK: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Document,
    Element,
    Text,
    Comment,
    Doctype,
}

#[derive(Clone, Copy, Debug)]
enum Name {
    Token(u32),
    Implied(&'static str),
}

pub(super) struct Node {
    kind: Kind,
    name: Option<Name>,
    parent: u32,
    /// Intrusive child list: four links per node make append, insert-before,
    /// detach and "move all children" O(1) and allocation-free (a `Vec<u32>` per
    /// node cost one allocation per node).
    first_child: u32,
    last_child: u32,
    next_sibling: u32,
    prev_sibling: u32,
    /// Namespace of an element, decided here and handed to the tape: rebuilding
    /// it on the tape would duplicate the integration-point rules and cannot see
    /// `annotation-xml`'s `encoding` attribute.
    ns: u8,
    span: Range<u32>,
    /// Source offset just past the element's end tag (or the point where it
    /// was implicitly closed), used for `outer_html`.
    end: u32,
    raw_text: bool,
}

/// Elements whose start tag closes an open `<p>` (`in body`).
/// Offset of the `</script` that ends script data starting at `start`.
///
/// The scanner ends a raw region at the first `</script`, but script data has
/// escape states: `<!--` enters "escaped", `<script` inside that enters
/// "double-escaped" where `</script` is text, and `-->` leaves them.  This is a
/// rescan of the region the scanner reported.
fn script_data_end(input: &[u8], start: usize) -> usize {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Normal,
        Escaped,
        DoubleEscaped,
    }
    let mut state = State::Normal;
    let mut i = start;
    let len = input.len();
    let starts = |at: usize, needle: &[u8]| {
        input
            .get(at..at + needle.len())
            .is_some_and(|bytes| bytes.eq_ignore_ascii_case(needle))
    };
    // `</script` or `<script` followed by a byte that may end a tag name.
    let tag_at = |at: usize, slash: bool| {
        let needle: &[u8] = if slash { b"</script" } else { b"<script" };
        if !starts(at, needle) {
            return false;
        }
        match input.get(at + needle.len()) {
            // EOF in the end-tag-name state does not emit the end tag.
            None => false,
            Some(&byte) => matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c | b'>' | b'/'),
        }
    };
    // In the *normal* state the same bytes are already inside the end tag's
    // states, so their text is dropped rather than kept
    // (`<script></script ` ends with an empty script element).
    let closing_at = |at: usize| {
        starts(at, b"</script")
            && match input.get(at + 8) {
                // EOF right after the name keeps the bytes as text
                // (`X</SCRipt`), while a separator starts the end tag proper and
                // its bytes are dropped (`<script></script `).
                None => false,
                Some(&byte) => {
                    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c | b'>' | b'/')
                }
            }
    };
    while i < len {
        match state {
            State::Normal => {
                if starts(i, b"<!--") {
                    state = State::Escaped;
                    i += 4;
                } else if closing_at(i) {
                    return i;
                } else {
                    i += 1;
                }
            }
            State::Escaped => {
                if starts(i, b"-->") {
                    state = State::Normal;
                    i += 3;
                } else if tag_at(i, true) {
                    return i;
                } else if tag_at(i, false) {
                    state = State::DoubleEscaped;
                    i += 7;
                } else {
                    i += 1;
                }
            }
            State::DoubleEscaped => {
                if starts(i, b"-->") {
                    state = State::Escaped;
                    i += 3;
                } else if tag_at(i, true) {
                    state = State::Escaped;
                    i += 8;
                } else {
                    i += 1;
                }
            }
        }
    }
    len
}

/// Start tags whose `in body` entry leaves `frameset-ok` set.
///
/// The flag is cleared by a handful of explicit entries (pre, listing, li, dd, dt,
/// button, table, hr, xmp, input, textarea, iframe, select, area, br, embed, img,
/// keygen, wbr) and by the catch-all "any other start tag"; every name here has its
/// own entry and does not clear it (`svg`/`math` included).
fn keeps_frameset_ok(name: &[u8]) -> bool {
    const OWN_ENTRY: [&[u8]; 85] = [
        b"html",
        b"head",
        b"body",
        b"base",
        b"basefont",
        b"bgsound",
        b"link",
        b"meta",
        b"noframes",
        b"script",
        b"style",
        b"title",
        b"template",
        b"a",
        b"b",
        b"big",
        b"code",
        b"em",
        b"font",
        b"i",
        b"nobr",
        b"s",
        b"small",
        b"strike",
        b"strong",
        b"tt",
        b"u",
        b"h1",
        b"h2",
        b"h3",
        b"h4",
        b"h5",
        b"h6",
        b"form",
        b"option",
        b"optgroup",
        b"ruby",
        b"rb",
        b"rtc",
        b"rp",
        b"rt",
        b"frameset",
        b"frame",
        b"marquee",
        b"object",
        b"applet",
        b"caption",
        b"col",
        b"colgroup",
        b"tbody",
        b"td",
        b"tfoot",
        b"th",
        b"thead",
        b"tr",
        b"plaintext",
        b"svg",
        b"math",
        b"address",
        b"article",
        b"aside",
        b"blockquote",
        b"center",
        b"details",
        b"dialog",
        b"dir",
        b"div",
        b"dl",
        b"fieldset",
        b"figcaption",
        b"figure",
        b"footer",
        b"header",
        b"hgroup",
        b"main",
        b"menu",
        b"nav",
        b"ol",
        b"p",
        b"section",
        b"summary",
        b"ul",
        b"param",
        b"source",
        b"track",
    ];
    OWN_ENTRY.contains(&name)
}

/// Lowercases a tag name into `buf` without allocating.
///
/// Every insertion-mode handler matches on the lowercased name; allocating a
/// `String` per token cost ~110 us/doc in tree build.
fn lower_into<'b>(buf: &'b mut [u8; 32], input: &[u8], tokens: &Tokens, tag: u32) -> &'b [u8] {
    let start = tokens.tag_name_start[tag as usize] as usize;
    let len = tokens.tag_name_len[tag as usize] as usize;
    let len = len.min(buf.len());
    for (slot, &byte) in buf.iter_mut().zip(&input[start..start + len]) {
        *slot = byte.to_ascii_lowercase();
    }
    &buf[..len]
}

const CLOSES_P: &[&[u8]] = &[
    b"address",
    b"article",
    b"aside",
    b"blockquote",
    b"center",
    b"details",
    b"dialog",
    b"dir",
    b"div",
    b"dl",
    b"fieldset",
    b"figcaption",
    b"figure",
    b"footer",
    b"header",
    b"hgroup",
    b"main",
    b"menu",
    b"nav",
    b"ol",
    b"p",
    b"pre",
    b"search",
    b"section",
    b"summary",
    b"ul",
];

/// Elements that bound a scope search (`parse-state.txt`).
/// Extra scope boundary names on top of the base set, per scope kind.
const BUTTON_EXTRA: &[&[u8]] = &[b"button"];
const LIST_ITEM_EXTRA: &[&[u8]] = &[b"ol", b"ul"];

/// Entry of the "list of active formatting elements".
#[derive(Clone, Copy, Debug)]
enum Afe {
    Marker,
    Element(u32),
}

/// Formatting elements the adoption agency algorithm applies to.
const FORMATTING: &[&[u8]] = &[
    b"a", b"b", b"big", b"code", b"em", b"font", b"i", b"nobr", b"s", b"small", b"strike",
    b"strong", b"tt", b"u",
];

/// End tags with their own `in body` entry: pop with implied end tags.
const BLOCK_END: &[&[u8]] = &[
    b"address",
    b"article",
    b"aside",
    b"blockquote",
    b"button",
    b"center",
    b"details",
    b"dialog",
    b"dir",
    b"div",
    b"dl",
    b"fieldset",
    b"figcaption",
    b"figure",
    b"footer",
    b"header",
    b"hgroup",
    b"listing",
    b"main",
    b"menu",
    b"nav",
    b"ol",
    b"pre",
    b"search",
    b"section",
    b"summary",
    b"ul",
];

/// Elements the specification calls "special" (furthest-block search).
const SPECIAL: &[&[u8]] = &[
    b"address",
    b"applet",
    b"area",
    b"article",
    b"aside",
    b"base",
    b"basefont",
    b"bgsound",
    b"blockquote",
    b"body",
    b"br",
    b"button",
    b"caption",
    b"center",
    b"col",
    b"colgroup",
    b"dd",
    b"details",
    b"dir",
    b"div",
    b"dl",
    b"dt",
    b"embed",
    b"fieldset",
    b"figcaption",
    b"figure",
    b"footer",
    b"form",
    b"frame",
    b"frameset",
    b"h1",
    b"h2",
    b"h3",
    b"h4",
    b"h5",
    b"h6",
    b"head",
    b"header",
    b"hgroup",
    b"hr",
    b"html",
    b"iframe",
    b"img",
    b"input",
    b"keygen",
    b"li",
    b"link",
    b"listing",
    b"main",
    b"marquee",
    b"menu",
    b"meta",
    b"nav",
    b"noembed",
    b"noframes",
    b"noscript",
    b"object",
    b"ol",
    b"p",
    b"param",
    b"plaintext",
    b"pre",
    b"script",
    b"search",
    b"section",
    b"select",
    b"source",
    b"style",
    b"summary",
    b"table",
    b"tbody",
    b"td",
    b"template",
    b"textarea",
    b"tfoot",
    b"th",
    b"thead",
    b"title",
    b"tr",
    b"track",
    b"ul",
    b"wbr",
    b"xmp",
];

/// HTML start tags that leave foreign content (`in foreign content`, "any other
/// start tag" breakout list).
const BREAKOUT: &[&[u8]] = &[
    b"b",
    b"big",
    b"blockquote",
    b"body",
    b"br",
    b"center",
    b"code",
    b"dd",
    b"div",
    b"dl",
    b"dt",
    b"em",
    b"embed",
    b"h1",
    b"h2",
    b"h3",
    b"h4",
    b"h5",
    b"h6",
    b"head",
    b"hr",
    b"i",
    b"img",
    b"li",
    b"listing",
    b"menu",
    b"meta",
    b"nobr",
    b"ol",
    b"p",
    b"pre",
    b"ruby",
    b"s",
    b"small",
    b"span",
    b"strong",
    b"strike",
    b"sub",
    b"sup",
    b"table",
    b"tt",
    b"u",
    b"ul",
    b"var",
];

pub(super) struct SpecTree<'a> {
    input: &'a [u8],
    tokens: &'a Tokens,
    /// Markup scan regions, sorted by start.  A raw text element's region may reach
    /// past the end tag that tree construction found (`</script` without a
    /// `>`), and those bytes are *dropped*, so the rescan needs the region end.
    regions: &'a [crate::OpaqueRegion],
    nodes: Vec<Node>,
    stack: Vec<u32>,
    mode: Mode,
    /// Mode to restore when a `text` element closes.
    text_return: Mode,
    /// `frameset-ok`, cleared by content a frameset would displace.
    frameset_ok: bool,
    /// Whether the Document is in quirks mode.  One rule reads it: `in body`'s
    /// `table` start tag only closes a paragraph outside quirks mode.
    quirks: bool,
    /// Open `form` element, for the form element pointer rule.
    form: Option<u32>,
    /// `<pre>`/`<listing>` ignore a single newline that immediately follows.
    skip_next_newline: bool,
    /// Whether an implied element or implied end tag was generated.
    implied_end: bool,
    /// Offset of the token being processed, for implied closes.
    at: u32,
    /// Adjusted SVG names, `(tag token, 1-based name index)`.
    adjusted: Vec<(u32, u8)>,
    /// Tokens and text runs starting before this offset were swallowed by a
    /// raw-text rescan and must be ignored.
    skip_until: u32,
    /// List of active formatting elements.
    afe: Vec<Afe>,
    /// Namespace for each open element's *children*, parallel to `stack`.
    ns: Vec<u8>,
    /// Pending characters for the "in table text" mode.
    table_text: Vec<(Range<u32>, bool)>,
    /// Mode to restore when the pending table text is flushed.
    table_text_return: Mode,
    /// `(destination node, source tag)` pairs whose attributes are merged by
    /// the `html`/`body` start tag rules.
    merges: Vec<(u32, u32)>,
    /// Foster-parent the next inserted element (used when the in-body rules are
    /// run from a table mode: "process the token using the rules for the in body
    /// insertion mode" with foster parenting).
    foster_next_insert: bool,
    /// Stack of insertion modes to restore when a `<template>` closes.
    template_modes: Vec<Mode>,
    warnings: Vec<Warning>,
}

/// What tree construction hands back: document flags, warnings, per-token
/// adjusted SVG names, and `(destination node, source tag)` attribute merges.
type TreeResult = (DocumentFlags, Vec<Warning>, Vec<u8>, Vec<(u32, u32)>);

pub(super) fn build(
    input: &[u8],
    tokens: &Tokens,
    index: &MarkupIndex,
    mode: RepairMode,
    builder: &mut TreeBuilder,
) -> TreeResult {
    if mode == RepairMode::None {
        let (flags, warnings) = build_without_repair(input, tokens, index, builder);
        return (flags, warnings, Vec::new(), Vec::new());
    }
    let mut tree = SpecTree {
        quirks: false,
        input,
        tokens,
        regions: &index.opaque_regions,
        nodes: Vec::with_capacity(tokens.tags.len() + index.text_runs.len() + 1),
        stack: Vec::with_capacity(32),
        mode: Mode::Initial,
        text_return: Mode::InBody,
        frameset_ok: true,
        form: None,
        skip_next_newline: false,
        implied_end: false,
        at: 0,
        skip_until: 0,
        adjusted: Vec::new(),
        afe: Vec::new(),
        ns: vec![crate::NS_HTML],
        table_text: Vec::new(),
        table_text_return: Mode::InTable,
        foster_next_insert: false,
        merges: Vec::new(),
        template_modes: Vec::new(),
        warnings: Vec::new(),
    };
    tree.push_node(Kind::Document, None, 0..0, false);
    tree.stack.push(0);

    // A DOCTYPE token ends at its first `>` (the tokenizer's DOCTYPE states do
    // not know about an internal subset - `[...]` is just bogus DOCTYPE data
    // with parse errors), so whatever follows it is ordinary text.  There is no
    // "skip to the matching ]>" rule in the standard; the scanner's region end
    // is authoritative.
    let doctype_skip_until = 0u32;

    // Comments, processing instructions, bogus comments and doctypes: the
    // scanner masked them into regions, and the tree keeps them as nodes.
    let mut comments: Vec<(u32, u32, bool)> = index
        .opaque_regions
        .iter()
        .filter_map(|region| match region.kind {
            crate::RegionKind::Comment => Some((region.span.start, region.span.end, false)),
            crate::RegionKind::Doctype => Some((region.span.start, region.span.end, true)),
            _ => None,
        })
        .collect();
    comments.sort_unstable();
    // CDATA sections are text inside foreign content (the text path already
    // covers them) but a **bogus comment** in HTML content, where nothing else
    // picks them up.  They take part in the same "earliest wins" ordering.
    let mut cdatas: Vec<(u32, u32)> = index
        .opaque_regions
        .iter()
        .filter(|region| region.kind == crate::RegionKind::Cdata)
        .map(|region| (region.span.start, region.span.end))
        .collect();
    cdatas.sort_unstable();
    let mut di = 0usize;
    let mut ci = 0usize;

    let mut texts = TextRuns::new(index).peekable();
    tree.skip_until = doctype_skip_until;
    let mut ti = 0usize;
    while ti < tokens.tags.len()
        || texts.peek().is_some()
        || ci < comments.len()
        || di < cdatas.len()
    {
        // Drop everything the raw-text rescan swallowed.
        if tree.skip_until > 0 {
            while texts
                .peek()
                .is_some_and(|run| run.span.start < tree.skip_until)
            {
                texts.next();
            }
            if ti < tokens.tags.len() && tokens.tags[ti].span.start < tree.skip_until {
                ti += 1;
                continue;
            }
            tree.skip_until = 0;
            if ti >= tokens.tags.len() && texts.peek().is_none() {
                break;
            }
        }
        // The earliest of the next tag, the next text run and the next comment
        // region wins, so document order is preserved.
        let next_comment = comments.get(ci).map(|&(start, _, _)| start);
        let next_cdata = cdatas.get(di).map(|&(start, _)| start);
        let earliest = [
            (ti < tokens.tags.len()).then(|| tokens.tags[ti].span.start),
            texts.peek().map(|run| run.span.start),
            next_comment,
            next_cdata,
        ]
        .into_iter()
        .flatten()
        .min();
        if next_cdata.is_some() && next_cdata == earliest && next_cdata != next_comment {
            let (start, end) = cdatas[di];
            di += 1;
            // `<![CDATA[...]]>` is character data in foreign content and a
            // bogus comment in HTML content.  CDATA regions are excluded from
            // the text runs, so this is the only place that inserts them.
            match super::cdata_inner(tree.input, &(start..end)) {
                Some(inner) if tree.in_foreign() => tree.insert_text(inner, true),
                _ => {
                    // A bogus comment's data is everything after `<!` up to the
                    // `>` that ends it, which the region span includes.
                    let end = if tree.input[end as usize - 1] == b'>' {
                        end - 1
                    } else {
                        end
                    };
                    tree.insert_comment(start + 2..end, false);
                }
            }
            continue;
        }
        if next_comment.is_some() && next_comment == earliest {
            let (start, end, is_doctype) = comments[ci];
            ci += 1;
            // A comment does not go through the insertion modes, so the pending
            // table characters have to be flushed here.
            tree.flush_table_text();
            // A processing instruction that hits EOF inside its *target* emits
            // nothing (`<?`, `<?hey`); one that was converted to a bogus
            // comment (`<?xml ...`, `<?#`) is emitted like any other comment.
            let dropped_pi = tree.input[start as usize..end as usize].starts_with(b"<?")
                && crate::token_data::pi_form(tree.input, start..end)
                    == crate::token_data::PiForm::Drop;
            if is_doctype || !dropped_pi {
                tree.insert_comment(start..end, is_doctype);
            }
            continue;
        }
        let take_tag = match texts.peek() {
            None => true,
            Some(run) => ti < tokens.tags.len() && tokens.tags[ti].span.start <= run.span.start,
        };
        if take_tag {
            let tok = ti;
            // A handler may request reprocessing, which is the spec's
            // "reprocess the token" and the reason this loop is not a match.
            // Bounded: the specification's reprocess chains are a handful of
            // steps, and an unbounded loop turns a rule bug into a hang.
            for _ in 0..32 {
                let step = tree.process_tag(tok);
                match step {
                    Step::Done => break,
                    Step::Reprocess => continue,
                }
            }
            ti += 1;
        } else {
            let run = texts.next().expect("peeked");
            tree.process_text(run);
        }
    }
    // End of file: the pending "in table text" characters are flushed *before*
    // the EOF token, because the EOF rules close the stack and the foster
    // location is computed from it (`<template><template><table>Foo` has to put
    // `Foo` inside the inner template, before the table).
    tree.flush_table_text();
    tree.eof();

    // End of file: close everything still open.
    while tree.stack.len() > 1 {
        tree.pop_current();
    }

    // The tree must be acyclic: a cycle here would make `emit` recurse forever,
    // so it is an invariant check rather than a timeout.
    let mut budget = tree.nodes.len() + 1;
    let _ = tree.emit_checked(builder, 0, &mut budget);
    let mut flags = DocumentFlags::empty();
    if tree.implied_end {
        flags |= DocumentFlags::IMPLIED_END;
    }
    let mut overrides = vec![0u8; tokens.tags.len()];
    for (tag, name) in tree.adjusted {
        overrides[tag as usize] = name;
    }
    (flags, tree.warnings, overrides, tree.merges)
}

/// `RepairMode::None`: one node per start tag, a matching end tag closes it,
/// void elements close immediately.  No implied anything.
fn build_without_repair(
    input: &[u8],
    tokens: &Tokens,
    index: &MarkupIndex,
    builder: &mut TreeBuilder,
) -> (DocumentFlags, Vec<Warning>) {
    let mut stack: Vec<u32> = Vec::new();
    let mut texts = TextRuns::new(index).peekable();
    let mut ti = 0usize;
    let mut warnings = Vec::new();
    while ti < tokens.tags.len() || texts.peek().is_some() {
        let take_tag = match texts.peek() {
            None => true,
            Some(run) => ti < tokens.tags.len() && tokens.tags[ti].span.start <= run.span.start,
        };
        if !take_tag {
            let run = texts.next().expect("peeked");
            builder.insert_characters(run);
            continue;
        }
        let token = &tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let name_start = tokens.tag_name_start[ti] as usize;
        let name_end = name_start + tokens.tag_name_len[ti] as usize;
        let name = &input[name_start..name_end];
        if matches!(token.kind, TagKind::Start) {
            builder.start_tag(tag, span.clone(), super::raw_mode_namespace(name));
            if SpecTree::is_void(name) {
                builder.pop_open_element(span.end..span.end, false);
            } else {
                stack.push(tag);
            }
        } else if let Some(top) = stack.last().copied() {
            let top_name = {
                let start = tokens.tag_name_start[top as usize] as usize;
                let end = start + tokens.tag_name_len[top as usize] as usize;
                &input[start..end]
            };
            if top_name.eq_ignore_ascii_case(name) {
                stack.pop();
                builder.pop_open_element(span.clone(), false);
            } else {
                warnings.push(Warning {
                    kind: WarningKind::UnexpectedEndTag,
                    span,
                });
            }
        } else {
            warnings.push(Warning {
                kind: WarningKind::UnexpectedEndTag,
                span,
            });
        }
        ti += 1;
    }
    while !stack.is_empty() {
        stack.pop();
        builder.pop_open_element(input.len() as u32..input.len() as u32, true);
    }
    (DocumentFlags::empty(), warnings)
}

impl SpecTree<'_> {
    // -- tree primitives ---------------------------------------------------

    fn push_node(
        &mut self,
        kind: Kind,
        name: Option<Name>,
        span: Range<u32>,
        raw_text: bool,
    ) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            kind,
            name,
            parent: NO_LINK,
            first_child: NO_LINK,
            last_child: NO_LINK,
            next_sibling: NO_LINK,
            prev_sibling: NO_LINK,
            ns: crate::NS_HTML,
            end: span.end,
            span,
            raw_text,
        });
        id
    }

    fn current(&self) -> u32 {
        *self.stack.last().expect("document is always open")
    }

    /// Inserts an element under the current node and pushes it.
    fn insert_element(&mut self, name: Name, span: Range<u32>) -> u32 {
        if matches!(name, Name::Implied(_)) {
            self.implied_end = true;
        }
        let mut name_buf = [0u8; 32];
        let own_name: Option<&[u8]> = match name {
            Name::Token(tag) => Some(lower_into(&mut name_buf, self.input, self.tokens, tag)),
            Name::Implied(_) => None,
        };
        // Not `take`: foster parenting stays enabled for the whole token, so the
        // elements `reconstruct_afe` creates land in the same place.
        let foster = self.foster_next_insert;
        // Foster parenting inserts *before* the table (or inside the open
        // template), not at the current node's end.
        let (parent, foster_before) = if foster && self.foster_target() {
            self.foster_location()
        } else {
            (self.current(), None)
        };
        let parent_ns = self.current_namespace();
        // An integration point is a foreign element whose *children* are
        // parsed as HTML; everything else keeps the parent's namespace, and
        // `svg`/`math` start a foreign subtree from HTML.  A MathML text
        // integration point hands its children back to HTML too, except for
        // `mglyph`/`malignmark`, which the dispatcher keeps in MathML.
        let own = own_name.unwrap_or(b"");
        let parent_is_html_point = if self.is_math_text_integration_point(parent) {
            !(own.eq_ignore_ascii_case(b"mglyph") || own.eq_ignore_ascii_case(b"malignmark"))
        } else {
            self.is_html_integration_point(parent)
        };
        let mut element_ns = if parent_is_html_point {
            crate::NS_HTML
        } else {
            parent_ns
        };
        // `<svg>`/`<math>` pick their namespace by name, even inside foreign
        // content: `<math><annotation-xml><svg>` yields an SVG element (the WPT
        // expected tree prints `svg svg`).
        if let Some(own) = own_name {
            if own == b"svg" {
                element_ns = crate::NS_SVG;
            } else if own == b"math" {
                element_ns = crate::NS_MATHML;
            }
        }
        if element_ns == crate::NS_SVG
            && let (Name::Token(tag), Some(own)) = (name, own_name)
            && let Some(index) = super::svg_tag_adjustment_lower(own)
        {
            self.adjusted.push((tag, index));
        }
        let id = self.push_node(Kind::Element, Some(name), span.clone(), false);
        self.nodes[id as usize].parent = parent;
        // The tape records the *child routing* on the element: the two
        // integration-point kinds are annotated so a text node can ask which
        // rules produced it (NUL handling, CDATA recognition) without
        // re-deriving the name tables.  `Node::namespace` maps them back.
        self.nodes[id as usize].ns = element_ns;
        match foster_before {
            Some(anchor) => self.insert_before(parent, id, anchor),
            None => self.append_child(parent, id),
        }
        self.stack.push(id);
        self.ns.push(element_ns);
        id
    }

    /// End-of-file token.
    ///
    /// Several modes answer EOF by "anything else": synthesise the missing
    /// structural element and reprocess.  A document that ends in `after head`
    /// (or inside a table, or mid-raw-text) therefore still gets its implied
    /// `body`, which is exactly the difference between our tree and the
    /// expected one for the largest raw-caliber failure bucket.
    fn eof(&mut self) {
        for _ in 0..8 {
            // "In body": an end-of-file token is processed by the "in template"
            // rules whenever the stack of template insertion modes is not
            // empty.  That is what makes `<template><div>` still get a body.
            if !self.template_modes.is_empty() && !matches!(self.mode, Mode::InTemplate) {
                let mut found = None;
                for index in (0..self.stack.len()).rev() {
                    if self.is(self.stack[index], b"template") {
                        found = Some(index);
                        break;
                    }
                }
                match found {
                    Some(index) => {
                        self.pop_until(index);
                        if let Some(marker) = self.last_marker() {
                            self.afe.truncate(marker);
                        }
                        self.template_modes.pop();
                        self.reset_mode();
                        continue;
                    }
                    None => {
                        self.template_modes.clear();
                        continue;
                    }
                }
            }
            match self.mode {
                Mode::Initial | Mode::BeforeHtml => {
                    self.insert_element(Name::Implied("html"), 0..0);
                    self.mode = Mode::BeforeHead;
                }
                Mode::BeforeHead => {
                    self.insert_element(Name::Implied("head"), 0..0);
                    self.mode = Mode::InHead;
                }
                Mode::InHeadNoscript => {
                    // EOF in "in head noscript": pop the noscript and let the
                    // in-head walk continue.
                    if self.is(self.current(), b"noscript") {
                        self.pop_current();
                    }
                    self.mode = Mode::InHead;
                }
                Mode::InHead => {
                    if self.is(self.current(), b"head") {
                        self.pop_current();
                    }
                    self.mode = Mode::AfterHead;
                }
                Mode::AfterHead => {
                    self.insert_element(Name::Implied("body"), 0..0);
                    self.mode = Mode::InBody;
                }
                Mode::Text => {
                    self.pop_current();
                    self.mode = self.text_return;
                }
                Mode::InTemplate => {
                    let mut found = None;
                    for index in (0..self.stack.len()).rev() {
                        if self.is(self.stack[index], b"template") {
                            found = Some(index);
                            break;
                        }
                    }
                    match found {
                        Some(index) => {
                            self.pop_until(index);
                            if let Some(marker) = self.last_marker() {
                                self.afe.truncate(marker);
                            }
                            self.template_modes.pop();
                            self.reset_mode();
                        }
                        None => break,
                    }
                }
                Mode::InTableText => {
                    // "In table text" has a single rule: flush the pending
                    // tokens, then reprocess.  EOF flushes too.
                    self.in_table_text(0);
                }
                Mode::InTable
                | Mode::InTableBody
                | Mode::InRow
                | Mode::InCell
                | Mode::InCaption
                | Mode::InColumnGroup => self.mode = Mode::InBody,
                _ => break,
            }
        }
    }

    /// Inserts a comment/doctype node.
    ///
    /// Doctypes are appended to the document node (their mode is `initial`);
    /// comments go to the current node, which is the document before `html`
    /// exists.
    fn insert_comment(&mut self, span: Range<u32>, is_doctype: bool) {
        // A DOCTYPE token is only inserted by the "initial" mode; every other
        // mode answers it with "Parse error. Ignore the token."
        if is_doctype {
            if !matches!(self.mode, Mode::Initial) {
                return;
            }
            self.quirks = crate::token_data::doctype_is_quirks(self.input, span.clone());
            // "Then switch the insertion mode to before html" - so a *second*
            // DOCTYPE is ignored (only the initial mode inserts one), and the
            // next token does not count as "anything else".
            self.mode = Mode::BeforeHtml;
        }
        // Comments do not always land on the current node: `after body` says
        // "insert a comment given (the first element in the stack of open
        // elements, null)" - the `html` element - and both `after after body`
        // and `after after frameset` say "given (the Document object, null)".
        let parent = match self.mode {
            Mode::AfterBody => self.stack.get(1).copied().unwrap_or(0),
            Mode::AfterAfterBody | Mode::AfterAfterFrameset => 0,
            _ => self.current(),
        };
        let kind = if is_doctype {
            Kind::Doctype
        } else {
            Kind::Comment
        };
        let id = self.push_node(kind, None, span, false);
        self.append_child(parent, id);
    }

    /// "In table text": flush the pending character tokens.
    ///
    /// Called from the EOF token and from every token that bypasses the mode
    /// handlers (comments, doctypes, CDATA): their insertion position must come
    /// *after* the pending characters, or `<table>  <!--foo-->` puts the comment
    /// first.
    fn flush_table_text(&mut self) {
        if self.mode == Mode::InTableText {
            self.mode = self.table_text_return;
        }
        if self.table_text.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.table_text);
        let visible = pending.iter().any(|(span, _)| {
            self.input[span.start as usize..span.end as usize]
                .iter()
                .any(|&byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
        });
        for (span, raw) in pending {
            if visible {
                // "reprocess ... using the rules given in the 'anything else'
                // entry in the 'in table' insertion mode": foster parenting plus
                // the `in body` character rules, which *reconstruct the active
                // formatting elements* first (`<table><a>1<td>2</td>3` puts `3`
                // in a rebuilt `a`, and that `a` is fostered too).
                self.foster_and_insert_text(&TextRun {
                    span: span.clone(),
                    raw_text: raw,
                });
            } else {
                self.insert_text(span, raw);
            }
        }
    }

    fn insert_text(&mut self, span: Range<u32>, raw_text: bool) {
        if span.is_empty() {
            return;
        }
        let parent = self.current();
        let id = self.push_node(Kind::Text, None, span, raw_text);
        self.append_child(parent, id);
    }

    /// Records a void element: inserted and closed immediately.
    fn insert_void(&mut self, name: Name, span: Range<u32>) -> u32 {
        let id = self.insert_element(name, span);
        self.stack.pop();
        self.ns.pop();
        id
    }

    fn pop_until(&mut self, index: usize) {
        while self.stack.len() > index {
            self.pop_current();
        }
    }

    /// Pops the current element (the caller knows a matching start tag exists).
    fn pop_current(&mut self) {
        if self.stack.len() > 1 {
            self.ns.pop();
            let id = self.stack.pop().expect("open element");
            let end = self.at.max(self.nodes[id as usize].span.end);
            self.nodes[id as usize].end = self.nodes[id as usize].end.max(end);
            // The form element pointer is *not* cleared by popping: `in table`
            // inserts a form and pops it immediately while the pointer keeps
            // pointing at it, which is what makes the second `<form>` ignored.
            // `</form>` clears it explicitly.
        }
    }

    // -- token classification ---------------------------------------------

    fn token_name(&self, tag: u32) -> &[u8] {
        let start = self.tokens.tag_name_start[tag as usize] as usize;
        let end = start + self.tokens.tag_name_len[tag as usize] as usize;
        &self.input[start..end]
    }

    fn node_name(&self, id: u32) -> Option<&[u8]> {
        match self.nodes[id as usize].name? {
            Name::Token(tag) => {
                let start = self.tokens.tag_name_start[tag as usize] as usize;
                let end = start + self.tokens.tag_name_len[tag as usize] as usize;
                Some(&self.input[start..end])
            }
            Name::Implied(name) => Some(name.as_bytes()),
        }
    }

    fn is(&self, id: u32, lower: &[u8]) -> bool {
        self.node_name(id)
            .is_some_and(|name| name.eq_ignore_ascii_case(lower))
    }

    fn is_void(name: &[u8]) -> bool {
        matches!(name.len(), 2..=8)
            && [
                &b"area"[..],
                b"base",
                b"basefont",
                b"bgsound",
                b"br",
                b"col",
                b"embed",
                b"frame",
                b"hr",
                b"img",
                b"input",
                b"keygen",
                b"link",
                b"meta",
                b"param",
                b"source",
                b"track",
                b"wbr",
            ]
            .iter()
            .any(|void| name.eq_ignore_ascii_case(void))
    }

    /// Base scope boundary for the element at `index`, by namespace.
    ///
    /// HTML elements use the standard's default scope list; foreign integration
    /// points are boundaries too (html5ever's `default_scope`), which is what
    /// keeps `<p><svg><desc><p>` from closing the outer p.
    fn is_scope_boundary(&self, index: usize, extra: &[&[u8]]) -> bool {
        let id = self.stack[index];
        let ns = self.ns.get(index).copied().unwrap_or(crate::NS_HTML);
        let named = |names: &[&[u8]]| names.iter().any(|name| self.is(id, name));
        let base = match ns {
            crate::NS_SVG => named(&[b"foreignobject", b"desc", b"title"]),
            crate::NS_MATHML => named(&[b"mi", b"mo", b"mn", b"ms", b"mtext", b"annotation-xml"]),
            _ => named(&[
                b"applet",
                b"caption",
                b"html",
                b"table",
                b"td",
                b"th",
                b"marquee",
                b"object",
                b"template",
            ]),
        };
        base || named(extra)
    }

    /// `has X in table scope`: the table scope walk stops only at
    /// `html`, `table` and `template` (not at `td`/`th`/`caption`), which is
    /// what lets `</table>` inside a cell close the table.
    fn table_scope_index(&self, target: &[&[u8]]) -> Option<usize> {
        for index in (0..self.stack.len()).rev() {
            let id = self.stack[index];
            if target.iter().any(|name| self.is(id, name)) {
                return Some(index);
            }
            if self.is(id, b"html") || self.is(id, b"table") || self.is(id, b"template") {
                return None;
            }
        }
        None
    }

    fn has_in_table_scope(&self, target: &[&[u8]]) -> bool {
        self.table_scope_index(target).is_some()
    }

    /// `has X in scope`: walks the open stack until a boundary.
    fn has_in_scope(&self, target: &[&[u8]], extra: &[&[u8]]) -> bool {
        self.stack_index(target, extra).is_some()
    }

    fn stack_index(&self, target: &[&[u8]], extra: &[&[u8]]) -> Option<usize> {
        for (index, &id) in self.stack.iter().enumerate().rev() {
            if target.iter().any(|name| self.is(id, name)) {
                return Some(index);
            }
            if self.is_scope_boundary(index, extra) {
                return None;
            }
        }
        None
    }

    /// The specification's *special* category, as the stack walks see it.
    ///
    /// `SPECIAL` is the HTML name list; the foreign integration points are
    /// special too (that is what keeps `<x><search></x>` from closing `x`).
    fn special_at(&self, index: usize) -> bool {
        let id = self.stack[index];
        if SPECIAL.iter().any(|name| self.is(id, name)) {
            return true;
        }
        let ns = self.ns.get(index).copied().unwrap_or(crate::NS_HTML);
        let named = |names: &[&[u8]]| names.iter().any(|name| self.is(id, name));
        match ns {
            crate::NS_SVG => named(&[b"foreignobject", b"desc", b"title"]),
            crate::NS_MATHML => named(&[b"mi", b"mo", b"mn", b"ms", b"mtext", b"annotation-xml"]),
            _ => false,
        }
    }

    /// The `li`/`dd`/`dt` start tag walk: from the current node upwards, stop at
    /// the first matching name, or at a special element that is not
    /// `address`/`div`/`p` ("Otherwise, if node is in the special category, but
    /// is not an address, div, or p element, then jump to the step labeled
    /// done").
    fn list_item_index(&self, target: &[&[u8]]) -> Option<usize> {
        for index in (0..self.stack.len()).rev() {
            let id = self.stack[index];
            if target.iter().any(|name| self.is(id, name)) {
                return Some(index);
            }
            if self.special_at(index)
                && !(self.is(id, b"address") || self.is(id, b"div") || self.is(id, b"p"))
            {
                return None;
            }
        }
        None
    }

    /// "Any other end tag": same walk, but *any* special element stops it.
    fn any_other_end_tag_index(&self, name: &[u8]) -> Option<usize> {
        for index in (0..self.stack.len()).rev() {
            let id = self.stack[index];
            if self.is(id, name) {
                return Some(index);
            }
            if index > 0 && self.special_at(index) {
                return None;
            }
        }
        None
    }

    /// "Generate all implied end tags thoroughly": the `dd`/`dt`/`li`/`p`/... set
    /// plus the table parts (`caption`, `colgroup`, `tbody`, `td`, `tfoot`,
    /// `th`, `thead`, `tr`).
    fn generate_implied_end_tags_thoroughly(&mut self) {
        const THOROUGH: [&[u8]; 17] = [
            b"caption",
            b"colgroup",
            b"dd",
            b"dt",
            b"li",
            b"optgroup",
            b"option",
            b"p",
            b"rb",
            b"rp",
            b"rt",
            b"rtc",
            b"tbody",
            b"td",
            b"tfoot",
            b"th",
            b"thead",
        ];
        while self.stack.len() > 1 {
            let current = self.current();
            let matched =
                THOROUGH.iter().any(|name| self.is(current, name)) || self.is(current, b"tr");
            if !matched {
                return;
            }
            self.implied_end = true;
            self.pop_current();
        }
    }

    /// "If the stack of open elements has a p element in button scope, then
    /// close a p element."  Half a dozen `in body` entries start with it.
    fn close_p_in_button_scope(&mut self) {
        if let Some(index) = self.stack_index(&[b"p"], BUTTON_EXTRA) {
            self.generate_implied_end_tags();
            self.pop_until(index);
        }
    }

    /// Whether any open element has this name (the spec's "on the stack of open
    /// elements" phrasing, e.g. for the `form`/`html` template checks).
    fn has_open(&self, name: &[u8]) -> bool {
        self.stack.iter().any(|&id| self.is(id, name))
    }

    /// "Generate implied end tags": pop `dd`, `dt`, `li`, `optgroup`,
    /// `option`, `p`, `rb`, `rp`, `rt`, `rtc`.
    fn generate_implied_end_tags(&mut self) {
        loop {
            let current = self.current();
            let implied = [
                &b"dd"[..],
                b"dt",
                b"li",
                b"optgroup",
                b"option",
                b"p",
                b"rb",
                b"rp",
                b"rt",
                b"rtc",
            ]
            .iter()
            .any(|name| self.is(current, name));
            if !implied || self.stack.len() == 1 {
                return;
            }
            self.implied_end = true;
            self.pop_current();
        }
    }

    // -- active formatting elements + adoption agency ----------------------

    fn stack_pos(&self, id: u32) -> Option<usize> {
        self.stack.iter().position(|&open| open == id)
    }

    fn afe_pos(&self, id: u32) -> Option<usize> {
        self.afe
            .iter()
            .rposition(|entry| matches!(entry, Afe::Element(open) if *open == id))
    }

    fn last_marker(&self) -> Option<usize> {
        self.afe
            .iter()
            .rposition(|entry| matches!(entry, Afe::Marker))
    }

    /// Last formatting element after the last marker with this tag name.
    fn afe_last(&self, name: &[u8]) -> Option<usize> {
        let start = self.last_marker().map_or(0, |marker| marker + 1);
        self.afe[start..]
            .iter()
            .rposition(|entry| matches!(entry, Afe::Element(id) if self.is(*id, name)))
            .map(|offset| start + offset)
    }

    fn is_in_afe(&self, id: u32) -> bool {
        let Some(marker) = self.last_marker() else {
            return self.afe_pos(id).is_some();
        };
        self.afe[marker + 1..]
            .iter()
            .any(|entry| matches!(entry, Afe::Element(open) if *open == id))
    }

    fn is_special(&self, id: u32) -> bool {
        SPECIAL.iter().any(|name| self.is(id, name))
    }

    /// Whether two elements have the same tag name, namespace and attributes.
    ///
    /// This is the Noah's Ark comparison: the attributes are compared "as
    /// dictionaries" - names case-insensitively, values exactly, order ignored.
    fn same_element(&self, a: u32, b: u32) -> bool {
        let (Some(Name::Token(tag_a)), Some(Name::Token(tag_b))) =
            (self.nodes[a as usize].name, self.nodes[b as usize].name)
        else {
            return false;
        };
        if !self
            .token_name(tag_a)
            .eq_ignore_ascii_case(self.token_name(tag_b))
        {
            return false;
        }
        let range_a = self.tokens.tags[tag_a as usize].attr_range.clone();
        let range_b = self.tokens.tags[tag_b as usize].attr_range.clone();
        if range_a.len() != range_b.len() {
            return false;
        }
        // Every attribute of `a` must appear in `b` with the same value.
        range_a.clone().all(|attr| {
            let start = self.tokens.attr_name_start[attr as usize] as usize;
            let len = self.tokens.attr_name_len[attr as usize] as usize;
            let name = &self.input[start..start + len];
            let value_start = self.tokens.attr_value_start[attr as usize] as usize;
            let value_len = self.tokens.attr_value_len[attr as usize] as usize;
            let value = &self.input[value_start..value_start + value_len];
            range_b.clone().any(|other| {
                let other_start = self.tokens.attr_name_start[other as usize] as usize;
                let other_len = self.tokens.attr_name_len[other as usize] as usize;
                if !self.input[other_start..other_start + other_len].eq_ignore_ascii_case(name) {
                    return false;
                }
                let other_value_start = self.tokens.attr_value_start[other as usize] as usize;
                let other_value_len = self.tokens.attr_value_len[other as usize] as usize;
                self.input[other_value_start..other_value_start + other_value_len] == *value
            })
        })
    }

    /// "Reconstruct the active formatting elements".
    fn reconstruct_afe(&mut self) {
        let Some(last) = self.afe.len().checked_sub(1) else {
            return;
        };
        if matches!(self.afe[last], Afe::Marker) {
            return;
        }
        if let Afe::Element(id) = self.afe[last]
            && self.stack_pos(id).is_some()
        {
            return;
        }
        let mut index = last;
        while index > 0 {
            match self.afe[index - 1] {
                Afe::Marker => break,
                Afe::Element(id) if self.stack_pos(id).is_none() => index -= 1,
                _ => break,
            }
        }
        while index < self.afe.len() {
            let Afe::Element(id) = self.afe[index] else {
                index += 1;
                continue;
            };
            let Some(Name::Token(tag)) = self.nodes[id as usize].name else {
                index += 1;
                continue;
            };
            let span = self.nodes[id as usize].span.clone();
            let new = self.insert_element(Name::Token(tag), span);
            self.afe[index] = Afe::Element(new);
            index += 1;
        }
    }

    /// Pushes a formatting element, with Noah's Ark (at most three equal names).
    fn push_formatting(&mut self, id: u32) {
        self.afe.push(Afe::Element(id));
        let start = self.last_marker().map_or(0, |marker| marker + 1);
        // Noah's Ark clause: at most three entries with the same tag name,
        // namespace *and attributes* may sit after the last marker; the earliest
        // is dropped when a fourth arrives.
        let mut same: Vec<usize> = Vec::new();
        for (index, entry) in self.afe.iter().enumerate().skip(start) {
            let Afe::Element(other) = *entry else {
                continue;
            };
            if self.same_element(other, id) {
                same.push(index);
            }
        }
        if same.len() > 3 {
            self.afe.remove(same[0]);
        }
    }

    /// Unlinks a node from its parent's child list (its own children stay).
    fn detach(&mut self, id: u32) {
        let parent = self.nodes[id as usize].parent;
        if parent == NO_LINK {
            return;
        }
        let prev = self.nodes[id as usize].prev_sibling;
        let next = self.nodes[id as usize].next_sibling;
        if prev == NO_LINK {
            self.nodes[parent as usize].first_child = next;
        } else {
            self.nodes[prev as usize].next_sibling = next;
        }
        if next == NO_LINK {
            self.nodes[parent as usize].last_child = prev;
        } else {
            self.nodes[next as usize].prev_sibling = prev;
        }
        self.nodes[id as usize].prev_sibling = NO_LINK;
        self.nodes[id as usize].next_sibling = NO_LINK;
        self.nodes[id as usize].parent = NO_LINK;
    }

    /// Appends `id` as the last child of `parent`.
    fn append_child(&mut self, parent: u32, id: u32) {
        let last = self.nodes[parent as usize].last_child;
        self.nodes[id as usize].parent = parent;
        self.nodes[id as usize].prev_sibling = last;
        self.nodes[id as usize].next_sibling = NO_LINK;
        if last == NO_LINK {
            self.nodes[parent as usize].first_child = id;
        } else {
            self.nodes[last as usize].next_sibling = id;
        }
        self.nodes[parent as usize].last_child = id;
    }

    /// Inserts `id` immediately before `anchor` among `parent`'s children.
    fn insert_before(&mut self, parent: u32, id: u32, anchor: u32) {
        let prev = self.nodes[anchor as usize].prev_sibling;
        self.nodes[id as usize].parent = parent;
        self.nodes[id as usize].prev_sibling = prev;
        self.nodes[id as usize].next_sibling = anchor;
        self.nodes[anchor as usize].prev_sibling = id;
        if prev == NO_LINK {
            self.nodes[parent as usize].first_child = id;
        } else {
            self.nodes[prev as usize].next_sibling = id;
        }
    }

    /// Moves every child of `from` to the end of `to`'s children (the adoption
    /// agency's reparenting step, in O(1) links instead of a clone-and-append).
    fn take_children(&mut self, from: u32, to: u32) {
        let first = self.nodes[from as usize].first_child;
        let last = self.nodes[from as usize].last_child;
        if first == NO_LINK {
            return;
        }
        let anchor = self.nodes[to as usize].last_child;
        if anchor == NO_LINK {
            self.nodes[to as usize].first_child = first;
        } else {
            self.nodes[anchor as usize].next_sibling = first;
        }
        self.nodes[first as usize].prev_sibling = anchor;
        self.nodes[to as usize].last_child = last;
        let mut child = first;
        while child != NO_LINK {
            self.nodes[child as usize].parent = to;
            child = self.nodes[child as usize].next_sibling;
        }
        self.nodes[from as usize].first_child = NO_LINK;
        self.nodes[from as usize].last_child = NO_LINK;
    }

    fn clone_element(&mut self, id: u32) -> u32 {
        let name = self.nodes[id as usize].name.expect("element");
        let span = self.nodes[id as usize].span.clone();
        self.push_node(Kind::Element, Some(name), span, false)
    }

    /// "The appropriate place for inserting a node" relative to `target`.
    fn insert_at_appropriate_place(&mut self, target: u32, id: u32) {
        if self.is(target, b"table")
            || self.is(target, b"tbody")
            || self.is(target, b"tfoot")
            || self.is(target, b"thead")
            || self.is(target, b"tr")
        {
            let table = self
                .stack
                .iter()
                .rev()
                .copied()
                .find(|&open| self.is(open, b"table"));
            if let Some(table) = table {
                let parent = self.nodes[table as usize].parent;
                if parent != u32::MAX {
                    self.insert_before(parent, id, table);
                    return;
                }
            }
        }
        self.append_child(target, id);
    }

    /// The adoption agency algorithm (`tree-construction__parsing.txt`).
    fn adoption_agency(&mut self, subject: &[u8], span: Range<u32>) -> Step {
        // Step 2: the current node already is the subject.
        let current = self.current();
        if self.is(current, subject) && !self.is_in_afe(current) {
            self.pop_current();
            return Step::Done;
        }

        for _ in 0..8 {
            let Some(afe_index) = self.afe_last(subject) else {
                return self.any_other_end(subject, span);
            };
            let Afe::Element(formatting) = self.afe[afe_index] else {
                unreachable!("afe_last returns elements");
            };
            let Some(formatting_pos) = self.stack_pos(formatting) else {
                self.afe.remove(afe_index);
                return Step::Done;
            };
            // "not in scope": a scope boundary between the top and the element.
            if (formatting_pos + 1..self.stack.len())
                .any(|index| self.is_scope_boundary(index, &[]))
            {
                return Step::Done;
            }

            // "The topmost node in the stack that is lower in the stack than
            // formattingElement": topmost means closest to the top, i.e. the
            // *first* special element below it (html5ever: skip(index).find()).
            let Some(furthest_pos) = (formatting_pos + 1..self.stack.len())
                .find(|&pos| self.is_special(self.stack[pos]))
            else {
                self.pop_until(formatting_pos);
                self.afe_remove(formatting);
                return Step::Done;
            };
            let furthest = self.stack[furthest_pos];
            let common_ancestor = self.stack[formatting_pos - 1];
            let mut bookmark = afe_index;

            let mut node_pos = furthest_pos;
            let mut last_node = furthest;
            let mut inner = 0;
            loop {
                inner += 1;
                if node_pos == 0 {
                    break;
                }
                node_pos -= 1;
                let node = self.stack[node_pos];
                if node == formatting {
                    break;
                }
                let mut afe_pos = self.afe_pos(node);
                if inner > 3
                    && let Some(pos) = afe_pos
                {
                    self.afe.remove(pos);
                    if pos < bookmark {
                        bookmark -= 1;
                    }
                    afe_pos = None;
                }
                let Some(afe_pos) = afe_pos else {
                    self.stack.remove(node_pos);
                    self.ns.remove(node_pos);
                    continue;
                };
                let clone = self.clone_element(node);
                self.afe[afe_pos] = Afe::Element(clone);
                self.stack[node_pos] = clone;
                if last_node == furthest {
                    bookmark = afe_pos + 1;
                }
                self.detach(last_node);
                self.append_child(clone, last_node);
                last_node = clone;
            }

            self.detach(last_node);
            self.insert_at_appropriate_place(common_ancestor, last_node);

            // "Create an element for the token for which formattingElement was
            // created ... Take all of the child nodes of furthestBlock and append
            // them to the element created in the last step.  Append that new
            // element to furthestBlock."
            let new = self.clone_element(formatting);
            // "Take all of the child nodes of furthestBlock and append them to
            // the element created in the last step" - one link splice.
            self.take_children(furthest, new);
            self.append_child(furthest, new);

            self.afe_remove(formatting);
            let bookmark = bookmark.min(self.afe.len());
            self.afe.insert(bookmark, Afe::Element(new));

            if let Some(pos) = self.stack_pos(formatting) {
                self.stack.remove(pos);
                self.ns.remove(pos);
            }
            // "... insert the new element into the stack of open elements
            // immediately below the position of furthestBlock."
            if let Some(pos) = self.stack_pos(furthest) {
                self.stack.insert(pos + 1, new);
                self.ns.insert(pos + 1, crate::NS_HTML);
            }
            // Step 20: the outer loop repeats (eight iterations at most).
        }
        Step::Done
    }

    fn afe_remove(&mut self, id: u32) {
        if let Some(pos) = self.afe_pos(id) {
            self.afe.remove(pos);
        }
    }

    /// "Any other end tag" (`in body`): walk the stack from the current node.
    ///
    /// A special node encountered before the match blocks the token, which is
    /// why `<menuitem><p></menuitem>` leaves the p (and its following text)
    /// alone.
    fn any_other_end(&mut self, name: &[u8], span: Range<u32>) -> Step {
        let mut index = self.stack.len();
        while index > 1 {
            index -= 1;
            let node = self.stack[index];
            if self.is(node, name) {
                // "Generate implied end tags, except for HTML elements with the
                // same tag name as the token."
                self.generate_implied_end_tags_except(name);
                self.pop_until(index);
                return Step::Done;
            }
            if self.is_special(node) {
                self.warnings.push(Warning {
                    kind: WarningKind::UnexpectedEndTag,
                    span,
                });
                return Step::Done;
            }
        }
        self.warnings.push(Warning {
            kind: WarningKind::UnexpectedEndTag,
            span,
        });
        Step::Done
    }

    /// "Generate implied end tags, except for HTML elements with this name".
    fn generate_implied_end_tags_except(&mut self, name: &[u8]) {
        loop {
            let current = self.current();
            if self.is(current, name) || self.stack.len() == 1 {
                return;
            }
            let implied = [
                &b"dd"[..],
                b"dt",
                b"li",
                b"optgroup",
                b"option",
                b"p",
                b"rb",
                b"rp",
                b"rt",
                b"rtc",
            ]
            .iter()
            .any(|candidate| self.is(current, candidate));
            if !implied {
                return;
            }
            self.implied_end = true;
            self.pop_current();
        }
    }

    // -- dispatch ---------------------------------------------------------

    fn process_tag(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let start = matches!(token.kind, TagKind::Start);
        // `<pre>`/`<listing>` only skip the newline that *immediately* follows.
        self.skip_next_newline = false;
        self.at = span.start;

        // "In foreign content": while the current node's namespace is not HTML,
        // its own rules apply before the insertion mode's.
        if self.in_foreign() && self.mode != Mode::Text {
            let mut name_buf = [0u8; 32];
            let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
            // The dispatcher: a MathML text integration point uses HTML rules
            // for everything except `mglyph`/`malignmark` start tags; an HTML
            // integration point uses HTML rules unconditionally.
            let foreign = if self.is_math_text_integration_point(self.current()) {
                start && (name == b"mglyph" || name == b"malignmark")
            } else {
                !self.is_html_integration_point(self.current())
            };
            if foreign {
                if start {
                    let breakout = BREAKOUT.contains(&name)
                        || (name == b"font" && self.font_has_breaking_attribute(tag));
                    if breakout {
                        // "Pop elements from the stack until the current node is an
                        // HTML element, then reprocess the token."
                        while self.at_foreign_element() && self.stack.len() > 1 {
                            self.pop_current();
                        }
                        return Step::Reprocess;
                    }
                    self.insert_element(Name::Token(tag), span);
                    // "If the token has its self-closing flag set, pop the current
                    // node off the stack of open elements" - in foreign content
                    // only; the HTML rules ignore the flag.
                    if token.self_closing {
                        self.pop_current();
                    }
                    return Step::Done;
                }
                if name == b"br" || name == b"p" {
                    while self.at_foreign_element() && self.stack.len() > 1 {
                        self.pop_current();
                    }
                    return Step::Reprocess;
                }
                // "An end tag whose tag name is subject": pop up to and including the
                // nearest open element with that name.
                if let Some(pos) = self.stack_index(&[name], &[]) {
                    self.pop_until(pos);
                }
                return Step::Done;
            }
        }

        // "Process the token using the rules for the in head insertion mode"
        // applies to template tokens from every mode.
        if self.token_name(tag).eq_ignore_ascii_case(b"template")
            && matches!(
                self.mode,
                Mode::InTable
                    | Mode::InTableBody
                    | Mode::InRow
                    | Mode::InCell
                    | Mode::InCaption
                    | Mode::InColumnGroup
                    | Mode::AfterBody
                    | Mode::AfterAfterBody
            )
        {
            return self.in_head(ti);
        }

        match self.mode {
            Mode::Initial => {
                // Comments/doctype never reach here: the markup scan masks them.
                // "Anything else ... set the Document to quirks mode."
                self.quirks = true;
                self.mode = Mode::BeforeHtml;
                Step::Reprocess
            }
            Mode::BeforeHtml => {
                if start && self.token_is(tag, b"html") {
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::BeforeHead;
                    return Step::Done;
                }
                if !start {
                    self.warnings.push(Warning {
                        kind: WarningKind::UnexpectedEndTag,
                        span: span.clone(),
                    });
                }
                if !start
                    && !self.token_is(tag, b"head")
                    && !self.token_is(tag, b"body")
                    && !self.token_is(tag, b"html")
                    && !self.token_is(tag, b"br")
                {
                    return Step::Done;
                }
                self.insert_element(Name::Implied("html"), span.start..span.start);
                self.mode = Mode::BeforeHead;
                Step::Reprocess
            }
            Mode::BeforeHead => {
                if start && self.token_is(tag, b"head") {
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InHead;
                    return Step::Done;
                }
                if start && self.token_is(tag, b"html") {
                    // "Process the token using the rules for the in body
                    // insertion mode": that is where the attributes merge.
                    return self.in_body(ti);
                }
                if !start {
                    self.warnings.push(Warning {
                        kind: WarningKind::UnexpectedEndTag,
                        span: span.clone(),
                    });
                }
                if !start
                    && !self.token_is(tag, b"head")
                    && !self.token_is(tag, b"body")
                    && !self.token_is(tag, b"html")
                    && !self.token_is(tag, b"br")
                {
                    return Step::Done;
                }
                self.insert_element(Name::Implied("head"), span.start..span.start);
                self.mode = Mode::InHead;
                Step::Reprocess
            }
            Mode::InHead => self.in_head(ti),
            Mode::InHeadNoscript => self.in_head_noscript(ti),
            Mode::AfterHead => self.after_head(ti),
            Mode::InBody => self.in_body(ti),
            Mode::Text => self.text_mode(ti),
            Mode::AfterBody => self.after_body(ti),
            Mode::AfterAfterBody => {
                if start && self.token_is(tag, b"html") {
                    return Step::Done;
                }
                self.mode = Mode::InBody;
                Step::Reprocess
            }
            Mode::InTableText => self.in_table_text(ti),
            Mode::InTable => self.in_table(ti),
            Mode::InTableBody => self.in_table_body(ti),
            Mode::InRow => self.in_row(ti),
            Mode::InCell => self.in_cell(ti),
            Mode::InCaption => self.in_caption(ti),
            Mode::InColumnGroup => self.in_column_group(ti),
            Mode::InFrameset => self.in_frameset(ti),
            Mode::AfterFrameset => self.after_frameset(ti),
            Mode::AfterAfterFrameset => self.after_after_frameset(ti),
            Mode::InTemplate => self.in_template(ti),
        }
    }

    fn token_is(&self, tag: u32, lower: &[u8]) -> bool {
        self.token_name(tag).eq_ignore_ascii_case(lower)
    }

    fn text_mode(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        // In `text`, only the matching end tag does anything.
        if matches!(token.kind, TagKind::End) {
            let current = self.current();
            let matches_text = match self.nodes[current as usize].name {
                Some(Name::Token(tag)) => self.token_is(ti as u32, self.token_name(tag)),
                Some(Name::Implied(name)) => self
                    .token_name(ti as u32)
                    .eq_ignore_ascii_case(name.as_bytes()),
                None => false,
            };
            if matches_text {
                self.pop_current();
                self.mode = self.text_return;
                return Step::Done;
            }
        }
        Step::Done
    }

    /// The "in head noscript" insertion mode (`parsing-main-inheadnoscript`).
    fn in_head_noscript(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::Start) {
            match name {
                b"html" => return self.in_body(ti),
                b"basefont" | b"bgsound" | b"link" | b"meta" | b"noframes" | b"style" => {
                    return self.in_head(ti);
                }
                // "Parse error. Ignore the token."
                b"head" | b"noscript" => return Step::Done,
                _ => {}
            }
        } else if name == b"noscript" {
            // "Pop the current node (which will be a noscript element) ... and
            // switch to in head."
            self.pop_current();
            self.mode = Mode::InHead;
            return Step::Done;
        } else if name == b"br" {
            // `</br>` acts like the "anything else" entry below.
        } else {
            // Any other end tag: parse error, ignore.
            return Step::Done;
        }
        // Anything else: pop the noscript and reprocess in `in head`.
        self.pop_current();
        self.mode = Mode::InHead;
        Step::Reprocess
    }

    fn after_body(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        if matches!(token.kind, TagKind::End) && self.token_is(ti as u32, b"html") {
            self.mode = Mode::AfterAfterBody;
            return Step::Done;
        }
        self.mode = Mode::InBody;
        Step::Reprocess
    }

    fn in_head(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let start = matches!(token.kind, TagKind::Start);
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);

        if start {
            match name {
                b"html" => return self.in_body(ti),
                b"base" | b"basefont" | b"bgsound" | b"link" | b"meta" => {
                    self.insert_void(Name::Token(tag), span);
                    return Step::Done;
                }
                b"title" => {
                    self.insert_element(Name::Token(tag), span);
                    self.text_return = self.mode;
                    self.mode = Mode::Text;
                    return Step::Done;
                }
                b"template" => {
                    self.insert_element(Name::Token(tag), span);
                    self.afe.push(Afe::Marker);
                    // "Set the frameset-ok flag to not ok".
                    self.frameset_ok = false;
                    self.template_modes.push(Mode::InTemplate);
                    self.mode = Mode::InTemplate;
                    return Step::Done;
                }
                b"noscript" => {
                    // Scripting is disabled: insert the element and switch to
                    // the "in head noscript" mode, which is *not* the in-head
                    // rules - it is a mode of its own.
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InHeadNoscript;
                    return Step::Done;
                }
                b"script" => {
                    self.insert_element(Name::Token(tag), span.clone());
                    // Script data has escape states, so the end of the element
                    // is rescanned here rather than taken from the scanner's
                    // first `</script`.
                    let content_start = span.end as usize;
                    let end = script_data_end(self.input, content_start) as u32;
                    self.insert_text(content_start as u32..end, true);
                    // Everything up to the end of the scanned region is
                    // consumed: the content was inserted above, and an
                    // unterminated `</script` tail is dropped rather than kept.
                    let region_end = self
                        .regions
                        .binary_search_by_key(&(content_start as u32), |region| region.span.start)
                        .map(|index| self.regions[index].span.end)
                        .unwrap_or(end);
                    self.skip_until = end.max(span.end).max(region_end);
                    self.text_return = self.mode;
                    self.mode = Mode::Text;
                    return Step::Done;
                }
                b"style" | b"noframes" => {
                    self.insert_element(Name::Token(tag), span);
                    self.text_return = self.mode;
                    self.mode = Mode::Text;
                    return Step::Done;
                }
                _ => {}
            }
        } else if name == b"template" {
            self.close_template(span);
            return Step::Done;
        } else if name == b"head" {
            self.pop_current();
            self.mode = Mode::AfterHead;
            return Step::Done;
        } else if matches!(name, b"body" | b"html" | b"br") {
            self.pop_current();
            self.mode = Mode::AfterHead;
            return Step::Reprocess;
        }
        // "Anything else": pop the head, switch to after head, reprocess.

        self.pop_current();
        self.mode = Mode::AfterHead;
        Step::Reprocess
    }

    fn after_head(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let start = matches!(token.kind, TagKind::Start);
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);

        if start {
            match name {
                b"html" => return self.in_body(ti),
                b"body" => {
                    self.insert_element(Name::Token(tag), span);
                    // A body created here starts the document body proper, so a
                    // later `<frameset>` must not replace it (the WPT expected
                    // trees follow this; the standard leaves the flag set and
                    // would let the frameset win).
                    self.frameset_ok = false;
                    self.mode = Mode::InBody;
                    return Step::Done;
                }
                b"base" | b"basefont" | b"bgsound" | b"link" | b"meta" | b"noframes"
                | b"script" | b"style" | b"title" | b"template" => {
                    // "Parse error. Push the node pointed to by the head
                    // element pointer onto the stack of open elements, process
                    // the token using the in head rules, then remove that node
                    // from the stack."  Without pushing the head back, the
                    // content is inserted at the html level instead of in head.
                    // The head element pointer: the first `head` element in the
                    // tree, whether it was explicit or implied.
                    let total = self.nodes.len() as u32;
                    let head = (0..total).find(|&id| self.is(id, b"head"));
                    match head {
                        Some(id) => {
                            self.stack.push(id);
                            self.ns.push(crate::NS_HTML);
                            let step = self.in_head(ti);
                            // "Remove that node from the stack of open
                            // elements" - it is not necessarily the current
                            // node: a raw text element opened inside the pushed
                            // head (`<style>`, `<title>`) sits above it, and
                            // for those the mode is now `text`, which pops the
                            // raw element and expects the head to be gone.
                            if let Some(index) = self.stack.iter().rposition(|&open| open == id) {
                                self.stack.remove(index);
                                self.ns.remove(index);
                            }
                            return step;
                        }
                        None => return self.in_head(ti),
                    }
                }
                _ => {}
            }
        } else if matches!(name, b"body" | b"html" | b"br") {
            self.insert_element(Name::Implied("body"), span.start..span.start);
            self.mode = Mode::InBody;
            return Step::Reprocess;
        } else {
            return Step::Done;
        }

        self.insert_element(Name::Implied("body"), span.start..span.start);
        self.mode = Mode::InBody;
        Step::Reprocess
    }

    fn in_body(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        if matches!(token.kind, TagKind::End) {
            self.at = span.end;
        }
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);

        if matches!(token.kind, TagKind::Start) {
            if name == b"html" {
                // "Parse error.  If there is a template element on the stack of
                // open elements, then ignore the token.  Otherwise, for each
                // attribute on the token, if the current node does not have it,
                // add it to the html element."
                if self.has_open(b"template") {
                    return Step::Done;
                }
                if let Some(existing) = self.bottom_element(b"html") {
                    self.merges.push((existing, tag));
                }
                return Step::Done;
            }
            if name == b"body" && self.has_in_scope(&[b"body"], &[]) {
                if let Some(existing) = self.bottom_element(b"body") {
                    self.merges.push((existing, tag));
                }
                self.frameset_ok = false;
                return Step::Done;
            }
            // `frameset-ok` bookkeeping (`in body`).
            let hidden_input = name == b"input" && self.has_attr_value(tag, b"type", b"hidden");
            if self.frameset_ok && !hidden_input && !keeps_frameset_ok(name) {
                self.frameset_ok = false;
            }
            if name == b"frameset" && !self.frameset_ok {
                return Step::Done;
            }
            if CLOSES_P.contains(&name)
                && self.has_in_scope(&[b"p"], BUTTON_EXTRA)
                && let Some(index) = self.stack_index(&[b"p"], BUTTON_EXTRA)
            {
                self.pop_until(index);
            }
            if name == b"hr" {
                // "If the stack has a p in button scope, close p.  If the stack
                // has a select element in scope: generate implied end tags.
                // Insert an HTML element for the token.  Immediately pop it.
                // Set frameset-ok to not ok."
                if let Some(index) = self.stack_index(&[b"p"], BUTTON_EXTRA) {
                    self.generate_implied_end_tags();
                    self.pop_until(index);
                }
                if self.has_in_scope(&[b"select"], &[]) {
                    self.generate_implied_end_tags();
                }
                self.insert_void(Name::Token(tag), span);
                self.frameset_ok = false;
                return Step::Done;
            }
            // "A start tag whose tag name is one of: caption, col, colgroup,
            // frame, head, tbody, td, tfoot, th, thead, tr: parse error, ignore
            // the token."  `col` and `frame` are in the void list, so the
            // generic void path below would insert them; the ignore rule wins
            // in body.
            if name == b"col" || name == b"frame" {
                return Step::Done;
            }
            // "A start tag whose tag name is image: parse error.  Change the
            // token's tag name to img and reprocess it."  The token keeps its
            // attributes; only the reported name changes.
            if name == b"image" {
                self.adjusted.push((tag, super::image_alias_index()));
            }
            if name == b"input" && self.has_in_scope(&[b"select"], &[]) {
                // "Parse error.  Repeatedly pop the current node off the stack
                // of open elements until a select element has been popped."
                if let Some(index) = self.stack_index(&[b"select"], &[]) {
                    self.pop_until(index);
                }
            }
            if Self::is_void(name) || name == b"image" {
                self.insert_void(Name::Token(tag), span);
                return Step::Done;
            }
            // "If the list of active formatting elements contains an `a`
            // element between the end of the list and the last marker ... run
            // the adoption agency algorithm for the token, then remove that
            // element from the list and stack if it is still there."
            if name == b"a" && self.afe_last(b"a").is_some() {
                let _ = self.adoption_agency(b"a", span.clone());
                if let Some(index) = self.afe_last(b"a") {
                    let old = match self.afe[index] {
                        Afe::Element(old) => old,
                        Afe::Marker => unreachable!("afe_last returns elements"),
                    };
                    self.afe.remove(index);
                    if let Some(pos) = self.stack_pos(old) {
                        self.stack.remove(pos);
                        self.ns.remove(pos);
                    }
                }
            }
            if name == b"nobr" {
                // "Reconstruct the active formatting elements.  If the stack has
                // a nobr in scope: parse error, run the adoption agency for
                // nobr, and reconstruct again."  This entry sits before the
                // generic formatting entry on purpose.
                self.reconstruct_afe();
                if self.has_in_scope(&[b"nobr"], &[]) {
                    let _ = self.adoption_agency(b"nobr", span.clone());
                    self.reconstruct_afe();
                }
                let id = self.insert_element(Name::Token(tag), span);
                self.push_formatting(id);
                return Step::Done;
            }
            if FORMATTING.contains(&name) {
                self.reconstruct_afe();
                let id = self.insert_element(Name::Token(tag), span);
                self.push_formatting(id);
                return Step::Done;
            }
            if name == b"pre" || name == b"listing" {
                // `pre`/`listing` are in the CLOSES_P list *and* skip the
                // newline that immediately follows, so they are handled before
                // the generic branch.
                if let Some(index) = self.stack_index(&[b"p"], BUTTON_EXTRA) {
                    self.generate_implied_end_tags();
                    self.pop_until(index);
                }
                self.insert_element(Name::Token(tag), span);
                self.skip_next_newline = true;
                return Step::Done;
            }
            if CLOSES_P.contains(&name) {
                // "A start tag whose tag name is one of: address, article, ...":
                // close a p in button scope, then insert.  No reconstruction -
                // that is what keeps <p><b><p> from nesting the second p inside
                // a rebuilt <b>.
                self.insert_element(Name::Token(tag), span);
                return Step::Done;
            }
            match name {
                b"head" | b"caption" | b"col" | b"colgroup" | b"frame" | b"tbody" | b"td"
                | b"tfoot" | b"th" | b"thead" | b"tr" => return Step::Done,
                b"body" => return Step::Done,
                b"li" => {
                    if let Some(index) = self.list_item_index(&[b"li"]) {
                        self.generate_implied_end_tags();
                        self.pop_until(index);
                    }
                    self.close_p_in_button_scope();
                    // No "reconstruct": the `li` entry inserts directly, so a
                    // formatting element that was open inside the previous item
                    // is *not* rebuilt around the new one.
                    self.insert_element(Name::Token(tag), span);
                    return Step::Done;
                }
                b"dd" | b"dt" => {
                    if let Some(index) = self.list_item_index(&[b"dd", b"dt"]) {
                        self.generate_implied_end_tags();
                        self.pop_until(index);
                    }
                    self.close_p_in_button_scope();
                    self.insert_element(Name::Token(tag), span);
                    return Step::Done;
                }
                b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
                    // "If the stack of open elements has a p element in button
                    // scope, then close a p element."  (`h1`-`h6` are *not* in
                    // the address/article list, so they close their own p.)
                    if let Some(index) = self.stack_index(&[b"p"], BUTTON_EXTRA) {
                        self.generate_implied_end_tags();
                        self.pop_until(index);
                    }
                    // "If the current node is an HTML element whose tag name is
                    // one of h1-h6, parse error; pop the current node."
                    let current = self.current();
                    let headings: &[&[u8]] = &[b"h1", b"h2", b"h3", b"h4", b"h5", b"h6"];
                    if headings.iter().any(|name| self.is(current, name)) {
                        self.pop_current();
                    }
                    self.insert_element(Name::Token(tag), span);
                    return Step::Done;
                }
                b"form" => {
                    // "If the form element pointer is not null, and there is no
                    // template element on the stack of open elements, then parse
                    // error; ignore the token."
                    if self.form.is_some() && !self.has_open(b"template") {
                        return Step::Done;
                    }
                    self.close_p_in_button_scope();
                    let id = self.insert_element(Name::Token(tag), span);
                    self.form = Some(id);
                    return Step::Done;
                }
                b"button" => {
                    if let Some(index) = self.stack_index(&[b"button"], &[]) {
                        self.generate_implied_end_tags();
                        self.pop_until(index);
                    }
                    self.reconstruct_afe();
                    let id = self.insert_element(Name::Token(tag), span);
                    self.frameset_ok = false;
                    let _ = id;
                    return Step::Done;
                }
                b"nobr" => {
                    // "Reconstruct the active formatting elements.  If the stack
                    // of open elements has a nobr element in scope, parse error,
                    // run the adoption agency for nobr, and reconstruct again."
                    self.reconstruct_afe();
                    if self.has_in_scope(&[b"nobr"], &[]) {
                        let _ = self.adoption_agency(b"nobr", span.clone());
                        self.reconstruct_afe();
                    }
                    let id = self.insert_element(Name::Token(tag), span);
                    self.push_formatting(id);
                    return Step::Done;
                }
                b"xmp" => {
                    // close p, reconstruct, then raw text.
                    if let Some(index) = self.stack_index(&[b"p"], BUTTON_EXTRA) {
                        self.generate_implied_end_tags();
                        self.pop_until(index);
                    }
                    self.reconstruct_afe();
                    self.frameset_ok = false;
                    self.insert_element(Name::Token(tag), span);
                    self.text_return = self.mode;
                    self.mode = Mode::Text;
                    return Step::Done;
                }
                b"iframe" | b"noembed" => {
                    self.frameset_ok = false;
                    self.insert_element(Name::Token(tag), span);
                    self.text_return = self.mode;
                    self.mode = Mode::Text;
                    return Step::Done;
                }
                b"plaintext" => {
                    // "Close a p element.  Insert an HTML element for the token.
                    // Switch the tokenizer to the PLAINTEXT state."  There is no
                    // reconstruct and **no mode switch**: the tree builder stays
                    // in `in body`, so the character tokens that follow are
                    // processed by the `in body` rules and reconstruct the
                    // formatting elements *inside* the plaintext element
                    // (`<p><a><plaintext>b` -> `plaintext > a > "b"`).
                    self.close_p_in_button_scope();
                    self.insert_element(Name::Token(tag), span);
                    return Step::Done;
                }
                b"ruby" | b"rb" | b"rtc" | b"rp" | b"rt" => {
                    // Two different rules share this arm:
                    // `ruby`/`rb`/`rtc` generate implied end tags, while
                    // `rt`/`rp` generate them *except for rtc* (so an open
                    // `rtc` keeps collecting `rt` children).  Running both - or
                    // the plain one for `rt` - pops the `rtc` too early.
                    if matches!(name, b"rt" | b"rp") {
                        if self.has_in_scope(&[b"ruby"], &[]) {
                            self.generate_implied_end_tags_except(b"rtc");
                        }
                    } else if self.has_in_scope(&[b"ruby"], &[]) {
                        self.generate_implied_end_tags();
                    }
                    self.insert_element(Name::Token(tag), span);
                    return Step::Done;
                }
                b"base" | b"basefont" | b"bgsound" | b"link" | b"meta" | b"noframes"
                | b"noscript" | b"script" | b"style" | b"template" | b"title" => {
                    // The specification's in-body entry for these: "process the
                    // token using the rules for the in head insertion mode".
                    return self.in_head(ti);
                }
                b"textarea" => {
                    self.insert_element(Name::Token(tag), span);
                    // "If the next token is a U+000A LINE FEED character token,
                    // ignore that token."
                    self.skip_next_newline = true;
                    self.text_return = Mode::InBody;
                    self.mode = Mode::Text;
                    return Step::Done;
                }
                b"select" => {
                    // "If the stack of open elements has a select element in
                    // scope: parse error, ignore the token, and pop until a
                    // select element has been popped.  Otherwise: reconstruct,
                    // insert, set frameset-ok to not ok."
                    //
                    // There is no "in select" insertion mode: the standard
                    // removed it, so `<select>` accepts any content and the
                    // table rules are not suspended inside it.
                    if self.has_in_scope(&[b"select"], &[]) {
                        if let Some(index) = self.stack_index(&[b"select"], &[]) {
                            self.pop_until(index);
                        }
                        return Step::Done;
                    }
                    self.reconstruct_afe();
                    self.insert_element(Name::Token(tag), span);
                    self.frameset_ok = false;
                    return Step::Done;
                }
                b"option" => {
                    if self.has_in_scope(&[b"select"], &[]) {
                        self.generate_implied_end_tags_except(b"optgroup");
                    } else if self.is(self.current(), b"option") {
                        self.pop_current();
                    }
                    self.reconstruct_afe();
                    self.insert_element(Name::Token(tag), span);
                    return Step::Done;
                }
                b"optgroup" => {
                    if self.has_in_scope(&[b"select"], &[]) {
                        self.generate_implied_end_tags();
                    } else if self.is(self.current(), b"option") {
                        self.pop_current();
                    }
                    self.reconstruct_afe();
                    self.insert_element(Name::Token(tag), span);
                    return Step::Done;
                }
                b"frameset" => {
                    // The stack's first entry is the document node, so the
                    // "second element" is index 2 and the root html is index 1.
                    if !self.frameset_ok || self.stack.len() < 3 || !self.is(self.stack[2], b"body")
                    {
                        return Step::Done;
                    }
                    let body = self.stack[2];
                    self.detach(body);
                    while self.stack.len() > 2 {
                        self.pop_current();
                    }
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InFrameset;
                    return Step::Done;
                }
                b"table" => {
                    // "If the Document is not set to quirks mode, and the stack
                    // of open elements has a p element in button scope, then
                    // close a p element."
                    if !self.quirks
                        && let Some(index) = self.stack_index(&[b"p"], BUTTON_EXTRA)
                    {
                        self.generate_implied_end_tags();
                        self.pop_until(index);
                    }
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InTable;
                    return Step::Done;
                }
                b"marquee" | b"object" | b"applet" => {
                    self.reconstruct_afe();
                    self.insert_element(Name::Token(tag), span);
                    self.afe.push(Afe::Marker);
                    self.frameset_ok = false;
                    return Step::Done;
                }
                _ => {}
            }
            // "Any other start tag": reconstruct, then insert.  `svg`/`math`
            // land here (they are not in any of the lists above), and both of
            // their entries end with "if the token has its self-closing flag
            // set, pop the current node off the stack of open elements".
            self.reconstruct_afe();
            if name == b"svg" || name == b"math" {
                self.insert_element(Name::Token(tag), span);
                if token.self_closing {
                    self.pop_current();
                }
                return Step::Done;
            }
            self.insert_element(Name::Token(tag), span);
            return Step::Done;
        }

        // End tags.
        match name {
            b"template" => self.in_head(ti),
            b"body" => {
                if self.has_in_scope(&[b"body"], &[]) {
                    self.mode = Mode::AfterBody;
                }
                Step::Done
            }
            b"html" => {
                if self.has_in_scope(&[b"body"], &[]) {
                    self.mode = Mode::AfterBody;
                }
                Step::Done
            }
            b"p" => {
                if !self.has_in_scope(&[b"p"], BUTTON_EXTRA) {
                    // insert an element for a p start tag and close it
                    self.insert_void(Name::Implied("p"), span);
                    return Step::Done;
                }
                if let Some(index) = self.stack_index(&[b"p"], BUTTON_EXTRA) {
                    self.generate_implied_end_tags();
                    self.pop_until(index);
                }
                Step::Done
            }
            _ if BLOCK_END.contains(&name) => {
                // "An end tag whose tag name is one of: address, ..., ul" (and
                // button): ignore when nothing matches in scope, otherwise
                // generate implied end tags and pop to it.  This must not go
                // through the 'any other end tag' walk, which a special element
                // in between would block.
                let extra: &[&[u8]] = if name == b"button" { BUTTON_EXTRA } else { &[] };
                if let Some(index) = self.stack_index(&[name], extra) {
                    self.generate_implied_end_tags_except(name);
                    self.pop_until(index);
                }
                Step::Done
            }
            _ if FORMATTING.contains(&name) => self.adoption_agency(name, span),
            b"marquee" | b"object" | b"applet" if self.has_in_scope(&[name], &[]) => {
                self.generate_implied_end_tags();
                if let Some(index) = self.stack_index(&[name], &[]) {
                    self.pop_until(index);
                }
                if let Some(marker) = self.last_marker() {
                    self.afe.truncate(marker);
                }
                Step::Done
            }
            // "An end tag whose tag name is one of: caption, col, colgroup,
            // frame, head, tbody, td, tfoot, th, thead, tr" -> parse error,
            // ignore the token.
            b"caption" | b"col" | b"colgroup" | b"frame" | b"head" | b"tbody" | b"td"
            | b"tfoot" | b"th" | b"thead" | b"tr" => Step::Done,
            b"br" => {
                self.insert_void(Name::Implied("br"), span);
                Step::Done
            }
            b"form" => {
                // "Take the current node off the stack if it is a form element
                // and clear the form element pointer."
                let node = self.current();
                self.form = None;
                if self.is(node, b"form") {
                    self.pop_current();
                }
                Step::Done
            }
            b"li" => self.close_list_item(&[b"li"], LIST_ITEM_EXTRA),
            b"dd" | b"dt" => self.close_list_item(&[b"dd", b"dt"], &[]),
            b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
                let headings: &[&[u8]] = &[b"h1", b"h2", b"h3", b"h4", b"h5", b"h6"];
                if self.has_in_scope(headings, &[]) {
                    self.generate_implied_end_tags();
                    if let Some(index) = self.stack_index(headings, &[]) {
                        self.pop_until(index);
                    }
                }
                Step::Done
            }
            _ => {
                if let Some(index) = self.any_other_end_tag_index(name) {
                    self.generate_implied_end_tags_except(name);
                    self.pop_until(index);
                } else {
                    self.warnings.push(Warning {
                        kind: WarningKind::UnexpectedEndTag,
                        span,
                    });
                }
                Step::Done
            }
        }
    }

    fn close_list_item(&mut self, target: &[&[u8]], boundaries: &[&[u8]]) -> Step {
        if let Some(index) = self.stack_index(target, boundaries) {
            self.generate_implied_end_tags();
            self.pop_until(index);
        }
        Step::Done
    }

    // -- table modes (`in table` and friends) ------------------------------

    fn current_is(&self, names: &[&[u8]]) -> bool {
        names.iter().any(|name| self.is(self.current(), name))
    }

    /// Whether the current node is a table part, i.e. whether foster parenting
    /// actually moves the insertion point.
    ///
    /// Foster parenting is a property of the *insertion location*, not a flag on
    /// the token: once `reconstruct_afe` has created an element, the characters
    /// that follow go inside it rather than next to the table.
    fn foster_target(&self) -> bool {
        let current = self.current();
        [&b"table"[..], b"tbody", b"tfoot", b"thead", b"tr"]
            .iter()
            .any(|name| self.is(current, name))
    }

    /// "The appropriate place for inserting a node", foster-parenting variant.
    ///
    /// Three cases, in this order: inside the last *template* if one is open
    /// more recently than the last table (so fostered content in
    /// `<table><template>` stays inside the template); before the last table;
    /// or inside the `html` element when there is no table at all.
    fn foster_location(&self) -> (u32, Option<u32>) {
        let last_template = self.stack.iter().rposition(|&id| self.is(id, b"template"));
        let last_table = self.stack.iter().rposition(|&id| self.is(id, b"table"));
        if let Some(template) = last_template
            && last_table.is_none_or(|table| template > table)
        {
            return (self.stack[template], None);
        }
        match last_table {
            Some(index) => {
                let table = self.stack[index];
                let parent = self.nodes[table as usize].parent;
                if parent != u32::MAX {
                    (parent, Some(table))
                } else {
                    (*self.stack.get(1).unwrap_or(&0), None)
                }
            }
            None => (*self.stack.get(1).unwrap_or(&0), None),
        }
    }

    fn foster_insert_text(&mut self, span: Range<u32>, raw_text: bool) {
        if span.is_empty() {
            return;
        }
        let (parent, before) = self.foster_location();
        let id = self.push_node(Kind::Text, None, span, raw_text);
        match before {
            Some(anchor) => self.insert_before(parent, id, anchor),
            None => self.append_child(parent, id),
        }
    }

    /// "Clear the stack back to a table context."
    fn clear_to_table_context(&mut self) {
        while !self.current_is(&[b"table", b"template", b"html"]) && self.stack.len() > 1 {
            self.pop_current();
        }
    }

    fn clear_to_table_body_context(&mut self) {
        while !self.current_is(&[b"tbody", b"tfoot", b"thead", b"template", b"html"])
            && self.stack.len() > 1
        {
            self.pop_current();
        }
    }

    fn clear_to_table_row_context(&mut self) {
        while !self.current_is(&[b"tr", b"template", b"html"]) && self.stack.len() > 1 {
            self.pop_current();
        }
    }

    /// "In table text": characters are buffered until a non-character token
    /// arrives; whitespace-only buffers are inserted, anything else is
    /// foster-parented.
    fn in_table_text(&mut self, _ti: usize) -> Step {
        self.flush_table_text();
        Step::Reprocess
    }

    fn in_table(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        let start = matches!(token.kind, TagKind::Start);
        if start {
            match name {
                b"caption" => {
                    self.clear_to_table_context();
                    // "Insert a marker at the end of the list of active
                    // formatting elements."
                    self.afe.push(Afe::Marker);
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InCaption;
                    Step::Done
                }
                b"colgroup" => {
                    self.clear_to_table_context();
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InColumnGroup;
                    Step::Done
                }
                b"col" => {
                    self.clear_to_table_context();
                    self.insert_element(Name::Implied("colgroup"), span.start..span.start);
                    self.mode = Mode::InColumnGroup;
                    Step::Reprocess
                }
                b"tbody" | b"tfoot" | b"thead" => {
                    self.clear_to_table_context();
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InTableBody;
                    Step::Done
                }
                b"td" | b"th" | b"tr" => {
                    self.clear_to_table_context();
                    self.insert_element(Name::Implied("tbody"), span.start..span.start);
                    self.mode = Mode::InTableBody;
                    Step::Reprocess
                }
                b"table" => {
                    // A table inside a table closes the outer one first.
                    if let Some(index) = self.table_scope_index(&[b"table"]) {
                        self.pop_until(index);
                        self.reset_mode();
                        return Step::Reprocess;
                    }
                    Step::Done
                }
                b"style" | b"script" | b"template" => self.in_head(ti),
                b"input" => {
                    if self.has_attr_value(tag, b"type", b"hidden") {
                        self.insert_void(Name::Token(tag), span);
                        Step::Done
                    } else {
                        self.foster_and_reprocess(ti)
                    }
                }
                b"form" => {
                    if self.form.is_none() {
                        let id = self.insert_element(Name::Token(tag), span);
                        self.form = Some(id);
                        self.pop_current();
                    }
                    Step::Done
                }
                _ => self.foster_and_reprocess(ti),
            }
        } else if name == b"table" {
            if let Some(index) = self.table_scope_index(&[b"table"]) {
                self.pop_until(index);
                self.reset_mode();
            }
            Step::Done
        } else if matches!(
            name,
            b"body"
                | b"caption"
                | b"col"
                | b"colgroup"
                | b"html"
                | b"tbody"
                | b"td"
                | b"tfoot"
                | b"th"
                | b"thead"
                | b"tr"
        ) {
            Step::Done
        } else if name == b"template" {
            self.in_head(ti)
        } else {
            self.foster_and_reprocess(ti)
        }
    }

    /// Processes a token with foster parenting enabled: the element is inserted
    /// before the table but the `in body` rules still decide what it is.
    /// "Parse error. Enable foster parenting, process the token using the
    /// rules for the in body insertion mode, and then disable foster
    /// parenting."
    ///
    /// Everything goes through `in_body`: foster parenting only changes where
    /// the element is inserted, and `insert_element` handles that (including the
    /// namespace bookkeeping).
    fn foster_and_reprocess(&mut self, ti: usize) -> Step {
        self.foster_next_insert = true;
        let step = self.in_body(ti);
        self.foster_next_insert = false;
        step
    }

    fn in_table_body(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::Start) {
            match name {
                b"tr" => {
                    self.clear_to_table_body_context();
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InRow;
                    Step::Done
                }
                b"th" | b"td" => {
                    self.clear_to_table_body_context();
                    self.insert_element(Name::Implied("tr"), span.start..span.start);
                    self.mode = Mode::InRow;
                    Step::Reprocess
                }
                b"caption" | b"col" | b"colgroup" | b"tbody" | b"tfoot" | b"thead" => {
                    if self.has_in_table_scope(&[b"tbody", b"thead", b"tfoot"]) {
                        self.clear_to_table_body_context();
                        self.pop_current();
                        self.mode = Mode::InTable;
                        return Step::Reprocess;
                    }
                    Step::Done
                }
                _ => self.in_table(ti),
            }
        } else if matches!(name, b"tbody" | b"tfoot" | b"thead") {
            if let Some(index) = self.stack_index(&[name], &[]) {
                self.clear_to_table_body_context();
                self.pop_until(index);
                self.mode = Mode::InTable;
            }
            Step::Done
        } else if name == b"table" {
            if self.has_in_table_scope(&[b"tbody", b"thead", b"tfoot"]) {
                self.clear_to_table_body_context();
                self.pop_current();
                self.mode = Mode::InTable;
                return Step::Reprocess;
            }
            Step::Done
        } else if matches!(
            name,
            b"body" | b"caption" | b"col" | b"colgroup" | b"html" | b"td" | b"th" | b"tr"
        ) {
            Step::Done
        } else {
            self.in_table(ti)
        }
    }

    fn in_row(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::Start) {
            match name {
                b"th" | b"td" => {
                    self.clear_to_table_row_context();
                    self.insert_element(Name::Token(tag), span);
                    self.mode = Mode::InCell;
                    // "Insert a marker at the end of the list of active
                    // formatting elements."
                    self.afe.push(Afe::Marker);
                    Step::Done
                }
                b"caption" | b"col" | b"colgroup" | b"tbody" | b"tfoot" | b"thead" | b"tr" => {
                    if self.has_in_table_scope(&[b"tr"]) {
                        self.clear_to_table_row_context();
                        self.pop_current();
                        self.mode = Mode::InTableBody;
                        return Step::Reprocess;
                    }
                    Step::Done
                }
                _ => self.in_table(ti),
            }
        } else if name == b"tr" {
            if let Some(index) = self.table_scope_index(&[b"tr"]) {
                self.clear_to_table_row_context();
                self.pop_until(index);
                self.mode = Mode::InTableBody;
            }
            Step::Done
        } else if name == b"table" {
            if let Some(index) = self.table_scope_index(&[b"tr"]) {
                self.pop_until(index);
                self.mode = Mode::InTableBody;
                return Step::Reprocess;
            }
            Step::Done
        } else if matches!(name, b"tbody" | b"tfoot" | b"thead") {
            if self.has_in_table_scope(&[name]) && self.has_in_table_scope(&[b"tr"]) {
                self.clear_to_table_row_context();
                self.pop_current();
                self.mode = Mode::InTableBody;
                return Step::Reprocess;
            }
            Step::Done
        } else if matches!(
            name,
            b"body" | b"caption" | b"col" | b"colgroup" | b"html" | b"td" | b"th"
        ) {
            Step::Done
        } else {
            self.in_table(ti)
        }
    }

    fn in_cell(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::End) {
            if matches!(name, b"td" | b"th") {
                if let Some(index) = self.table_scope_index(&[name]) {
                    self.generate_implied_end_tags();
                    self.pop_until(index);
                    // "Clear the list of active formatting elements up to the
                    // last marker" - the cell pushed one, and leaving it in place
                    // makes every later `reconstruct_afe` a no-op.
                    if let Some(marker) = self.last_marker() {
                        self.afe.truncate(marker);
                    }
                    self.mode = Mode::InRow;
                }
                return Step::Done;
            }
            if matches!(name, b"body" | b"caption" | b"col" | b"colgroup" | b"html") {
                return Step::Done;
            }
            if matches!(name, b"table" | b"tbody" | b"tfoot" | b"thead" | b"tr") {
                if self.has_in_table_scope(&[name]) {
                    self.close_cell();
                    return Step::Reprocess;
                }
                return Step::Done;
            }
        } else {
            if matches!(
                name,
                b"caption"
                    | b"col"
                    | b"colgroup"
                    | b"tbody"
                    | b"td"
                    | b"tfoot"
                    | b"th"
                    | b"thead"
                    | b"tr"
            ) && self.has_in_table_scope(&[b"td", b"th"])
            {
                self.close_cell();
                return Step::Reprocess;
            }
        }
        self.in_body(ti)
    }

    fn close_cell(&mut self) {
        self.generate_implied_end_tags();
        // "Pop elements until a td or th has been popped" - a *table* scope
        // walk, so an integration point inside the cell does not stop it.
        if let Some(index) = self.table_scope_index(&[b"td", b"th"]) {
            self.pop_until(index);
        }
        // "Clear the list of active formatting elements up to the last marker."
        if let Some(marker) = self.last_marker() {
            self.afe.truncate(marker);
        }
        self.mode = Mode::InRow;
    }

    fn in_caption(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::End) && name == b"caption" {
            if let Some(index) = self.table_scope_index(&[b"caption"]) {
                self.generate_implied_end_tags();
                self.pop_until(index);
                if let Some(marker) = self.last_marker() {
                    self.afe.truncate(marker);
                }
                self.mode = Mode::InTable;
            }
            return Step::Done;
        }
        let closes = matches!(
            name,
            b"caption"
                | b"col"
                | b"colgroup"
                | b"tbody"
                | b"td"
                | b"tfoot"
                | b"th"
                | b"thead"
                | b"tr"
        ) || (matches!(token.kind, TagKind::End) && name == b"table");
        if closes && self.has_in_table_scope(&[b"caption"]) {
            self.generate_implied_end_tags();
            if let Some(index) = self.table_scope_index(&[b"caption"]) {
                self.pop_until(index);
            }
            self.mode = Mode::InTable;
            return Step::Reprocess;
        }
        if matches!(token.kind, TagKind::End)
            && matches!(
                name,
                b"body"
                    | b"col"
                    | b"colgroup"
                    | b"html"
                    | b"tbody"
                    | b"td"
                    | b"tfoot"
                    | b"th"
                    | b"tr"
            )
        {
            return Step::Done;
        }
        self.in_body(ti)
    }

    fn in_column_group(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::Start) && name == b"col" {
            self.insert_void(Name::Token(tag), span);
            return Step::Done;
        }
        if matches!(token.kind, TagKind::End) && name == b"colgroup" {
            if self.is(self.current(), b"colgroup") {
                self.pop_current();
                self.mode = Mode::InTable;
            }
            return Step::Done;
        }
        if matches!(token.kind, TagKind::End) && name == b"col" {
            return Step::Done;
        }
        if name == b"template" {
            return self.in_head(ti);
        }
        if self.is(self.current(), b"colgroup") {
            self.pop_current();
            self.mode = Mode::InTable;
            return Step::Reprocess;
        }
        Step::Done
    }

    /// "Reset the insertion mode appropriately" (`tree-construction`).
    ///
    /// Walks the open element stack from the current node upwards and picks the
    /// mode its nearest marker element implies.  This is what makes closing a
    /// `</template>` (or a cell, a row, a table) restore the *right* mode rather
    /// than a guess.
    fn reset_mode(&mut self) {
        let mut index = self.stack.len();
        let mut last = false;
        while index > 0 {
            index -= 1;
            let node = self.stack[index];
            if index == 0 {
                last = true;
            }
            if (self.is(node, b"td") || self.is(node, b"th")) && !last {
                self.mode = Mode::InCell;
                return;
            }
            if self.is(node, b"tr") {
                self.mode = Mode::InRow;
                return;
            }
            if self.is(node, b"tbody") || self.is(node, b"thead") || self.is(node, b"tfoot") {
                self.mode = Mode::InTableBody;
                return;
            }
            if self.is(node, b"caption") {
                self.mode = Mode::InCaption;
                return;
            }
            if self.is(node, b"colgroup") {
                self.mode = Mode::InColumnGroup;
                return;
            }
            if self.is(node, b"table") {
                self.mode = Mode::InTable;
                return;
            }
            if self.is(node, b"template") {
                self.mode = self.template_modes.last().copied().unwrap_or(Mode::InBody);
                return;
            }
            if self.is(node, b"head") && !last {
                self.mode = Mode::InHead;
                return;
            }
            if self.is(node, b"body") {
                self.mode = Mode::InBody;
                return;
            }
            if self.is(node, b"html") {
                self.mode = if self.is(node, b"head") {
                    Mode::BeforeHead
                } else {
                    Mode::AfterHead
                };
                return;
            }
            if last {
                self.mode = Mode::InBody;
                return;
            }
        }
        self.mode = Mode::InBody;
    }

    fn close_template(&mut self, span: Range<u32>) {
        // "Let template be the last template element in the stack of open
        // elements" - a plain reverse search, *not* a scope walk: an open `td`
        // would stop the scope walk before reaching the template.
        if let Some(index) = self.stack.iter().rposition(|&id| self.is(id, b"template")) {
            // "Generate all implied end tags thoroughly" - this also closes an
            // open `td`/`tr`/`tbody`, which the ordinary rule leaves open.
            self.generate_implied_end_tags_thoroughly();
            self.pop_until(index);
            if let Some(marker) = self.last_marker() {
                self.afe.truncate(marker);
            }
            self.mode = self
                .template_modes
                .pop()
                .map_or(Mode::InBody, Mode::after_template);
            self.reset_mode();
        }
        let _ = span;
    }

    fn in_template(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::Start) {
            match name {
                b"caption" | b"colgroup" | b"tbody" | b"tfoot" | b"thead" => {
                    self.template_modes.pop();
                    self.template_modes.push(Mode::InTable);
                    self.mode = Mode::InTable;
                    return Step::Reprocess;
                }
                b"col" => {
                    self.template_modes.pop();
                    self.template_modes.push(Mode::InColumnGroup);
                    self.mode = Mode::InColumnGroup;
                    return Step::Reprocess;
                }
                b"tr" => {
                    self.template_modes.pop();
                    self.template_modes.push(Mode::InTableBody);
                    self.mode = Mode::InTableBody;
                    return Step::Reprocess;
                }
                b"td" | b"th" => {
                    self.template_modes.pop();
                    self.template_modes.push(Mode::InRow);
                    self.mode = Mode::InRow;
                    return Step::Reprocess;
                }
                b"base" | b"basefont" | b"bgsound" | b"link" | b"meta" | b"noframes"
                | b"script" | b"style" | b"template" | b"title" => return self.in_head(ti),
                _ => {
                    self.template_modes.pop();
                    self.template_modes.push(Mode::InBody);
                    self.mode = Mode::InBody;
                    return Step::Reprocess;
                }
            }
        }
        if matches!(token.kind, TagKind::End) {
            if name == b"template" {
                self.close_template(span);
                return Step::Done;
            }
            return Step::Done;
        }
        self.in_body(ti)
    }

    fn in_frameset(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let span = token.span.clone();
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::Start) {
            match name {
                b"frameset" => {
                    self.insert_element(Name::Token(tag), span);
                    Step::Done
                }
                b"frame" => {
                    self.insert_void(Name::Token(tag), span);
                    Step::Done
                }
                b"noframes" => self.in_head(ti),
                _ => Step::Done,
            }
        } else if name == b"frameset" {
            // "If the current node is the root html element, parse error, ignore;
            // otherwise pop, and switch to after frameset if the current node is
            // no longer the root html element."
            // `stack[0]` is the document and `stack[1]` the root html, so the
            // root html is the current node only when the stack has length 2.
            if self.stack.len() <= 2 {
                return Step::Done;
            }
            self.pop_current();
            // "If the parser's fragment context element is null and the
            // current node is no longer a frameset element, then switch the
            // insertion mode to after frameset."
            if !self.is(self.current(), b"frameset") {
                self.mode = Mode::AfterFrameset;
            }
            Step::Done
        } else if name == b"noframes" {
            // The standard says "process using the in head rules", whose
            // "anything else" pops the *current* node - which in this mode is
            // the frameset, not the head.  Ignoring the token is what browsers
            // do and what the expected trees show.
            Step::Done
        } else {
            Step::Done
        }
    }

    fn after_frameset(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::End) && name == b"html" {
            self.mode = Mode::AfterAfterFrameset;
            return Step::Done;
        }
        if matches!(token.kind, TagKind::Start) && name == b"noframes" {
            return self.in_head(ti);
        }
        if matches!(token.kind, TagKind::Start) && name == b"html" {
            self.mode = Mode::InBody;
            return Step::Reprocess;
        }
        Step::Done
    }

    fn after_after_frameset(&mut self, ti: usize) -> Step {
        let token = &self.tokens.tags[ti];
        let tag = ti as u32;
        let mut name_buf = [0u8; 32];
        let name = lower_into(&mut name_buf, self.input, self.tokens, tag);
        if matches!(token.kind, TagKind::Start) && name == b"noframes" {
            return self.in_head(ti);
        }
        if matches!(token.kind, TagKind::Start) && name == b"html" {
            self.mode = Mode::InBody;
            return Step::Reprocess;
        }
        Step::Done
    }

    /// The insertion namespace implied by the open elements (`NS_SVG`/`NS_MATH`,
    /// or HTML), following integration points.
    fn current_namespace(&self) -> u8 {
        self.ns.last().copied().unwrap_or(crate::NS_HTML)
    }

    /// Whether the current node is inside SVG/MathML (CDATA sections are only
    /// recognised there; in HTML they are bogus comments).
    /// The first open element with this name (bottom of the stack), as its tag
    /// token, for the `html`/`body` attribute-merge rules.
    fn bottom_element(&self, name: &[u8]) -> Option<u32> {
        // The *node*, not its tag token: an implied `html`/`body` element has no
        // token of its own, and it still has to receive the merged attributes
        // (`<!DOCTYPE html>X</body><html id=x>`).
        self.stack.iter().copied().find(|&id| self.is(id, name))
    }

    /// Whether the token has an attribute whose value matches (ASCII
    /// case-insensitive), e.g. `input type=hidden`.
    fn has_attr_value(&self, tag: u32, name: &[u8], value: &[u8]) -> bool {
        for attr in self.tokens.tags[tag as usize].attr_range.clone() {
            let start = self.tokens.attr_name_start[attr as usize] as usize;
            let len = self.tokens.attr_name_len[attr as usize] as usize;
            if !self.input[start..start + len].eq_ignore_ascii_case(name) {
                continue;
            }
            let vs = self.tokens.attr_value_start[attr as usize] as usize;
            let vl = self.tokens.attr_value_len[attr as usize] as usize;
            return self.input[vs..vs + vl].eq_ignore_ascii_case(value);
        }
        false
    }

    /// Whether a `font` token carries color/face/size (it breaks out of foreign
    /// content only in that case).
    fn font_has_breaking_attribute(&self, tag: u32) -> bool {
        for attr in self.tokens.tags[tag as usize].attr_range.clone() {
            let start = self.tokens.attr_name_start[attr as usize] as usize;
            let len = self.tokens.attr_name_len[attr as usize] as usize;
            let name = &self.input[start..start + len];
            if name.eq_ignore_ascii_case(b"color")
                || name.eq_ignore_ascii_case(b"face")
                || name.eq_ignore_ascii_case(b"size")
            {
                return true;
            }
        }
        false
    }

    /// Whether the current node is in a foreign namespace (CDATA recognition).
    fn in_foreign(&self) -> bool {
        // CDATA sections are recognized when the *adjusted current node* is in a
        // foreign namespace, and an integration point is still a foreign
        // element there (`<svg><foreignObject><![CDATA[x]]>` is text).
        self.current_namespace() != crate::NS_HTML
    }

    /// The breakout loop's condition: an element that is neither an integration
    /// point nor HTML.  `<svg><desc><svg><ul>` stops at `desc`, and the `ul` is
    /// then reprocessed in HTML content inside it.
    fn at_foreign_element(&self) -> bool {
        let current = self.current();
        !(self.is_html_integration_point(current) || self.is_math_text_integration_point(current))
            && self.in_foreign()
    }

    /// Whether the element `id` is an HTML integration point.
    ///
    /// SVG: foreignObject, desc, title.  MathML: annotation-xml when its
    /// encoding is text/html or application/xhtml+xml.  The MathML *text*
    /// integration points are separate (see
    /// [`Self::is_math_text_integration_point`]) because `mglyph`/`malignmark`
    /// stay in MathML there.
    fn is_html_integration_point(&self, id: u32) -> bool {
        let ns = self
            .stack
            .iter()
            .position(|&open| open == id)
            .and_then(|index| self.ns.get(index).copied())
            .unwrap_or_else(|| self.current_namespace());
        if ns == crate::NS_SVG {
            return [&b"foreignobject"[..], b"desc", b"title"]
                .iter()
                .any(|name| self.is(id, name));
        }
        if ns != crate::NS_MATHML {
            return false;
        }
        if !self.is(id, b"annotation-xml") {
            return false;
        }
        match self.nodes[id as usize].name {
            Some(Name::Token(tag)) => {
                self.has_attr_value(tag, b"encoding", b"text/html")
                    || self.has_attr_value(tag, b"encoding", b"application/xhtml+xml")
            }
            _ => false,
        }
    }

    /// Whether the element `id` is a MathML text integration point
    /// (`mi`, `mo`, `mn`, `ms`, `mtext`).
    fn is_math_text_integration_point(&self, id: u32) -> bool {
        [&b"mi"[..], b"mo", b"mn", b"ms", b"mtext"]
            .iter()
            .any(|name| self.is(id, name))
    }

    /// Length of the line feed at `at`, for the `pre`/`listing`/`textarea`
    /// "ignore the next LF" rule.
    ///
    /// The token stream has already been newline-normalised, so a literal CR
    /// (with or without a following LF) *is* the LF character token; a character
    /// reference (`&#x0a;`, `&#10;`, `&NewLine;`) decodes to one as well.
    fn leading_newline_len(&self, at: usize) -> usize {
        match self.input.get(at) {
            Some(b'\n') => 1,
            Some(b'\r') => {
                if self.input.get(at + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                }
            }
            Some(b'&') => self.character_reference_newline_len(at),
            _ => 0,
        }
    }

    /// Length of a character reference that decodes to U+000A, or 0.
    fn character_reference_newline_len(&self, at: usize) -> usize {
        let rest = &self.input[at..];
        if rest.starts_with(b"&NewLine;") {
            return 9;
        }
        // `prefix` is the index of the first digit: `&#x`/`&#X` is three bytes,
        // `&#` two.
        let digits = if rest.starts_with(b"&#x") || rest.starts_with(b"&#X") {
            (3, 16)
        } else if rest.starts_with(b"&#") {
            (2, 10)
        } else {
            return 0;
        };
        let (prefix, radix) = digits;
        let mut value = 0u32;
        let mut index = prefix;
        while let Some(&byte) = rest.get(index) {
            let digit = match (byte as char).to_digit(radix) {
                Some(digit) => digit,
                None => break,
            };
            value = value.saturating_mul(radix).saturating_add(digit);
            index += 1;
        }
        let terminator = rest.get(index).copied();
        let terminated = terminator == Some(b';');
        if value == 0x0a {
            if terminated { index + 1 } else { index }
        } else {
            0
        }
    }

    /// A character token under foster parenting: the `in body` character rules
    /// with foster parenting enabled.
    ///
    /// `reconstruct_afe` runs *first* and its clones are fostered too; once the
    /// current node is no longer a table part, the characters go inside the
    /// clone instead of next to the table.
    fn foster_and_insert_text(&mut self, run: &TextRun) {
        if self.text_clears_frameset(run) {
            self.frameset_ok = false;
        }
        self.foster_next_insert = true;
        self.reconstruct_afe();
        if self.foster_target() {
            self.foster_insert_text(run.span.clone(), run.raw_text);
        } else {
            self.insert_text(run.span.clone(), run.raw_text);
        }
        self.foster_next_insert = false;
    }

    /// Whether a character run contains a character that clears `frameset-ok`.
    ///
    /// Whitespace never does, and neither does U+0000 (the token is ignored in
    /// HTML content and replaced in foreign content, but either way it is not
    /// "any other character token").  A run that needs character-reference or
    /// NUL handling is decoded, which only costs anything when a `&` or a NUL
    /// byte is actually present.
    fn text_clears_frameset(&self, run: &TextRun) -> bool {
        let bytes = &self.input[run.span.start as usize..run.span.end as usize];
        let is_space = |b: &u8| matches!(*b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c);
        if !bytes.contains(&b'&') && !bytes.contains(&0) {
            return bytes.iter().any(|b| !is_space(b));
        }
        let raw = std::str::from_utf8(bytes).expect("Document input is valid UTF-8");
        let decoded = crate::decode_character_references(raw, false);
        decoded
            .chars()
            .any(|c| c != '\0' && !matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}'))
    }

    fn process_text(&mut self, run: TextRun) {
        // `pre`/`listing`: "If the next token is a U+000A LINE FEED character
        // token, ignore that token."
        let run = if std::mem::take(&mut self.skip_next_newline) {
            let skip = self.leading_newline_len(run.span.start as usize);
            if skip > 0 {
                TextRun {
                    span: run.span.start + skip as u32..run.span.end,
                    raw_text: run.raw_text,
                }
            } else {
                run
            }
        } else {
            run
        };
        // A CDATA section of its own is a *region* (handled by `build`), but
        // inside a raw text element the scanner reports the whole content as
        // one region, so `<svg><title><![CDATA[x]]>` arrives here with the
        // markers intact.  Recognize that shape and strip them.
        if self.input[run.span.start as usize..run.span.end as usize].starts_with(b"<![CDATA[") {
            if let Some(inner) = super::cdata_inner(self.input, &run.span) {
                if self.in_foreign() {
                    self.insert_text(inner, true);
                } else {
                    let start = run.span.start + 2;
                    let end = if run.span.end > start + 2
                        && self.input[run.span.end as usize - 1] == b'>'
                    {
                        run.span.end - 1
                    } else {
                        run.span.end
                    };
                    self.insert_comment(start..end, false);
                }
            } else if self.in_foreign() {
                self.insert_text(run.span.start + 9..run.span.end, true);
            } else {
                self.insert_comment(run.span.start + 2..run.span.end, false);
            }
            return;
        }
        // Character tokens are mode-specific: the early modes ignore
        // whitespace, `in head` keeps it in the head, `after head` inserts it
        // before a body exists.  Table foster parenting arrives with R2.
        // "A character token that is U+0000 NULL: parse error.  Ignore the
        // token" - `in body`, i.e. ordinary HTML character data.  Raw text runs
        // (plaintext, CDATA inside a foreign element) are the tokenizer's own
        // states and keep their NULs, so the run is split only for the plain
        // case and `Document::text` decides the rest.
        if self.mode != Mode::Text && !self.in_foreign() && !run.raw_text {
            let bytes = &self.input[run.span.start as usize..run.span.end as usize];
            if bytes.contains(&0) {
                let mut start = run.span.start;
                for (offset, &byte) in bytes.iter().enumerate() {
                    let at = run.span.start + offset as u32;
                    if byte == 0 {
                        if start < at {
                            self.process_text(TextRun {
                                span: start..at,
                                raw_text: run.raw_text,
                            });
                        }
                        start = at + 1;
                    }
                }
                if start < run.span.end {
                    self.process_text(TextRun {
                        span: start..run.span.end,
                        raw_text: run.raw_text,
                    });
                }
                return;
            }
        }
        let whitespace = self
            .input
            .get(run.span.start as usize..run.span.end as usize)
            .is_some_and(|bytes| {
                bytes
                    .iter()
                    .all(|&byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
            });
        // The same "ignore the next line feed" rule, for the runs that reach the
        // per-mode dispatch (the `pre`/`textarea` path above already handled the
        // one that arrives first).
        let skip = if std::mem::take(&mut self.skip_next_newline) {
            self.leading_newline_len(run.span.start as usize)
        } else {
            0
        };
        if skip > 0 {
            self.insert_text(run.span.start + skip as u32..run.span.end, run.raw_text);
            return;
        }
        // Bounded: a mode handler that changes neither the mode nor the tree
        // must not turn a rule bug into a hang.
        for _ in 0..16 {
            match self.mode {
                Mode::Initial | Mode::BeforeHtml | Mode::BeforeHead => {
                    if whitespace {
                        return;
                    }
                    // "Ignore the character token" applies per character in
                    // these modes, so a run like "\n]>" keeps only "]>".
                    let bytes = &self.input[run.span.start as usize..run.span.end as usize];
                    let lead = bytes
                        .iter()
                        .take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
                        .count();
                    if lead > 0 {
                        let rest = TextRun {
                            span: run.span.start + lead as u32..run.span.end,
                            raw_text: run.raw_text,
                        };
                        self.process_text(rest);
                        return;
                    }
                    match self.mode {
                        Mode::Initial => self.mode = Mode::BeforeHtml,
                        Mode::BeforeHtml => {
                            let at = run.span.start;
                            self.insert_element(Name::Implied("html"), at..at);
                            self.mode = Mode::BeforeHead;
                        }
                        _ => {
                            let at = run.span.start;
                            self.insert_element(Name::Implied("head"), at..at);
                            self.mode = Mode::InHead;
                        }
                    }
                }
                Mode::InBody => {
                    // "Any other character token: insert it and set frameset-ok
                    // to not ok" - the foreign content rule says the same, so
                    // the flag is cleared outside HTML content too.  A U+0000
                    // character token is not "any other character": it is
                    // ignored (or replaced), so it leaves the flag alone.
                    if self.text_clears_frameset(&run) {
                        self.frameset_ok = false;
                    }
                    // Characters reconstruct the formatting elements first.
                    self.reconstruct_afe();
                    self.insert_text(run.span, run.raw_text);
                    return;
                }
                Mode::InHead => {
                    // "A character token that is whitespace: insert it.  Any
                    // other character token: pop the head, switch to after head
                    // and reprocess."  The rule is per character, so a mixed run
                    // keeps its leading whitespace in the head.
                    let bytes = &self.input[run.span.start as usize..run.span.end as usize];
                    let ws = bytes
                        .iter()
                        .take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
                        .count();
                    if ws > 0 {
                        self.insert_text(run.span.start..run.span.start + ws as u32, run.raw_text);
                        if ws == bytes.len() {
                            return;
                        }
                        self.pop_current();
                        self.mode = Mode::AfterHead;
                        self.process_text(TextRun {
                            span: run.span.start + ws as u32..run.span.end,
                            raw_text: run.raw_text,
                        });
                        return;
                    }
                    self.pop_current();
                    self.mode = Mode::AfterHead;
                }
                Mode::AfterHead => {
                    if whitespace {
                        self.insert_text(run.span, run.raw_text);
                        return;
                    }
                    let at = run.span.start;
                    self.insert_element(Name::Implied("body"), at..at);
                    self.mode = Mode::InBody;
                }
                Mode::AfterBody | Mode::AfterAfterBody => {
                    if whitespace {
                        // "Process the token using the rules for the in body
                        // insertion mode" - the character lands in the body,
                        // but the mode must not change: a comment after the
                        // whitespace still goes to the `html` element (after
                        // body) or the Document (after after body).
                        self.reconstruct_afe();
                        self.insert_text(run.span, run.raw_text);
                        return;
                    }
                    // Anything else: parse error; switch to in body and
                    // reprocess.
                    self.mode = Mode::InBody;
                }
                Mode::InTemplate => {
                    // "Process the token using the rules for the in body
                    // insertion mode."
                    if !whitespace {
                        self.frameset_ok = false;
                    }
                    self.reconstruct_afe();
                    self.insert_text(run.span, run.raw_text);
                    return;
                }
                Mode::InFrameset | Mode::AfterFrameset | Mode::AfterAfterFrameset => {
                    // "A character token that is one of whitespace: insert it.
                    // Any other character token: parse error, ignore it."  The
                    // rule is per character, so every whitespace byte of the run
                    // is inserted and the rest is dropped (`<frameset> te st`
                    // keeps two spaces).
                    let bytes = &self.input[run.span.start as usize..run.span.end as usize];
                    let mut start = 0usize;
                    while start < bytes.len() {
                        let ws = bytes[start..]
                            .iter()
                            .take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
                            .count();
                        if ws > 0 {
                            let from = run.span.start + start as u32;
                            self.insert_text(from..from + ws as u32, run.raw_text);
                        }
                        start += ws;
                        let skip = bytes[start..]
                            .iter()
                            .take_while(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
                            .count();
                        start += skip;
                    }
                    return;
                }
                Mode::InColumnGroup => {
                    // Characters are handled one at a time in this mode: leading
                    // whitespace is inserted into the colgroup, and the first
                    // non-whitespace character pops it and is reprocessed (which
                    // fosters it).  A run may contain both.
                    let bytes = &self.input[run.span.start as usize..run.span.end as usize];
                    let ws = bytes
                        .iter()
                        .take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
                        .count();
                    if ws > 0 && ws < bytes.len() {
                        self.insert_text(run.span.start..run.span.start + ws as u32, run.raw_text);
                        if self.is(self.current(), b"colgroup") {
                            self.pop_current();
                            self.mode = Mode::InTable;
                        }
                        let rest = run.span.start + ws as u32..run.span.end;
                        self.process_text(TextRun {
                            span: rest,
                            raw_text: run.raw_text,
                        });
                        return;
                    }
                    if whitespace {
                        self.insert_text(run.span, run.raw_text);
                        return;
                    }
                    // "Anything else": pop the colgroup, switch to in table and
                    // reprocess the characters.  When the current node is not a
                    // colgroup there is nothing to pop, so insert here - without
                    // this the loop would spin on the same run forever.
                    if self.is(self.current(), b"colgroup") {
                        self.pop_current();
                        self.mode = Mode::InTable;
                        // ... and reprocess the characters, which in `in table`
                        // become pending table text (and are fostered).
                        continue;
                    }
                    // "Anything else": parse error, ignore the token.
                    return;
                }
                Mode::InHeadNoscript => {
                    if whitespace {
                        self.insert_text(run.span, run.raw_text);
                        return;
                    }
                    // "Anything else": pop the noscript, go back to in head and
                    // reprocess the characters there.
                    if self.is(self.current(), b"noscript") {
                        self.pop_current();
                        self.mode = Mode::InHead;
                        continue;
                    }
                    self.insert_text(run.span, run.raw_text);
                    return;
                }
                Mode::InTable | Mode::InTableBody | Mode::InRow => {
                    if self.current_is(&[b"table", b"tbody", b"tfoot", b"thead", b"tr"]) {
                        // "Set the pending table character tokens to an empty
                        // list, switch to in table text, reprocess."
                        self.table_text_return = self.mode;
                        self.table_text.push((run.span, run.raw_text));
                        self.mode = Mode::InTableText;
                    } else {
                        // The current node is not a table part (e.g. an element
                        // that was foster-parented into place), so the "anything
                        // else" entry applies: foster parenting plus the `in
                        // body` character rules.
                        self.foster_and_insert_text(&run);
                    }
                    return;
                }
                Mode::InTableText => {
                    self.table_text.push((run.span, run.raw_text));
                    return;
                }
                _ => {
                    self.insert_text(run.span, run.raw_text);
                    return;
                }
            }
        }
    }

    // -- materialisation ---------------------------------------------------

    /// Emits `id` in preorder and returns the end offset of its subtree.
    ///
    /// The end is computed bottom-up in the same walk; an earlier version
    /// re-walked each subtree (`O(n^2)`), which cost 209 us/doc in tree build.
    fn emit_checked(&self, builder: &mut TreeBuilder, id: u32, budget: &mut usize) -> u32 {
        if *budget == 0 {
            let head: String =
                String::from_utf8_lossy(&self.input[..self.input.len().min(60)]).replace('\n', " ");
            panic!("tree construction produced a cycle or duplicate: input starts {head:?}");
        }
        *budget -= 1;
        self.emit_inner(builder, id, budget)
    }

    fn emit_inner(&self, builder: &mut TreeBuilder, id: u32, budget: &mut usize) -> u32 {
        let node = &self.nodes[id as usize];
        match node.kind {
            Kind::Document => {
                let mut end = node.span.end;
                let mut child = node.first_child;
                while child != NO_LINK {
                    let next = self.nodes[child as usize].next_sibling;
                    end = end.max(self.emit_checked(builder, child, budget));
                    child = next;
                }
                return end;
            }
            Kind::Text => builder.insert_characters(TextRun {
                span: node.span.clone(),
                raw_text: node.raw_text,
            }),
            Kind::Comment | Kind::Doctype => {
                builder.insert_comment(node.span.clone(), matches!(node.kind, Kind::Doctype))
            }
            Kind::Element => {
                match node.name.expect("element has a name") {
                    Name::Token(tag) => {
                        builder.start_tag(tag, node.span.clone(), node.ns);
                    }
                    Name::Implied(name) => {
                        builder.insert_implied_element(name, node.span.clone(), node.ns);
                    }
                }
                let mut end = node.end;
                let mut child = node.first_child;
                while child != NO_LINK {
                    let next = self.nodes[child as usize].next_sibling;
                    end = end.max(self.emit_checked(builder, child, budget));
                    child = next;
                }
                builder.pop_open_element(end..end, false);
                return end;
            }
        }
        node.span.end
    }
}
