//! simdhtml is a fast HTML parser powered by SIMD.
//!
//! See [`document::Document`] and [`document::Document::parse`] for a quickstart.
//!
//! This crate is not user-facing. See `simdsel` which provides a parsel like facade.

pub mod character_reference;
pub mod document;
pub mod encoding;
pub mod error;
pub mod markup;
pub mod parse;
pub mod simd;
pub mod tag_code;
pub mod token_data;
pub mod tokenizer;
mod tree_builder;
