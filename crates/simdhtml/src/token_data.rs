//! Token *data* extraction: comments, doctypes and processing instructions.
//!
//! The markup scan answers only "which bytes form a comment / doctype / PI region"; the
//! tokenizer states decide what the *data* of that token is.  Those states are
//! pure byte-level questions with no tree semantics, so they live here and are
//! shared by the DOM and the test harness instead of being re-derived with
//! `strip_prefix` guesses.
//!
//! The states implemented below are the ones in the standard's tokenizer that
//! can change a token's data:
//!
//! * comment start / comment start dash (the abrupt-closing `<!-->` and
//!   `<!--->` forms have empty data),
//! * comment end dash / comment end / comment end bang (`--!>` ends a comment),
//! * the "less than" pair (`<!--` inside a comment is *data*, not a nested
//!   comment; the outer `-->` closes it),
//! * bogus comment (`<!X>` and `<?X>`: data is everything after `<!` / `<?`),
//! * DOCTYPE name / public / system identifiers.

use core::ops::Range;

/// The data bytes of a comment token, as a range into the input.
///
/// `span` covers the whole region including its delimiters (`<!-- ... -->`,
/// `<! ... >`, `<? ... >`).  For comments the data is contiguous: every state
/// either appends the bytes it consumed or ends the token, so the result is a
/// sub-range rather than a rewritten buffer.  Callers that need NUL replacement
/// or newline normalisation apply it to the slice.
pub fn comment_data(input: &[u8], span: Range<u32>) -> Range<u32> {
    let start = span.start as usize;
    let end = (span.end as usize).min(input.len());
    if start >= end {
        return span;
    }
    let body = &input[start..end];
    // `<!--` opens the comment proper; the data starts after it.
    let open = if body.starts_with(b"<!--") {
        4
    } else if body.starts_with(b"<!") || body.starts_with(b"</") || body.starts_with(b"<?") {
        2
    } else {
        return span;
    };
    let rem = &body[open..];
    // Abrupt closing of an empty comment: `<!-->` and `<!--->`.
    if body.starts_with(b"<!--") && (rem == b">" || rem == b"->") {
        let empty = (start + open) as u32;
        return empty..empty;
    }
    // A closed comment ends at `-->`; `--!>` is the comment-end-bang form.
    let data_len = if let Some(tail) = rem.strip_suffix(b"--!>") {
        tail.len()
    } else if let Some(tail) = rem.strip_suffix(b"-->") {
        tail.len()
    } else if rem.ends_with(b">") && !body.starts_with(b"<!--") {
        // Bogus comment: everything up to and including the `>` is consumed,
        // the data stops before it.
        rem.len() - 1
    } else {
        // Unclosed comment: data runs to the end of the region (and thus to the
        // end of input), except that the comment-end-dash/comment-end states
        // hold the trailing `-`s.  `<!--x-` ends in the dash state (one `-`
        // dropped) and `<!--x--` in the end state (two dropped).
        let mut len = rem.len();
        if rem.ends_with(b"--") {
            len -= 2;
        } else if rem.ends_with(b"-") {
            len -= 1;
        }
        len
    };
    let data_start = (start + open) as u32;
    data_start..data_start + data_len as u32
}

/// The data bytes of a processing instruction token.
///
/// The standard tokenizes `<?...>` as a bogus comment whose data is everything
/// after `<?`, so the tree's PI text is `?` followed by that data (the
/// expected-tree format writes `<?data>`).
pub fn pi_data(input: &[u8], span: Range<u32>) -> Range<u32> {
    // The expected trees write a PI as `<?data>`, and a source `<?data?>` has
    // the `?` of `?>` inside its bogus-comment data.  Strip it so both sides
    // agree on the data, per the "tests are the standard" decision.
    let data = comment_data(input, span);
    let mut end = data.end;
    if end > data.start && input[end as usize - 1] == b'?' {
        end -= 1;
    }
    data.start..end
}

