use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Dimension for text embeddings stored in LanceDB.
pub const TEXT_VECTOR_DIM: usize = 1536;

/// Generate a lightweight, deterministic embedding for text.
///
/// This is a hashed bag-of-words vector intended for local demos. It keeps the
/// system self-contained without external embedding services.
pub fn embed_text(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0f32; TEXT_VECTOR_DIM];

    for token in tokenize(text) {
        let mut hasher = DefaultHasher::new();
        token.hash(&mut hasher);
        let index = (hasher.finish() as usize) % TEXT_VECTOR_DIM;
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

fn normalize_vector(vector: &mut [f32]) {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in vector.iter_mut() {
            *v /= norm;
        }
    }
}
