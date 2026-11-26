use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use entities::ENTITIES;

/// Longest named entity in the HTML table.
const MAX_NAMED_ENTITY_LEN: usize = 35;

fn entity_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        ENTITIES
            .iter()
            .map(|entity| (entity.entity, entity.characters))
            .collect()
    })
}

/// Decodes HTML character references.  `in_attribute` applies the HTML rule that
/// a named reference without `;` is not decoded when followed by an
/// alphanumeric or `=`; unknown or malformed references stay literal.
pub fn decode_character_references(input: &str, in_attribute: bool) -> Cow<'_, str> {
    if !input.as_bytes().contains(&b'&') {
        return Cow::Borrowed(input);
    }

    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let mut last = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'&' {
            i += 1;
            continue;
        }

        let decoded: Option<(Entity, usize)> = if bytes.get(i + 1) == Some(&b'#') {
            decode_numeric(&input[i..]).map(|(c, used)| (Entity::Char(c), used))
        } else {
            decode_named(&input[i..], in_attribute).map(|(s, used)| (Entity::Static(s), used))
        };

        if let Some((entity, used)) = decoded {
            out.push_str(&input[last..i]);
            match entity {
                Entity::Static(s) => out.push_str(s),
                Entity::Char(c) => out.push(c),
            }
            i += used;
            last = i;
        } else {
            i += 1;
        }
    }

    if last == 0 {
        return Cow::Borrowed(input);
    }
    out.push_str(&input[last..]);
    Cow::Owned(out)
}

enum Entity {
    Static(&'static str),
    Char(char),
}

fn decode_named(input: &str, in_attribute: bool) -> Option<(&'static str, usize)> {
    debug_assert!(input.starts_with('&'));
    let bytes = input.as_bytes();

    let mut max = 1usize;
    for &b in bytes.iter().skip(1).take(MAX_NAMED_ENTITY_LEN) {
        if b.is_ascii_alphanumeric() || b == b';' {
            max += 1;
        } else {
            break;
        }
    }

    // Longest match wins; `&amp;` is preferred over `&amp`.
    for len in (2..=max).rev() {
        let candidate = &input[..len];
        if let Some(&decoded) = entity_map().get(candidate) {
            let has_semicolon = candidate.ends_with(';');
            if in_attribute
                && !has_semicolon
                && bytes
                    .get(len)
                    .is_some_and(|&next| next.is_ascii_alphanumeric() || next == b'=')
            {
                return None;
            }
            return Some((decoded, len));
        }
    }
    None
}

fn decode_numeric(input: &str) -> Option<(char, usize)> {
    let bytes = input.as_bytes();
    debug_assert!(bytes.first() == Some(&b'&') && bytes.get(1) == Some(&b'#'));

    let mut i = 2usize;
    let hex = bytes.get(i).is_some_and(|&b| b == b'x' || b == b'X');
    if hex {
        i += 1;
    }

    let digits_start = i;
    let mut value = 0u32;
    while let Some(&b) = bytes.get(i) {
        let digit = match b {
            b'0'..=b'9' => u32::from(b - b'0'),
            b'a'..=b'f' if hex => u32::from(b - b'a') + 10,
            b'A'..=b'F' if hex => u32::from(b - b'A') + 10,
            _ => break,
        };
        value = value.saturating_mul(if hex { 16 } else { 10 });
        value = value.saturating_add(digit);
        i += 1;
    }

    if i == digits_start {
        return None;
    }
    if bytes.get(i) == Some(&b';') {
        i += 1;
    }

    Some((numeric_char(value), i))
}

fn numeric_char(value: u32) -> char {
    // HTML maps C1 code points through windows-1252 and replaces surrogates /
    // out-of-range values with U+FFFD.
    const WINDOWS_1252: [u32; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    ];

    let scalar = match value {
        0 => None,
        0x80..=0x9F => Some(WINDOWS_1252[(value - 0x80) as usize]),
        0xD800..=0xDFFF | 0x11_0000..=u32::MAX => None,
        _ => Some(value),
    };

    scalar.and_then(char::from_u32).unwrap_or('\u{FFFD}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_and_numeric() {
        assert_eq!(decode_character_references("a &amp; b", false), "a & b");
        assert_eq!(decode_character_references("&#65;&#x42;", false), "AB");
        assert_eq!(decode_character_references("&lt;div&gt;", false), "<div>");
        assert_eq!(decode_character_references("&nbsp;", false), "\u{00A0}");
    }

    #[test]
    fn legacy_without_semicolon() {
        assert_eq!(decode_character_references("&amp x", false), "& x");
        assert_eq!(decode_character_references("&amp x", true), "& x");
        assert_eq!(decode_character_references("&ampx", true), "&ampx");
        assert_eq!(decode_character_references("&ampx", false), "&x");
    }

    #[test]
    fn longest_match_wins() {
        assert_eq!(decode_character_references("&notin;", false), "\u{2209}");
        assert_eq!(decode_character_references("&notit;", false), "\u{00AC}it;");
    }

    #[test]
    fn malformed_is_literal() {
        assert_eq!(
            decode_character_references("&nothing", false),
            "\u{00AC}hing"
        );
        assert_eq!(decode_character_references("&#;", false), "&#;");
        assert_eq!(decode_character_references("&;", false), "&;");
    }

    #[test]
    fn numeric_replacements() {
        assert_eq!(decode_character_references("&#0;", false), "\u{FFFD}");
        assert_eq!(decode_character_references("&#x1F600;", false), "\u{1F600}");
        assert_eq!(decode_character_references("&#128;", false), "\u{20AC}");
        assert_eq!(decode_character_references("&#xD800;", false), "\u{FFFD}");
    }

    #[test]
    fn borrowed_when_no_ampersand() {
        assert!(matches!(
            decode_character_references("plain text", false),
            Cow::Borrowed(_)
        ));
    }
}
