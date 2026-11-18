//! Per-token tag classification used by normalization.
//!
//! This is deliberately *not* a global tag interner: the tokenizer stores one
//! small integer per tag token, and unknown names collapse to [`TagCode::Other`]
//! (which keeps its raw name span for equality checks).  The normalizer's
//! implied-end and table-wrapper rules only care about a fixed set of names,
//! so a byte-sized code turns those rules into integer switches instead of
//! repeated case-insensitive byte-string comparisons.

/// The tag names normalization has to distinguish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum TagCode {
    /// Any tag without a dedicated code.
    Other = 0,

    P,
    Li,
    Ul,
    Ol,
    Menu,
    Dl,
    Dt,
    Dd,
    Option,
    Optgroup,
    Select,
    Datalist,
    Table,
    Thead,
    Tbody,
    Tfoot,
    Tr,
    Td,
    Th,
    Colgroup,
    Col,
    Caption,
    Ruby,
    Rt,
    Rp,

    Address,
    Article,
    Aside,
    Blockquote,
    Div,
    Fieldset,
    Footer,
    Form,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Header,
    Hr,
    Main,
    Nav,
    Pre,
    Section,

    Button,
    Marquee,
    Object,
    Applet,
    Template,

    Area,
    Base,
    Br,
    Embed,
    Img,
    Input,
    Link,
    Meta,
    Param,
    Source,
    Track,
    Wbr,
}

const MAX_NAME: usize = 16;

impl TagCode {
    /// Classifies a raw tag name (ASCII case-insensitive).
    #[must_use]
    pub(crate) fn of(name: &[u8]) -> Self {
        if name.len() > MAX_NAME {
            return Self::Other;
        }
        // The overwhelmingly common case is already lowercase; only pay for a
        // copy when an uppercase byte is actually present.
        if name.iter().any(u8::is_ascii_uppercase) {
            let mut buf = [0u8; MAX_NAME];
            for (dst, &src) in buf.iter_mut().zip(name) {
                *dst = src.to_ascii_lowercase();
            }
            classify(&buf[..name.len()])
        } else {
            classify(name)
        }
    }
}

fn classify(name: &[u8]) -> TagCode {
    match name {
        b"p" => TagCode::P,
        b"li" => TagCode::Li,
        b"ul" => TagCode::Ul,
        b"ol" => TagCode::Ol,
        b"menu" => TagCode::Menu,
        b"dl" => TagCode::Dl,
        b"dt" => TagCode::Dt,
        b"dd" => TagCode::Dd,
        b"option" => TagCode::Option,
        b"optgroup" => TagCode::Optgroup,
        b"select" => TagCode::Select,
        b"datalist" => TagCode::Datalist,
        b"table" => TagCode::Table,
        b"thead" => TagCode::Thead,
        b"tbody" => TagCode::Tbody,
        b"tfoot" => TagCode::Tfoot,
        b"tr" => TagCode::Tr,
        b"td" => TagCode::Td,
        b"th" => TagCode::Th,
        b"colgroup" => TagCode::Colgroup,
        b"col" => TagCode::Col,
        b"caption" => TagCode::Caption,
        b"ruby" => TagCode::Ruby,
        b"rt" => TagCode::Rt,
        b"rp" => TagCode::Rp,

        b"address" => TagCode::Address,
        b"article" => TagCode::Article,
        b"aside" => TagCode::Aside,
        b"blockquote" => TagCode::Blockquote,
        b"div" => TagCode::Div,
        b"fieldset" => TagCode::Fieldset,
        b"footer" => TagCode::Footer,
        b"form" => TagCode::Form,
        b"h1" => TagCode::H1,
        b"h2" => TagCode::H2,
        b"h3" => TagCode::H3,
        b"h4" => TagCode::H4,
        b"h5" => TagCode::H5,
        b"h6" => TagCode::H6,
        b"header" => TagCode::Header,
        b"hr" => TagCode::Hr,
        b"main" => TagCode::Main,
        b"nav" => TagCode::Nav,
        b"pre" => TagCode::Pre,
        b"section" => TagCode::Section,

        b"button" => TagCode::Button,
        b"marquee" => TagCode::Marquee,
        b"object" => TagCode::Object,
        b"applet" => TagCode::Applet,
        b"template" => TagCode::Template,

        b"area" => TagCode::Area,
        b"base" => TagCode::Base,
        b"br" => TagCode::Br,
        b"embed" => TagCode::Embed,
        b"img" => TagCode::Img,
        b"input" => TagCode::Input,
        b"link" => TagCode::Link,
        b"meta" => TagCode::Meta,
        b"param" => TagCode::Param,
        b"source" => TagCode::Source,
        b"track" => TagCode::Track,
        b"wbr" => TagCode::Wbr,
        _ => TagCode::Other,
    }
}

/// Tag names that close an open `<p>`.
#[must_use]
pub(crate) const fn closes_p(code: TagCode) -> bool {
    matches!(
        code,
        TagCode::Address
            | TagCode::Article
            | TagCode::Aside
            | TagCode::Blockquote
            | TagCode::Div
            | TagCode::Dl
            | TagCode::Fieldset
            | TagCode::Footer
            | TagCode::Form
            | TagCode::H1
            | TagCode::H2
            | TagCode::H3
            | TagCode::H4
            | TagCode::H5
            | TagCode::H6
            | TagCode::Header
            | TagCode::Hr
            | TagCode::Main
            | TagCode::Nav
            | TagCode::Ol
            | TagCode::P
            | TagCode::Pre
            | TagCode::Section
            | TagCode::Table
            | TagCode::Ul
            | TagCode::Li
            | TagCode::Dt
            | TagCode::Dd
            | TagCode::Tr
            | TagCode::Td
            | TagCode::Th
            | TagCode::Thead
            | TagCode::Tbody
            | TagCode::Tfoot
            | TagCode::Colgroup
    )
}

/// Tags that end an open element without an explicit end tag.
#[must_use]
pub(crate) const fn is_void(code: TagCode) -> bool {
    matches!(
        code,
        TagCode::Area
            | TagCode::Base
            | TagCode::Br
            | TagCode::Col
            | TagCode::Embed
            | TagCode::Hr
            | TagCode::Img
            | TagCode::Input
            | TagCode::Link
            | TagCode::Meta
            | TagCode::Param
            | TagCode::Source
            | TagCode::Track
            | TagCode::Wbr
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_case_insensitively() {
        assert_eq!(TagCode::of(b"table"), TagCode::Table);
        assert_eq!(TagCode::of(b"TABLE"), TagCode::Table);
        assert_eq!(TagCode::of(b"TaBlE"), TagCode::Table);
        assert_eq!(TagCode::of(b"custom-element"), TagCode::Other);
        assert!(closes_p(TagCode::Div));
        assert!(is_void(TagCode::Br));
        assert!(!is_void(TagCode::Div));
    }
}
