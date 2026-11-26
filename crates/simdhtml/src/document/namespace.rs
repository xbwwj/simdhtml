/// The namespace attached to an element node.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Namespace(u8);

impl Namespace {
    pub const HTML: Self = Self(0);
    pub const SVG: Self = Self(1);
    pub const MATHML: Self = Self(2);
    /// A MathML text integration point: children belong to HTML.
    pub const MATH_TEXT_POINT: Self = Self(3);
    /// An SVG element whose children are HTML (`foreignObject`, `desc`, `title`).
    pub const SVG_HTML_POINT: Self = Self(4);

    pub(crate) const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub(crate) const fn bits(self) -> u8 {
        self.0
    }
}

pub(crate) const NS_SHIFT: u32 = 3;
pub(crate) const NS_MASK: u8 = 0b111 << NS_SHIFT;
