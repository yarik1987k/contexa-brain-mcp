pub mod rotation;
pub mod codebooks;
pub mod qjl;

use rotation::{generate_sign_flips, apply_rotation, apply_inverse_rotation};
use codebooks::{build_codebooks, quantize_vector, dequantize_vector, pack_indices, unpack_indices, Codebook};
use qjl::{qjl_project, qjl_inner_product};

const DEFAULT_DIMENSION: usize = 1024;
const DEFAULT_BITS: u8 = 2;
const ROTATION_SEED: &str = "contexa-turbo-v1";
const QJL_SEED: &str = "contexa-qjl-v1";

/// Quantized representation of a vector.
pub struct QuantizedVector {
    pub mse_indices: Vec<u8>,
    pub norm: f32,
    pub qjl_bits: Option<Vec<u8>>,
    pub residual_norm: f32,
    pub bits: u8,
    pub mode: QuantMode,
    pub original_dim: usize,
    pub padded_dim: usize,
}

#[derive(Clone, Copy, PartialEq)]
pub enum QuantMode {
    Fast,     // MSE only — fast, slightly biased
    Unbiased, // MSE + QJL — unbiased inner products
}

/// TurboQuant engine. Initialize once, reuse for all quantization.
pub struct TurboQuant {
    sign_flips: Vec<i8>,
    codebooks: Vec<Codebook>, // index 0 = 1-bit, 1 = 2-bit, etc.
    padded_dim: usize,
}

impl TurboQuant {
    /// Create a new TurboQuant engine for dimension d.
    pub fn new(d: usize) -> Self {
        let padded_dim = next_pow2(d);
        let sign_flips = generate_sign_flips(ROTATION_SEED, padded_dim);
        let codebooks = build_codebooks(padded_dim);
        Self {
            sign_flips,
            codebooks,
            padded_dim,
        }
    }

    /// Create with default dimension (1024).
    pub fn default() -> Self {
        Self::new(DEFAULT_DIMENSION)
    }

    /// Quantize a vector.
    pub fn quantize(&self, vector: &[f32], bits: u8, mode: QuantMode) -> QuantizedVector {
        let d = vector.len();
        let padded_dim = self.padded_dim;

        // Pad to power of 2
        let mut vec = vec![0.0f32; padded_dim];
        for i in 0..d.min(padded_dim) {
            vec[i] = vector[i];
        }
        let norm = l2_norm(&vec);

        if norm == 0.0 {
            return QuantizedVector {
                mse_indices: vec![0u8; (padded_dim * bits as usize + 7) / 8],
                norm: 0.0,
                qjl_bits: None,
                residual_norm: 0.0,
                bits,
                mode,
                original_dim: d,
                padded_dim,
            };
        }

        // Normalize to unit sphere
        let mut unit = vec![0.0f32; padded_dim];
        for i in 0..padded_dim {
            unit[i] = vec[i] / norm;
        }

        // Apply random rotation
        let mut rotated = unit.clone();
        apply_rotation(&mut rotated, &self.sign_flips);

        // Determine MSE bit-width
        let mse_bits = if mode == QuantMode::Unbiased {
            (bits - 1).max(1)
        } else {
            bits
        };
        let codebook = &self.codebooks[(mse_bits - 1) as usize];

        let indices = quantize_vector(&rotated, codebook);
        let packed = pack_indices(&indices, mse_bits);

        if mode == QuantMode::Fast {
            return QuantizedVector {
                mse_indices: packed,
                norm,
                qjl_bits: None,
                residual_norm: 0.0,
                bits,
                mode,
                original_dim: d,
                padded_dim,
            };
        }

        // Unbiased mode: compute residual and apply QJL
        let dequantized = dequantize_vector(&indices, codebook);

        let mut residual_rotated = vec![0.0f32; padded_dim];
        for i in 0..padded_dim {
            residual_rotated[i] = rotated[i] - dequantized[i];
        }

        let mut residual = residual_rotated.clone();
        apply_inverse_rotation(&mut residual, &self.sign_flips);
        let residual_norm = l2_norm(&residual);

        let qjl_bits = if residual_norm > 1e-10 {
            let mut residual_unit = vec![0.0f32; padded_dim];
            for i in 0..padded_dim {
                residual_unit[i] = residual[i] / residual_norm;
            }
            Some(qjl_project(&residual_unit, QJL_SEED, padded_dim))
        } else {
            None
        };

        QuantizedVector {
            mse_indices: packed,
            norm,
            qjl_bits,
            residual_norm,
            bits,
            mode,
            original_dim: d,
            padded_dim,
        }
    }