/// What a `<?...` region turns into.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PiForm {
    /// A processing instruction token: `target` and `data` ranges.
    Instruction {
        target: Range<u32>,
        data: Range<u32>,
    },
    /// The tokenizer converted the buffer to a bogus comment, whose data is
    /// everything after the `<` (so it starts with the `?`).
    Comment,
    /// End of file inside the processing instruction: no node at all.
    Drop,
}

/// Classifies the processing-instruction-shaped region in `span`.
///
/// Follows the tokenizer's `processing instruction open`, `target`,
/// `after target`, `data` and `questionable` states.  The distinction matters
/// twice: an EOF inside the target emits nothing, and `xml`/`xml-stylesheet`
/// targets (and a target that does not start with a letter or `_`) are turned
/// into bogus comments instead of instructions.
pub fn pi_form(input: &[u8], span: Range<u32>) -> PiForm {
    let start = span.start as usize;
    let end = (span.end as usize).min(input.len());
    if !input[start..end].starts_with(b"<?") {
        return PiForm::Comment;
    }
    let mut i = start + 2;
    let Some(&first) = input.get(i) else {
        // EOF right after `<?`.
        return PiForm::Drop;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return PiForm::Comment;
    }
    let target_start = i;
    while input
        .get(i)
        .is_some_and(|&byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        i += 1;
    }
    let target = target_start as u32..i as u32;
    let Some(&next) = input.get(i) else {
        // EOF in the target state: nothing is emitted.
        return PiForm::Drop;
    };
    // `Processing instruction target state`: only whitespace, `?` and `>`
    // end a valid target.  Anything else is an invalid target, and the whole
    // buffer becomes a bogus comment.
    if !matches!(next, b'\t' | b'\n' | 0x0c | b'\r' | b' ' | b'?' | b'>') {
        return PiForm::Comment;
    }
    let disallowed = {
        let name = &input[target_start..i];
        name.eq_ignore_ascii_case(b"xml") || name.eq_ignore_ascii_case(b"xml-stylesheet")
    };
    if disallowed {
        return PiForm::Comment;
    }
    // `After processing instruction target state`: whitespace is ignored.
    if matches!(next, b'\t' | b'\n' | 0x0c | b'\r' | b' ') {
        i += 1;
        while input
            .get(i)
            .is_some_and(|&byte| matches!(byte, b'\t' | b'\n' | 0x0c | b'\r' | b' '))
        {
            i += 1;
        }
    }
    // `Processing instruction data state`: `?>` ends it, a lone `>` also ends
    // it, and a `?` not followed by `>` is part of the data.
    let data_start = i;
    let data_end = loop {
        match input.get(i) {
            // EOF in the data/questionable state emits no token at all.
            None => return PiForm::Drop,
            Some(b'>') => break i,
            Some(b'?') => {
                if input.get(i + 1) == Some(&b'>') {
                    break i;
                }
                i += 1;
            }
            Some(_) => i += 1,
        }
    };
    PiForm::Instruction {
        target,
        data: data_start as u32..data_end as u32,
    }
}

/// The three identifiers of a DOCTYPE token.
#[derive(Clone, Debug, Default)]
pub struct DoctypeParts {
    pub name: Range<u32>,
    pub public: Option<Range<u32>>,
    pub system: Option<Range<u32>>,
    /// Set on any parse error in the DOCTYPE (the standard's force-quirks flag).
    pub force_quirks: bool,
}

