/// Quantized Johnson-Lindenstrauss (QJL) transform.
///
/// For a residual vector r, QJL computes:
///   qjl(r) = sign(S · r)
///
/// where S is a random Gaussian matrix generated on-the-fly from a seed.
/// The inner product estimator:
///   <y, r> ≈ ||r|| * (π/2) / m * Σ (S_i · y) * sign(S_i · r)

use super::rotation::Rng;
use std::f32::consts::PI;

/// Box-Muller transform: generate a pair of standard normal values.
fn box_muller(u1: f32, u2: f32) -> (f32, f32) {
    let r = (-2.0 * u1.ln()).sqrt();
    let theta = 2.0 * PI * u2;
    (r * theta.cos(), r * theta.sin())
}

/// Generate a single row of the projection matrix S and compute
/// the dot product with the input vector simultaneously.
fn projection_dot_product(seed: &str, row_index: usize, vector: &[f32]) -> f32 {
    let combined_seed = format!("{}:{}", seed, row_index);
    let mut rng = Rng::from_seed(&combined_seed);
    let d = vector.len();
    let mut dot = 0.0f32;

    let mut i = 0;
    while i < d {
        let u1 = (rng.next() as f32 + 1.0) / 4294967297.0;
        let u2 = (rng.next() as f32 + 1.0) / 4294967297.0;
        let (g1, g2) = box_muller(u1, u2);

        dot += g1 * vector[i];
        if i + 1 < d {
            dot += g2 * vector[i + 1];
        }
        i += 2;
    }

    dot
}

/// Apply QJL to a residual vector. Returns packed sign bits.
pub fn qjl_project(residual: &[f32], seed: &str, m: usize) -> Vec<u8> {
    let mut buf = vec![0u8; (m + 7) / 8];

    for i in 0..m {
        let dot = projection_dot_product(seed, i, residual);
        if dot >= 0.0 {
            buf[i >> 3] |= 1 << (i & 7);
        }
    }

    buf
}

/// Compute the QJL inner product estimate: <y, r>.
///
/// Formula: ||r|| * (π/2) / m * Σ (S_i · y) * sign(S_i · r)
pub fn qjl_inner_product(
    qjl_bits: &[u8],
    residual_norm: f32,
    query_vector: &[f32],
    seed: &str,
    m: usize,
) -> f32 {
    if residual_norm == 0.0 {
        return 0.0;
    }

    let mut sum = 0.0f32;
    for i in 0..m {
        let sign: f32 = if ((qjl_bits[i >> 3] >> (i & 7)) & 1) != 0 {
            1.0
        } else {
            -1.0
        };
        let dot = projection_dot_product(seed, i, query_vector);
        sum += dot * sign;
    }

    residual_norm * (PI / 2.0) * sum / m as f32
}
