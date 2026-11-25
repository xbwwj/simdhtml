use std::borrow::Cow;

/// Applies HTML's newline preprocessing: `\r\n` and `\r` become `\n`.
#[must_use]
pub fn normalize_newlines(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.as_bytes().contains(&b'\r') {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    Cow::Owned(out)
}

/// Replaces U+0000 with U+FFFD, as every HTML tokenizer state does.
///
/// The data model has no NUL: it is replaced when the character is emitted, so
/// doing it on extraction keeps the parser's spans valid and costs one byte
/// scan on documents that contain no NUL (the overwhelming majority).
#[must_use]
pub fn replace_nulls(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.as_bytes().contains(&0) {
        return text;
    }
    Cow::Owned(text.replace('\0', "\u{fffd}"))
}