/// Parses the DOCTYPE token in `span` (including `<!DOCTYPE` and `>`).
///
/// Follows the tokenizer's `DOCTYPE`, `before DOCTYPE name`, `DOCTYPE name`,
/// `after DOCTYPE name`, `after DOCTYPE public/system keyword` and quoted
/// identifier states.  Everything malformed sets force-quirks and stops, which
/// is exactly the behaviour the standard specifies: a stray unquoted token is
/// never part of an identifier.
pub fn doctype_parts(input: &[u8], span: Range<u32>) -> DoctypeParts {
    let start = span.start as usize;
    let end = (span.end as usize).min(input.len());
    let body = &input[start..end];
    let mut out = DoctypeParts::default();
    // `<!DOCTYPE`, case-insensitively.
    let mut i = if body.len() >= 9 && body[..9].eq_ignore_ascii_case(b"<!doctype") {
        9
    } else {
        out.force_quirks = true;
        out.name = (start as u32)..(start as u32);
        return out;
    };
    let skip_ws = |i: &mut usize| {
        while *i < body.len() && matches!(body[*i], b'\t' | b'\n' | 0x0c | b'\r' | b' ') {
            *i += 1;
        }
    };
    skip_ws(&mut i);
    // Name: up to whitespace or `>`.
    let name_start = i;
    while i < body.len() && !matches!(body[i], b'\t' | b'\n' | 0x0c | b'\r' | b' ' | b'>') {
        i += 1;
    }
    let name_end = i;
    out.name = (start + name_start) as u32..(start + name_end) as u32;
    if i >= body.len() || body[i] == b'>' {
        return out;
    }
    skip_ws(&mut i);
    // `PUBLIC` / `SYSTEM`, case-insensitively.
    let keyword = if body[i..].len() >= 6 && body[i..i + 6].eq_ignore_ascii_case(b"public") {
        Some(true)
    } else if body[i..].len() >= 6 && body[i..i + 6].eq_ignore_ascii_case(b"system") {
        Some(false)
    } else {
        None
    };
    let Some(is_public) = keyword else {
        out.force_quirks = true;
        return out;
    };
    i += 6;
    skip_ws(&mut i);
    let Some((first, next)) = quoted(body, i) else {
        out.force_quirks = true;
        return out;
    };
    i = next;
    if is_public {
        skip_ws(&mut i);
        match quoted(body, i) {
            Some((second, next)) => {
                out.public = Some((start + first.start) as u32..(start + first.end) as u32);
                out.system = Some((start + second.start) as u32..(start + second.end) as u32);
                i = next;
            }
            // `PUBLIC "x"` with a single identifier: the public identifier is
            // `x` and the system identifier is missing (the expected trees print
            // the missing one as `""`).
            None => {
                out.public = Some((start + first.start) as u32..(start + first.end) as u32);
            }
        }
    } else {
        out.system = Some((start + first.start) as u32..(start + first.end) as u32);
    }
    skip_ws(&mut i);
    if i < body.len() && body[i] != b'>' {
        out.force_quirks = true;
    }
    out
}

/// Public identifiers that put the document in quirks mode (exact match).
const QUIRKS_PUBLIC_IDS: [&str; 3] = [
    "-//W3O//DTD W3 HTML Strict 3.0//EN//",
    "-/W3C/DTD HTML 4.0 Transitional/EN",
    "HTML",
];

/// System identifiers that put the document in quirks mode (exact match).
const QUIRKS_SYSTEM_IDS: [&str; 1] = ["http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd"];

