/// Precomputed Lloyd-Max optimal codebooks for Gaussian quantization.
///
/// In high dimensions, each coordinate of a randomly rotated unit vector
/// follows approximately N(0, 1/sqrt(d)). These codebooks are the standard
/// Lloyd-Max levels for N(0,1), scaled by sigma = 1/sqrt(d).

/// A codebook for a specific bit-width.
pub struct Codebook {
    pub centroids: Vec<f32>,
    pub boundaries: Vec<f32>,
    pub num_levels: usize,
}

/// Standard Lloyd-Max centroids for N(0,1).
const GAUSSIAN_CENTROIDS_1BIT: [f32; 2] = [-0.7979, 0.7979];
const GAUSSIAN_BOUNDARIES_1BIT: [f32; 1] = [0.0];

const GAUSSIAN_CENTROIDS_2BIT: [f32; 4] = [-1.5104, -0.4528, 0.4528, 1.5104];
const GAUSSIAN_BOUNDARIES_2BIT: [f32; 3] = [-0.9816, 0.0, 0.9816];

const GAUSSIAN_CENTROIDS_3BIT: [f32; 8] = [
    -2.1520, -1.3440, -0.7560, -0.2451, 0.2451, 0.7560, 1.3440, 2.1520,
];
const GAUSSIAN_BOUNDARIES_3BIT: [f32; 7] = [
    -1.7480, -1.0500, -0.5006, 0.0, 0.5006, 1.0500, 1.7480,
];

const GAUSSIAN_CENTROIDS_4BIT: [f32; 16] = [
    -2.7326, -2.0690, -1.6180, -1.2562,
    -0.9423, -0.6568, -0.3880, -0.1284,
     0.1284,  0.3880,  0.6568,  0.9423,
     1.2562,  1.6180,  2.0690,  2.7326,
];
const GAUSSIAN_BOUNDARIES_4BIT: [f32; 15] = [
    -2.4008, -1.8435, -1.4371, -1.0993,
    -0.7996, -0.5224, -0.2582, 0.0,
     0.2582,  0.5224,  0.7996,  1.0993,
     1.4371,  1.8435,  2.4008,
];

/// Build scaled codebooks for dimension d.
/// Scales the standard Gaussian Lloyd-Max levels by sigma = 1/sqrt(d).
pub fn build_codebooks(d: usize) -> Vec<Codebook> {
    let sigma = 1.0 / (d as f32).sqrt();

    let raw: Vec<(&[f32], &[f32])> = vec![
        (&GAUSSIAN_CENTROIDS_1BIT, &GAUSSIAN_BOUNDARIES_1BIT),
        (&GAUSSIAN_CENTROIDS_2BIT, &GAUSSIAN_BOUNDARIES_2BIT),
        (&GAUSSIAN_CENTROIDS_3BIT, &GAUSSIAN_BOUNDARIES_3BIT),
        (&GAUSSIAN_CENTROIDS_4BIT, &GAUSSIAN_BOUNDARIES_4BIT),
    ];

    let mut codebooks = Vec::new();
    for (centroids, boundaries) in raw {
        let num_levels = centroids.len();
        codebooks.push(Codebook {
            centroids: centroids.iter().map(|c| c * sigma).collect(),
            boundaries: boundaries.iter().map(|b| b * sigma).collect(),
            num_levels,
        });
    }
    codebooks
}

/// Quantize a single scalar to the nearest centroid index (binary search).
pub fn quantize_scalar(value: f32, boundaries: &[f32]) -> u8 {
    let mut lo = 0usize;
    let mut hi = boundaries.len();
    while lo < hi {
        let mid = (lo + hi) >> 1;
        if value > boundaries[mid] {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo as u8
}

/// Quantize a vector of coordinates to index array.
pub fn quantize_vector(rotated: &[f32], codebook: &Codebook) -> Vec<u8> {
    rotated
        .iter()
        .map(|&v| quantize_scalar(v, &codebook.boundaries))
        .collect()
}

/// Dequantize index array back to centroid values.
pub fn dequantize_vector(indices: &[u8], codebook: &Codebook) -> Vec<f32> {
    indices
        .iter()
        .map(|&idx| codebook.centroids[idx as usize])
        .collect()
}

/// Pack quantization indices into compact bytes.
pub fn pack_indices(indices: &[u8], bits: u8) -> Vec<u8> {
    let n = indices.len();
    match bits {
        1 => {
            let mut buf = vec![0u8; (n + 7) / 8];
            for i in 0..n {
                if indices[i] != 0 {
                    buf[i >> 3] |= 1 << (i & 7);
                }
            }
            buf
        }
        2 => {
            let mut buf = vec![0u8; (n + 3) / 4];
            for i in 0..n {
                buf[i >> 2] |= (indices[i] & 0x3) << ((i & 3) * 2);
            }
            buf
        }
        3 => {
            let total_bits = n * 3;
            let mut buf = vec![0u8; (total_bits + 7) / 8];
            for i in 0..n {
                let bit_pos = i * 3;
                let byte_pos = bit_pos >> 3;
                let bit_offset = bit_pos & 7;
                buf[byte_pos] |= (indices[i] & 0x7) << bit_offset;
                if bit_offset > 5 && byte_pos + 1 < buf.len() {
                    buf[byte_pos + 1] |= (indices[i] & 0x7) >> (8 - bit_offset);
                }
            }
            buf
        }
        4 => {
            let mut buf = vec![0u8; (n + 1) / 2];
            for i in 0..n {
                if i & 1 != 0 {
                    buf[i >> 1] |= (indices[i] & 0xF) << 4;
                } else {
                    buf[i >> 1] |= indices[i] & 0xF;
                }
            }
            buf
        }
        _ => panic!("Unsupported bit-width: {}", bits),
    }
}

/// Unpack indices from compact bytes.
pub fn unpack_indices(buf: &[u8], bits: u8, n: usize) -> Vec<u8> {
    let mut indices = vec![0u8; n];
    match bits {
        1 => {
            for i in 0..n {
                indices[i] = (buf[i >> 3] >> (i & 7)) & 1;
            }
        }
        2 => {
            for i in 0..n {
                indices[i] = (buf[i >> 2] >> ((i & 3) * 2)) & 0x3;
            }
        }
        3 => {
            for i in 0..n {
                let bit_pos = i * 3;
                let byte_pos = bit_pos >> 3;
                let bit_offset = bit_pos & 7;
                let mut val = (buf[byte_pos] >> bit_offset) & 0x7;
                if bit_offset > 5 && byte_pos + 1 < buf.len() {
                    val |= (buf[byte_pos + 1] << (8 - bit_offset)) & 0x7;
                }
                indices[i] = val;
            }
        }
        4 => {
            for i in 0..n {
                if i & 1 != 0 {
                    indices[i] = (buf[i >> 1] >> 4) & 0xF;
                } else {
                    indices[i] = buf[i >> 1] & 0xF;
                }
            }
        }
        _ => panic!("Unsupported bit-width: {}", bits),
    }
    indices
}
