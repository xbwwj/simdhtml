use std::ops::Range;

use super::MarkupIndex;
use super::region::OpaqueRegion;

/// A text run between tags.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextRun {
    pub span: Range<u32>,
    /// Content of a raw text element: character references are not decoded.
    pub raw_text: bool,
}

/// Iterator over the tag spans implied by `tag_open` / `tag_close`, paired in
/// source order.  A start without a close runs to EOF; a close without a start
/// becomes a one-byte span.  Both are left for the tokenizer to judge.
struct TagSpans<'a> {
    starts: &'a [u32],
    ends: &'a [u32],
    input_len: u32,
    i: usize,
    j: usize,
}

impl Iterator for TagSpans<'_> {
    type Item = Range<u32>;

    fn next(&mut self) -> Option<Range<u32>> {
        Some(if self.i >= self.starts.len() {
            let end = self.ends.get(self.j).copied()?;
            self.j += 1;
            end..end + 1
        } else if self.j >= self.ends.len() {
            let start = self.starts[self.i];
            self.i += 1;
            start..self.input_len
        } else if self.starts[self.i] < self.ends[self.j] {
            let span = self.starts[self.i]..self.ends[self.j] + 1;
            self.i += 1;
            self.j += 1;
            span
        } else {
            let end = self.ends[self.j];
            self.j += 1;
            end..end + 1
        })
    }
}

/// Computes maximal text intervals as the complement of tag spans and non-text
/// opaque regions.  Both inputs are in source order, so this is one streaming
/// merge: no span vector, no sort.
pub(super) fn build_text_runs(
    input_len: usize,
    starts: &[u32],
    ends: &[u32],
    raw_spans: &[OpaqueRegion],
) -> Vec<Range<u32>> {
    let input_len = input_len as u32;
    let mut tags = TagSpans {
        starts,
        ends,
        input_len,
        i: 0,
        j: 0,
    };

    let mut runs: Vec<Range<u32>> = Vec::with_capacity(starts.len().min(1024) + 1);
    let mut cursor = 0u32;
    let mut ri = 0usize;
    let mut next_tag = tags.next();

    loop {
        // CDATA is excluded here: whether it becomes a text node depends on tree
        // construction, and keeping it out of the runs stops both paths from
        // inserting the same bytes.
        while ri < raw_spans.len()
            && (raw_spans[ri].kind.is_text() || raw_spans[ri].span.is_empty())
        {
            ri += 1;
        }
        let raw = raw_spans.get(ri).map(|region| region.span.clone());

        let take_raw = match (&raw, &next_tag) {
            (Some(raw), Some(tag)) => raw.start <= tag.start,
            (Some(_), None) => true,
            (None, _) => false,
        };
        let next = if take_raw {
            ri += 1;
            raw.expect("raw span is present")
        } else if next_tag.is_some() {
            let span = next_tag.take().expect("tag span is present");
            next_tag = tags.next();
            span
        } else {
            break;
        };

        if cursor < next.start {
            runs.push(cursor..next.start);
        }
        cursor = cursor.max(next.end);
    }

    if cursor < input_len {
        runs.push(cursor..input_len);
    }
    runs
}

/// Lazy text-run source.
///
/// Tree construction consumes text runs directly in source order; nothing
/// materializes a `Vec<TextRun>`.
pub(crate) struct TextRuns<'a> {
    index: &'a MarkupIndex,
    next: usize,
    raw_cursor: usize,
}

impl<'a> TextRuns<'a> {
    pub(crate) fn new(index: &'a MarkupIndex) -> Self {
        Self {
            index,
            next: 0,
            raw_cursor: 0,
        }
    }
}

impl Iterator for TextRuns<'_> {
    type Item = TextRun;

    fn next(&mut self) -> Option<Self::Item> {
        let run = self.index.text_runs.get(self.next)?.clone();
        self.next += 1;
        while self.raw_cursor < self.index.opaque_regions.len()
            && self.index.opaque_regions[self.raw_cursor].span.end <= run.start
        {
            self.raw_cursor += 1;
        }
        let raw_text = self.raw_cursor < self.index.opaque_regions.len()
            && self.index.opaque_regions[self.raw_cursor].span.start <= run.start
            && self.index.opaque_regions[self.raw_cursor].span.end >= run.end
            && !self.index.opaque_regions[self.raw_cursor]
                .kind
                .decodes_character_references();
        Some(TextRun {
            span: run,
            raw_text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_run_after_tag() {
        let input = r#"<div class="a b" id=x disabled data-q='v'>hi</div>"#;
        let index = crate::markup::scan(input.as_bytes()).unwrap();
        let texts: Vec<_> = TextRuns::new(&index).collect();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].span, 42..44);
    }

    #[test]
    fn rawtext_runs_are_marked_raw() {
        let input = "<script>a &amp; <b></script><p>x</p>";
        let index = crate::markup::scan(input.as_bytes()).unwrap();
        let texts: Vec<_> = TextRuns::new(&index).collect();
        assert!(texts[0].raw_text);
        assert!(!texts[1].raw_text);
    }
}