/// Public identifier prefixes that put the document in quirks mode.
const QUIRKS_PUBLIC_PREFIXES: [&str; 55] = [
    "+//Silmaril//dtd html Pro v0r11 19970101//",
    "-//AS//DTD HTML 3.0 asWedit + extensions//",
    "-//AdvaSoft Ltd//DTD HTML 3.0 asWedit + extensions//",
    "-//IETF//DTD HTML 2.0 Level 1//",
    "-//IETF//DTD HTML 2.0 Level 2//",
    "-//IETF//DTD HTML 2.0 Strict Level 1//",
    "-//IETF//DTD HTML 2.0 Strict Level 2//",
    "-//IETF//DTD HTML 2.0 Strict//",
    "-//IETF//DTD HTML 2.0//",
    "-//IETF//DTD HTML 2.1E//",
    "-//IETF//DTD HTML 3.0//",
    "-//IETF//DTD HTML 3.2 Final//",
    "-//IETF//DTD HTML 3.2//",
    "-//IETF//DTD HTML 3//",
    "-//IETF//DTD HTML Level 0//",
    "-//IETF//DTD HTML Level 1//",
    "-//IETF//DTD HTML Level 2//",
    "-//IETF//DTD HTML Level 3//",
    "-//IETF//DTD HTML Strict Level 0//",
    "-//IETF//DTD HTML Strict Level 1//",
    "-//IETF//DTD HTML Strict Level 2//",
    "-//IETF//DTD HTML Strict Level 3//",
    "-//IETF//DTD HTML Strict//",
    "-//IETF//DTD HTML//",
    "-//Metrius//DTD Metrius Presentational//",
    "-//Microsoft//DTD Internet Explorer 2.0 HTML Strict//",
    "-//Microsoft//DTD Internet Explorer 2.0 HTML//",
    "-//Microsoft//DTD Internet Explorer 2.0 Tables//",
    "-//Microsoft//DTD Internet Explorer 3.0 HTML Strict//",
    "-//Microsoft//DTD Internet Explorer 3.0 HTML//",
    "-//Microsoft//DTD Internet Explorer 3.0 Tables//",
    "-//Netscape Comm. Corp.//DTD HTML//",
    "-//Netscape Comm. Corp.//DTD Strict HTML//",
    "-//O'Reilly and Associates//DTD HTML 2.0//",
    "-//O'Reilly and Associates//DTD HTML Extended 1.0//",
    "-//O'Reilly and Associates//DTD HTML Extended Relaxed 1.0//",
    "-//SQ//DTD HTML 2.0 HoTMetaL + extensions//",
    "-//SoftQuad Software//DTD HoTMetaL PRO 6.0::19990601::extensions to HTML 4.0//",
    "-//SoftQuad//DTD HoTMetaL PRO 4.0::19971010::extensions to HTML 4.0//",
    "-//Spyglass//DTD HTML 2.0 Extended//",
    "-//Sun Microsystems Corp.//DTD HotJava HTML//",
    "-//Sun Microsystems Corp.//DTD HotJava Strict HTML//",
    "-//W3C//DTD HTML 3 1995-03-24//",
    "-//W3C//DTD HTML 3.2 Draft//",
    "-//W3C//DTD HTML 3.2 Final//",
    "-//W3C//DTD HTML 3.2//",
    "-//W3C//DTD HTML 3.2S Draft//",
    "-//W3C//DTD HTML 4.0 Frameset//",
    "-//W3C//DTD HTML 4.0 Transitional//",
    "-//W3C//DTD HTML Experimental 19960712//",
    "-//W3C//DTD HTML Experimental 970421//",
    "-//W3C//DTD W3 HTML//",
    "-//W3O//DTD W3 HTML 3.0//",
    "-//WebTechs//DTD Mozilla HTML 2.0//",
    "-//WebTechs//DTD Mozilla HTML//",
];

/// Public identifier prefixes that put the document in quirks mode only when
/// the system identifier is missing or empty.  (The limited-quirks list is not
/// needed: no tree-construction rule distinguishes it from no-quirks.)
const QUIRKS_PUBLIC_PREFIXES_NO_SYSTEM: [&str; 2] = [
    "-//W3C//DTD HTML 4.01 Frameset//",
    "-//W3C//DTD HTML 4.01 Transitional//",
];