    /// Dequantize back to approximate float vector.
    pub fn dequantize(&self, quantized: &QuantizedVector) -> Vec<f32> {
        if quantized.norm == 0.0 {
            return vec![0.0; quantized.original_dim];
        }

        let mse_bits = if quantized.mode == QuantMode::Unbiased {
            (quantized.bits - 1).max(1)
        } else {
            quantized.bits
        };
        let codebook = &self.codebooks[(mse_bits - 1) as usize];

        let indices = unpack_indices(&quantized.mse_indices, mse_bits, self.padded_dim);
        let mut rotated_approx = dequantize_vector(&indices, codebook);

        apply_inverse_rotation(&mut rotated_approx, &self.sign_flips);

        let mut result = vec![0.0f32; quantized.original_dim];
        for i in 0..quantized.original_dim {
            result[i] = rotated_approx[i] * quantized.norm;
        }
        result
    }

    /// Fast cosine similarity using only MSE component (biased but fast).
    pub fn fast_cosine_similarity(
        &self,
        query_rotated: &[f32],
        query_norm: f32,
        quantized: &QuantizedVector,
    ) -> f32 {
        if quantized.norm == 0.0 || query_norm == 0.0 {
            return 0.0;
        }

        let mse_bits = if quantized.mode == QuantMode::Unbiased {
            (quantized.bits - 1).max(1)
        } else {
            quantized.bits
        };
        let codebook = &self.codebooks[(mse_bits - 1) as usize];
        let d = query_rotated.len();
        let indices = unpack_indices(&quantized.mse_indices, mse_bits, d);

        let mut dot = 0.0f32;
        for i in 0..d {
            dot += query_rotated[i] * codebook.centroids[indices[i] as usize];
        }

        (dot * quantized.norm) / (query_norm * quantized.norm)
    }

    /// Full cosine similarity with QJL correction (unbiased).
    pub fn cosine_similarity(&self, query: &[f32], quantized: &QuantizedVector) -> f32 {
        let d = query.len();
        let padded_dim = self.padded_dim;

        if quantized.norm == 0.0 {
            return 0.0;
        }

        // Pad query
        let mut q = vec![0.0f32; padded_dim];
        for i in 0..d.min(padded_dim) {
            q[i] = query[i];
        }
        let query_norm = l2_norm(&q);
        if query_norm == 0.0 {
            return 0.0;
        }

        // Rotate query
        let mut query_rotated = q.clone();
        apply_rotation(&mut query_rotated, &self.sign_flips);

        // MSE component
        let mse_bits = if quantized.mode == QuantMode::Unbiased {
            (quantized.bits - 1).max(1)
        } else {
            quantized.bits
        };
        let codebook = &self.codebooks[(mse_bits - 1) as usize];
        let indices = unpack_indices(&quantized.mse_indices, mse_bits, d);

        let mut mse_dot = 0.0f32;
        for i in 0..d {
            mse_dot += query_rotated[i] * codebook.centroids[indices[i] as usize];
        }
        mse_dot *= quantized.norm;

        // QJL correction
        let mut qjl_dot = 0.0f32;
        if quantized.residual_norm > 1e-10 {
            if let Some(ref qjl_bits) = quantized.qjl_bits {
                qjl_dot = qjl_inner_product(
                    qjl_bits,
                    quantized.residual_norm * quantized.norm,
                    &q,
                    QJL_SEED,
                    d,
                );
            }
        }

        (mse_dot + qjl_dot) / (query_norm * quantized.norm)
    }

    /// Prepare a rotated query for batch fast_cosine_similarity calls.
    pub fn prepare_query(&self, query: &[f32]) -> (Vec<f32>, f32) {
        let d = query.len();
        let mut q = vec![0.0f32; self.padded_dim];
        for i in 0..d.min(self.padded_dim) {
            q[i] = query[i];
        }
        let norm = l2_norm(&q);
        apply_rotation(&mut q, &self.sign_flips);
        (q, norm)
    }

    /// Get compression statistics.
    pub fn storage_info(&self, bits: u8, d: usize) -> StorageInfo {
        let mse_bits = (bits - 1).max(1);
        let mse_bytes = (d * mse_bits as usize + 7) / 8;
        let qjl_bytes = (d + 7) / 8;
        let norm_bytes = 8;
        let total = mse_bytes + qjl_bytes + norm_bytes;
        StorageInfo {
            mse_bytes,
            qjl_bytes,
            norm_bytes,
            total_bytes: total,
            original_bytes: d * 4,
            compression_ratio: (d * 4) as f32 / total as f32,
        }
    }
}

pub struct StorageInfo {
    pub mse_bytes: usize,
    pub qjl_bytes: usize,
    pub norm_bytes: usize,
    pub total_bytes: usize,
    pub original_bytes: usize,
    pub compression_ratio: f32,
}

fn l2_norm(arr: &[f32]) -> f32 {
    let sum: f32 = arr.iter().map(|x| x * x).sum();
    sum.sqrt()
}

fn next_pow2(n: usize) -> usize {
    let mut p = 1;
    while p < n {
        p <<= 1;
    }
    p
}
