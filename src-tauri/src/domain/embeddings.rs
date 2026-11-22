//! Turning text into vectors — the one model this app ships, and the seam a
//! test puts a fake behind.
//!
//! ## What upstream's 383 lines were, and why ~60 are left
//!
//! Upstream carries a second, remote provider: an OpenAI-compatible
//! `/embeddings` endpoint with its base URL, model name, dimensions, a pinned
//! CA certificate, extra request headers, a TLS-verification switch, presets,
//! a merged config and an API key in the keychain. The owner's decision for
//! this port is **the bundled model only**:
//! code and questions about it never leave the machine to be indexed. Every
//! type that existed to configure or resolve that choice has nothing to
//! choose between, and is gone with it.
//!
//! What stays is the trait. Not for a second provider — for tests: the sync
//! that fills the index must be exercised on hundreds of chunks without
//! loading half a gigabyte of weights into every test.

use std::path::PathBuf;

use thiserror::Error;

/// The bundled model's identity, and the namespace its vectors are stored
/// under (the `embeddings.model_id` column of the index store).
///
/// Change it whenever the weights change in a way that moves vectors: old
/// vectors then sit under a name nothing asks for, and are rebuilt rather than
/// silently compared against new ones.
///
/// `-ru-en` because the vocabulary is cut to Russian and English (F-5.8b).
/// Vectors of Russian, English and code are the same as the full model's, but
/// not for text with other scripts — so it is a different model, and says so.
pub const LOCAL_MODEL_ID: &str = "local-potion-multilingual-128m-int8-ru-en";

/// One text's vector.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding(pub Vec<f32>);

/// Why the model could not be used. Every variant is about the *files*: once
/// loaded, a static model cannot fail to embed.
///
/// Plain data, so `Clone`: a caller that reports it and keeps it can do both.
#[derive(Debug, Clone, Error)]
pub enum EmbeddingError {
    #[error("the embedding model is missing: {0} not found")]
    NotFound(PathBuf),
    /// The file is a Git LFS pointer — a clone without `git lfs pull`. Named
    /// separately because the fix is one command, and "failed to parse" would
    /// send whoever reads it looking for a corrupt download.
    #[error("{0} is a Git LFS pointer, not the file: run `git lfs pull`")]
    LfsPointer(PathBuf),
    #[error("could not read {path}: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    #[error("the embedding model's files are invalid: {0}")]
    Invalid(String),
}

pub trait EmbeddingProvider: Send + Sync {
    /// One vector per text, in order.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError>;
    fn dimensions(&self) -> usize;
}

/// A unit vector stored as one signed byte a component plus one scale.
///
/// 260 bytes for 256 dimensions instead of 1 024, on disk and in memory, for
/// every chunk in the repository. Ranking survives it: each component is off
/// by at most half a step (`scale / 2`), and the tests bound what that does to
/// a dot product.
///
/// The scale is per vector (`max |component| / 127`) rather than one for the
/// whole index, so a vector with small components keeps its resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizedVector {
    pub scale: f32,
    pub values: Vec<i8>,
}

impl QuantizedVector {
    pub fn quantize(vector: &[f32]) -> Self {
        let max = vector.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
        let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
        let values = vector.iter().map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8).collect();
        Self { scale, values }
    }

    /// `query · self`. With both unit length — every model vector here is —
    /// this is the cosine similarity, higher is better.
    pub fn dot(&self, query: &[f32]) -> f32 {
        let raw: f32 = self.values.iter().zip(query).map(|(&v, &q)| f32::from(v) * q).sum();
        raw * self.scale
    }

    /// Scale as four little-endian bytes, then the components.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = self.scale.to_le_bytes().to_vec();
        bytes.extend(self.values.iter().map(|&v| v as u8));
        bytes
    }

    /// `None` for a blob that is not a vector of `dimensions` components — a
    /// damaged row is skipped and re-embedded, not trusted.
    pub fn from_bytes(bytes: &[u8], dimensions: usize) -> Option<Self> {
        if bytes.len() != 4 + dimensions {
            return None;
        }
        let scale = f32::from_le_bytes(bytes[..4].try_into().ok()?);
        (scale.is_finite() && scale > 0.0).then(|| Self {
            scale,
            values: bytes[4..].iter().map(|&b| b as i8).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(seed: u64, dimensions: usize) -> Vec<f32> {
        // A small deterministic generator: no rand crate for a test.
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let v: Vec<f32> = (0..dimensions)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((state >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / norm).collect()
    }

    fn exact_dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    /// The error that matters: how far a quantized score is from the exact
    /// one. Well under the gap between neighbours that ranking depends on.
    #[test]
    fn a_quantized_score_is_within_a_hundredth_of_the_exact_one() {
        let mut worst = 0.0_f32;
        for seed in 0..200 {
            let (stored, query) = (unit(seed, 256), unit(seed + 1_000, 256));
            let error = (QuantizedVector::quantize(&stored).dot(&query) - exact_dot(&stored, &query)).abs();
            worst = worst.max(error);
        }
        assert!(worst < 0.01, "worst error {worst}");
    }

    /// Why the scale is per vector: every component comes back within half a
    /// step of *this vector's* range. One scale for the whole index would be
    /// sized for the largest component anywhere, and a vector of small ones
    /// would keep a fraction of the resolution.
    #[test]
    fn each_component_is_within_half_a_step_of_its_own_range() {
        let small: Vec<f32> = unit(11, 256).iter().map(|x| x * 0.1).collect();
        let max = small.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
        let q = QuantizedVector::quantize(&small);
        for (original, &stored) in small.iter().zip(&q.values) {
            let restored = f32::from(stored) * q.scale;
            assert!((original - restored).abs() <= max / 254.0 * 1.001, "{original} came back as {restored}");
        }
    }

    #[test]
    fn a_vector_is_its_own_best_match_after_quantizing() {
        let stored = unit(7, 256);
        let score = QuantizedVector::quantize(&stored).dot(&stored);
        assert!((score - 1.0).abs() < 0.01, "{score}");
    }

    #[test]
    fn bytes_round_trip() {
        let vector = QuantizedVector::quantize(&unit(3, 256));
        assert_eq!(QuantizedVector::from_bytes(&vector.to_bytes(), 256), Some(vector));
    }

    /// A blob of the wrong length is some other model's vector or a damaged
    /// row; either way not one to rank by.
    #[test]
    fn a_blob_of_the_wrong_shape_is_not_a_vector() {
        let bytes = QuantizedVector::quantize(&unit(3, 256)).to_bytes();
        assert_eq!(QuantizedVector::from_bytes(&bytes, 128), None);
        assert_eq!(QuantizedVector::from_bytes(&bytes[..10], 256), None);
        let mut zero_scale = bytes.clone();
        zero_scale[..4].copy_from_slice(&0.0_f32.to_le_bytes());
        assert_eq!(QuantizedVector::from_bytes(&zero_scale, 256), None);
    }

    /// The zero vector — an empty text — must not divide by zero into NaN,
    /// which would sort unpredictably against every real score.
    #[test]
    fn the_zero_vector_scores_zero() {
        let zero = QuantizedVector::quantize(&[0.0; 256]);
        assert_eq!(zero.dot(&unit(1, 256)), 0.0);
    }
}
