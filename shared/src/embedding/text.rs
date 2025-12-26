#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Mutex, OnceLock};
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
};

#[cfg(not(target_arch = "wasm32"))]
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use super::utils::normalize_vector;

/// Text embedding language selector.
///
/// This is intentionally small (English/Chinese) to match the project's current
/// needs. Use `TextEmbeddingModel` if you want explicit model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextEmbeddingLanguage {
    English,
    Chinese,
}

impl TextEmbeddingLanguage {
    /// Pick a default model for each language.
    ///
    /// English defaults to BGESmallENV15; Chinese defaults to BGESmallZHV15.
    pub const fn default_model(self) -> TextEmbeddingModel {
        match self {
            TextEmbeddingLanguage::English => TextEmbeddingModel::BgeSmallEnV15,
            TextEmbeddingLanguage::Chinese => TextEmbeddingModel::BgeSmallZhV15,
        }
    }
}

/// Text embedding models backed by fastembed.
///
/// Variants map directly to `fastembed::EmbeddingModel` so we can switch models
/// without leaking fastembed types into other crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextEmbeddingModel {
    BgeSmallEnV15,
    BgeBaseEnV15,
    BgeLargeEnV15,
    BgeSmallZhV15,
    BgeLargeZhV15,
}

impl TextEmbeddingModel {
    /// Embedding dimension for each model (from fastembed model list).
    pub const fn dim(self) -> usize {
        match self {
            TextEmbeddingModel::BgeSmallEnV15 => 384,
            TextEmbeddingModel::BgeBaseEnV15 => 768,
            TextEmbeddingModel::BgeLargeEnV15 => 1024,
            TextEmbeddingModel::BgeSmallZhV15 => 512,
            TextEmbeddingModel::BgeLargeZhV15 => 1024,
        }
    }

    /// Map to the underlying fastembed enum.
    #[cfg(not(target_arch = "wasm32"))]
    fn to_fastembed(self) -> EmbeddingModel {
        match self {
            TextEmbeddingModel::BgeSmallEnV15 => EmbeddingModel::BGESmallENV15,
            TextEmbeddingModel::BgeBaseEnV15 => EmbeddingModel::BGEBaseENV15,
            TextEmbeddingModel::BgeLargeEnV15 => EmbeddingModel::BGELargeENV15,
            TextEmbeddingModel::BgeSmallZhV15 => EmbeddingModel::BGESmallZHV15,
            TextEmbeddingModel::BgeLargeZhV15 => EmbeddingModel::BGELargeZHV15,
        }
    }
}

/// Default language/model used by `embed_text`.
pub const DEFAULT_TEXT_LANGUAGE: TextEmbeddingLanguage = TextEmbeddingLanguage::English;
pub const DEFAULT_TEXT_MODEL: TextEmbeddingModel = DEFAULT_TEXT_LANGUAGE.default_model();

/// Dimension for English text embeddings stored in LanceDB.
///
/// IMPORTANT: If you change the default English model, update your LanceDB
/// schema and rebuild the tables to match the new vector dimension.
pub const TEXT_VECTOR_DIM_EN: usize = TextEmbeddingLanguage::English.default_model().dim();

/// Dimension for Chinese text embeddings stored in LanceDB.
///
/// IMPORTANT: If you change the default Chinese model, update your LanceDB
/// schema and rebuild the tables to match the new vector dimension.
pub const TEXT_VECTOR_DIM_ZH: usize = TextEmbeddingLanguage::Chinese.default_model().dim();

#[cfg(not(target_arch = "wasm32"))]
static FASTEMBED_TEXT_MODEL: OnceLock<Mutex<HashMap<TextEmbeddingModel, TextEmbedding>>> =
    OnceLock::new();

/// Generate a semantic embedding for text using the default language/model.
///
/// Use `embed_text_with_language` or `embed_text_with_model` if you need a
/// specific language or model.
pub fn embed_text(text: &str) -> Vec<f32> {
    embed_text_with_language(text, DEFAULT_TEXT_LANGUAGE)
}

/// Generate a semantic embedding for text using a language-specific default
/// model.
pub fn embed_text_with_language(text: &str, language: TextEmbeddingLanguage) -> Vec<f32> {
    embed_text_with_model(text, language.default_model())
}

/// Generate a semantic embedding for text using an explicit model selection.
pub fn embed_text_with_model(text: &str, model: TextEmbeddingModel) -> Vec<f32> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(vector) = fastembed_embedding(text, model) {
            return vector;
        }
    }

    hashed_embedding(text, model.dim())
}

/// Detect language with a lightweight heuristic.
///
/// If the input contains any CJK character, we treat it as Chinese; otherwise
/// default to English. This avoids external dependencies and keeps decisions
/// local and deterministic.
pub fn detect_language(text: &str) -> TextEmbeddingLanguage {
    if text.chars().any(is_cjk) {
        TextEmbeddingLanguage::Chinese
    } else {
        TextEmbeddingLanguage::English
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0xF900..=0xFAFF
            | 0x2F800..=0x2FA1F
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn fastembed_embedding(text: &str, model: TextEmbeddingModel) -> Option<Vec<f32>> {
    let lock = FASTEMBED_TEXT_MODEL.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = lock.lock().ok()?;

    if !guard.contains_key(&model) {
        // Model initialization is expensive; cache the instance for reuse.
        let options = TextInitOptions::new(model.to_fastembed());
        match TextEmbedding::try_new(options) {
            Ok(instance) => {
                guard.insert(model, instance);
            },
            Err(err) => {
                tracing::warn!(
                    "fastembed initialization failed, using hash embedding fallback: {err}"
                );
                return None;
            },
        }
    }

    let instance = guard.get_mut(&model)?;
    match instance.embed(vec![text], None) {
        Ok(mut embeddings) => embeddings.pop(),
        Err(err) => {
            tracing::warn!("fastembed embed failed, using hash embedding fallback: {err}");
            None
        },
    }
}

fn hashed_embedding(text: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0f32; dim];

    for token in tokenize(text) {
        let mut hasher = DefaultHasher::new();
        token.hash(&mut hasher);
        let index = (hasher.finish() as usize) % dim;
        vector[index] += 1.0;
    }

    normalize_vector(&mut vector);
    vector
}

fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                current.push(lower);
            }
        } else if !current.is_empty() {
            tokens.push(current.clone());
            current.clear();
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_embedding_is_deterministic() {
        let first = hashed_embedding("hello world", TEXT_VECTOR_DIM_EN);
        let second = hashed_embedding("hello world", TEXT_VECTOR_DIM_EN);
        assert_eq!(first, second);
    }

    #[test]
    fn embed_text_has_expected_shape() {
        let vector = embed_text("StaticFlow embeddings");
        assert_eq!(vector.len(), TEXT_VECTOR_DIM_EN);
        assert!(vector.iter().any(|v| *v != 0.0));
        assert!(vector.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn embed_text_with_language_matches_language_dim() {
        let vector = embed_text_with_language("中文内容", TextEmbeddingLanguage::Chinese);
        let expected_dim = TextEmbeddingLanguage::Chinese.default_model().dim();
        assert_eq!(vector.len(), expected_dim);
        assert!(vector.iter().any(|v| *v != 0.0));
        assert!(vector.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn fastembed_smoke_if_available() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(vector) = fastembed_embedding("fastembed smoke", DEFAULT_TEXT_MODEL) {
                assert_eq!(vector.len(), TEXT_VECTOR_DIM_EN);
                assert!(vector.iter().all(|v| v.is_finite()));
                assert!(vector.iter().any(|v| *v != 0.0));
            }
        }
    }
}
