//! Embedding helpers shared by content indexing and retrieval flows.
//!
//! Text and image embedding model wrappers (fastembed-backed) plus the local
//! model cache. Host-only machinery: keep this crate out of wasm dependency
//! graphs so frontend builds never pull the ONNX runtime.
#![allow(
    missing_docs,
    reason = "This crate exports many embedding-specific items; enforcing item-level docs is a \
              separate documentation pass."
)]

mod cache;
pub mod image;
pub mod text;

pub use image::{
    embed_image_bytes, embed_image_bytes_with_model, ImageEmbeddingModelChoice,
    DEFAULT_IMAGE_MODEL, IMAGE_VECTOR_DIM,
};
pub use text::{
    detect_language, embed_text, embed_text_with_language, embed_text_with_model,
    TextEmbeddingLanguage, TextEmbeddingModel, DEFAULT_TEXT_LANGUAGE, DEFAULT_TEXT_MODEL,
    TEXT_VECTOR_DIM_EN, TEXT_VECTOR_DIM_ZH,
};
