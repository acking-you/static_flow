#[cfg(not(target_arch = "wasm32"))]
use std::env;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Mutex, OnceLock};

#[cfg(not(target_arch = "wasm32"))]
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};

#[cfg(not(target_arch = "wasm32"))]
use super::cache::SmallModelCache;

/// Image embedding models backed by fastembed.
///
/// Variants map directly to `fastembed::ImageEmbeddingModel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageEmbeddingModelChoice {
    ClipVitB32,
    Resnet50,
    UnicomVitB16,
    UnicomVitB32,
    NomicEmbedVisionV15,
}

impl ImageEmbeddingModelChoice {
    /// Embedding dimension for each image model (from fastembed model list).
    pub const fn dim(self) -> usize {
        match self {
            ImageEmbeddingModelChoice::ClipVitB32 => 512,
            ImageEmbeddingModelChoice::Resnet50 => 2048,
            ImageEmbeddingModelChoice::UnicomVitB16 => 768,
            ImageEmbeddingModelChoice::UnicomVitB32 => 512,
            ImageEmbeddingModelChoice::NomicEmbedVisionV15 => 768,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn to_fastembed(self) -> ImageEmbeddingModel {
        match self {
            ImageEmbeddingModelChoice::ClipVitB32 => ImageEmbeddingModel::ClipVitB32,
            ImageEmbeddingModelChoice::Resnet50 => ImageEmbeddingModel::Resnet50,
            ImageEmbeddingModelChoice::UnicomVitB16 => ImageEmbeddingModel::UnicomVitB16,
            ImageEmbeddingModelChoice::UnicomVitB32 => ImageEmbeddingModel::UnicomVitB32,
            ImageEmbeddingModelChoice::NomicEmbedVisionV15 => {
                ImageEmbeddingModel::NomicEmbedVisionV15
            },
        }
    }
}

/// Default image model used by `embed_image_bytes`.
pub const DEFAULT_IMAGE_MODEL: ImageEmbeddingModelChoice = ImageEmbeddingModelChoice::ClipVitB32;

/// Dimension for image embeddings stored in LanceDB.
///
/// IMPORTANT: If you change the default model, update your LanceDB schema and
/// rebuild the tables to match the new vector dimension.
pub const IMAGE_VECTOR_DIM: usize = DEFAULT_IMAGE_MODEL.dim();

#[cfg(not(target_arch = "wasm32"))]
static FASTEMBED_IMAGE_MODEL: OnceLock<
    Mutex<SmallModelCache<ImageEmbeddingModelChoice, ImageEmbedding>>,
> = OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static FASTEMBED_IMAGE_MODEL_CACHE_LIMIT: OnceLock<usize> = OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_MAX_CACHED_IMAGE_MODELS: usize = 1;

/// Generate a semantic embedding for an image (bytes should be an encoded
/// image).
///
/// Use `embed_image_bytes_with_model` if you need a specific vision model.
pub fn embed_image_bytes(bytes: &[u8]) -> anyhow::Result<Vec<f32>> {
    embed_image_bytes_with_model(bytes, DEFAULT_IMAGE_MODEL)
}

/// Generate a semantic embedding for an image using a specific fastembed vision
/// model.
pub fn embed_image_bytes_with_model(
    bytes: &[u8],
    model: ImageEmbeddingModelChoice,
) -> anyhow::Result<Vec<f32>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        fastembed_image_embedding(bytes, model)
    }

    #[cfg(target_arch = "wasm32")]
    {
        let _ = bytes;
        let _ = model;
        anyhow::bail!("image embedding is not supported on wasm32")
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn fastembed_image_embedding(
    bytes: &[u8],
    model: ImageEmbeddingModelChoice,
) -> anyhow::Result<Vec<f32>> {
    let lock = FASTEMBED_IMAGE_MODEL
        .get_or_init(|| Mutex::new(SmallModelCache::new(image_model_cache_limit())));
    let mut guard = lock
        .lock()
        .map_err(|err| anyhow::anyhow!("image embedding mutex poisoned: {err}"))?;

    let instance = guard.get_or_try_insert_mut(model, || {
        // Keep a tiny LRU cache so rarely used vision models do not stay resident
        // forever.
        let options = ImageInitOptions::new(model.to_fastembed());
        ImageEmbedding::try_new(options).map_err(|err| {
            anyhow::anyhow!("failed to initialize image embedding model {:?}: {err}", model)
        })
    })?;

    let mut embeddings = instance.embed_bytes(&[bytes], None).map_err(|err| {
        anyhow::anyhow!(
            "image embedding failed for model {:?}; input bytes={}: {err}",
            model,
            bytes.len()
        )
    })?;

    embeddings.pop().ok_or_else(|| {
        anyhow::anyhow!("image embedding model {:?} returned empty embedding result", model)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn image_model_cache_limit() -> usize {
    *FASTEMBED_IMAGE_MODEL_CACHE_LIMIT.get_or_init(|| {
        env::var("FASTEMBED_MAX_CACHED_IMAGE_MODELS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_CACHED_IMAGE_MODELS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0x0F, 0x00, 0x01, 0x05, 0x01, 0x02, 0xA2, 0x7D, 0xA4, 0x31, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn embed_image_bytes_has_expected_shape() {
        let vector = embed_image_bytes(TEST_PNG_BYTES).expect("embed image");
        assert_eq!(vector.len(), IMAGE_VECTOR_DIM);
        assert!(vector.iter().any(|v| *v != 0.0));
        assert!(vector.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn fastembed_image_smoke_if_available() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Ok(vector) = fastembed_image_embedding(TEST_PNG_BYTES, DEFAULT_IMAGE_MODEL) {
                assert_eq!(vector.len(), IMAGE_VECTOR_DIM);
                assert!(vector.iter().all(|v| v.is_finite()));
                assert!(vector.iter().any(|v| *v != 0.0));
            }
        }
    }
}
