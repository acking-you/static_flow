pub mod image;
pub mod text;
mod utils;

pub use image::{
    embed_image_bytes, embed_image_bytes_with_model, ImageEmbeddingModelChoice,
    DEFAULT_IMAGE_MODEL, IMAGE_VECTOR_DIM,
};
pub use text::{
    detect_language, embed_text, embed_text_with_language, embed_text_with_model,
    TextEmbeddingLanguage, TextEmbeddingModel, DEFAULT_TEXT_LANGUAGE, DEFAULT_TEXT_MODEL,
    TEXT_VECTOR_DIM_EN, TEXT_VECTOR_DIM_ZH,
};