/// Whether the DOCTYPE token in `span` puts the document in **quirks** mode.
///
/// Only the full-quirks conditions matter to tree construction: `in body`'s
/// `table` start tag closes a paragraph "if the Document is not set to quirks
/// mode", and limited-quirks behaves like no-quirks there.  The lists are the
/// standard's, compared ASCII case-insensitively.
pub fn doctype_is_quirks(input: &[u8], span: Range<u32>) -> bool {
    let parts = doctype_parts(input, span);
    if parts.force_quirks {
        return true;
    }
    let text = |range: Range<u32>| {
        std::str::from_utf8(&input[range.start as usize..range.end as usize])
            .expect("Document input is valid UTF-8")
    };
    if !text(parts.name).eq_ignore_ascii_case("html") {
        return true;
    }
    let public = parts.public.map(text);
    let system = parts.system.map(text);
    if let Some(public) = public {
        if QUIRKS_PUBLIC_IDS
            .iter()
            .any(|id| id.eq_ignore_ascii_case(public))
        {
            return true;
        }
        let starts = |list: &[&str]| {
            list.iter().any(|prefix| {
                public.len() >= prefix.len()
                    && public.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
            })
        };
        if starts(&QUIRKS_PUBLIC_PREFIXES) {
            return true;
        }
        // The two 4.01 entries only apply without a system identifier (an
        // empty one counts as present).
        if system.is_none() && starts(&QUIRKS_PUBLIC_PREFIXES_NO_SYSTEM) {
            return true;
        }
    }
    if let Some(system) = system
        && QUIRKS_SYSTEM_IDS
            .iter()
            .any(|id| id.eq_ignore_ascii_case(system))
    {
        return true;
    }
    false
}

/// Reads a quoted identifier starting at `i`, returning its inner range and the
/// index just past the closing quote.
fn quoted(body: &[u8], mut i: usize) -> Option<(Range<usize>, usize)> {
    let quote = *body.get(i)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    i += 1;
    let start = i;
    while i < body.len() && body[i] != quote {
        i += 1;
    }
    if i >= body.len() {
        return None;
    }
    Some((start..i, i + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(src: &str) -> String {
        let bytes = src.as_bytes();
        let span = 0..bytes.len() as u32;
        let data = comment_data(bytes, span);
        String::from_utf8_lossy(&bytes[data.start as usize..data.end as usize]).into_owned()
    }

    #[test]
    fn comment_forms() {
        assert_eq!(comment("<!-- a -->"), " a ");
        assert_eq!(comment("<!--a--!>"), "a");
        assert_eq!(comment("<!-->"), "");
        assert_eq!(comment("<!--->"), "");
        assert_eq!(comment("<!---->"), "");
        assert_eq!(comment("<!-- unclosed"), " unclosed");
        assert_eq!(comment("<!doctype bogus>"), "doctype bogus");
        assert_eq!(comment("<?xml version='1.0'>"), "xml version='1.0'");
        assert_eq!(comment("<!-- a <!-- b -->"), " a <!-- b ");
    }

    fn part(src: &str) -> (String, Option<String>, Option<String>, bool) {
        let bytes = src.as_bytes().to_vec();
        let parts = doctype_parts(&bytes, 0..bytes.len() as u32);
        let text = |r: Range<u32>| {
            String::from_utf8_lossy(&bytes[r.start as usize..r.end as usize]).into_owned()
        };
        (
            text(parts.name),
            parts.public.map(text),
            parts.system.map(text),
            parts.force_quirks,
        )
    }

    #[test]
    fn doctype_forms() {
        assert_eq!(part("<!DOCTYPE html>"), ("html".into(), None, None, false));
        assert_eq!(part("<!dOctYpE HtMl>"), ("HtMl".into(), None, None, false));
        assert_eq!(
            part(r#"<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01//EN" "http://x">"#),
            (
                "html".into(),
                Some("-//W3C//DTD HTML 4.01//EN".into()),
                Some("http://x".into()),
                false
            )
        );
        assert_eq!(
            part("<!DOCTYPE potato SYSTEM 'taco\"'>"),
            ("potato".into(), None, Some("taco\"".into()), false)
        );
        assert_eq!(
            part("<!DOCTYPE potato taco>"),
            ("potato".into(), None, None, true)
        );
        // `PUBLIC` with a single identifier: public id set, system missing.
        assert_eq!(
            part(r#"<!DOCTYPE potato PUBLIC "go'of">"#),
            ("potato".into(), Some("go'of".into()), None, false)
        );
    }
}
