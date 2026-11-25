//! simdhtml is a fast HTML parser powered by SIMD.
//!
//! See [`Document`] and [`Document::parse`] for a quickstart.
//!
//! This crate is not user-facing. See `simdsel` which provides a parsel like facade.

mod character_reference;
mod document;
pub mod encoding;
mod error;
pub mod markup;
mod parse;
pub mod simd;
mod tag_code;
pub mod token_data;
mod tokenizer;
mod tree_builder;

pub use crate::character_reference::decode_character_references;
pub use crate::document::{
    DOC_ROOT, Document, DocumentFlags, NO_NODE, NO_TAG, NS_HTML, NS_MATH_TEXT_POINT, NS_MATHML,
    NS_SVG, NS_SVG_HTML_POINT, Node, NodeId, NodeKind, Warning, WarningKind,
};
pub use crate::document::{normalize_newlines, replace_nulls};
pub use crate::error::{ParseError, Result};
pub use crate::markup::{MarkupDiagnostics, MarkupError, MarkupIndex, OpaqueRegion, RegionKind};
pub use crate::parse::{EncodingMode, ParseOptions, RepairMode};
pub use crate::tag_code::code_of as tag_code_of;
pub use crate::tokenizer::{AttrQuote, TagKind, TagToken};
