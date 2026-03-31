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
        // Map u32 [0, 2^32-1] to open interval (0, 1) for Box-Muller:
        //   +1 shifts range to [1, 2^32], then /2^32+1 maps to (0, 1).
        //   This avoids u1=0 which would cause ln(0)=-inf in Box-Muller.
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

