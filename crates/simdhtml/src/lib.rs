//! simdhtml is a fast HTML parser powered by SIMD.
//!
//! This crate is not user-facing. For a parsel like facade, see [`simdsel`](https://docs.rs/simdsel/latest/simdsel).
//!
//! The entry point is [`document::Document::parse`].

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
