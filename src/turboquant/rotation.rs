/// Random rotation via Fast Walsh-Hadamard Transform (FWHT) + random sign flips.
///
/// rotate(x) = (1/sqrt(d)) * H * diag(signs) * x
///
/// where H is the Hadamard matrix (applied via FWHT in O(d log d))
/// and signs is a random ±1 diagonal.

/// Deterministic PRNG (xorshift128+) for reproducible sign flips.
pub struct Rng {
    s0: u32,
    s1: u32,
}

impl Rng {
    pub fn from_seed(seed: &str) -> Self {
        let mut s0: u32 = 0;
        let mut s1: u32 = 0;
        for b in seed.bytes() {
            s0 = s0.wrapping_mul(31).wrapping_add(b as u32);
            s1 = s1.wrapping_mul(37).wrapping_add(b as u32);
        }
        if s0 == 0 { s0 = 0x12345678; }
        if s1 == 0 { s1 = 0x87654321; }
        Self { s0, s1 }
    }

    pub fn next(&mut self) -> u32 {
        let mut x = self.s0;
        let y = self.s1;
        self.s0 = y;
        x ^= x << 23;
        x ^= x >> 17;
        x ^= y;
        x ^= y >> 26;
        self.s1 = x;
        self.s0.wrapping_add(self.s1)
    }
}

/// Generate random ±1 sign flips from a seed.
pub fn generate_sign_flips(seed: &str, d: usize) -> Vec<i8> {
    let mut rng = Rng::from_seed(seed);
    let mut signs = vec![0i8; d];
    for i in 0..d {
        signs[i] = if (rng.next() & 1) != 0 { 1 } else { -1 };
    }
    signs
}

/// In-place Fast Walsh-Hadamard Transform with 1/sqrt(n) normalization.
/// Input length must be a power of 2.
pub fn fwht(arr: &mut [f32]) {
    let n = arr.len();
    let mut len = 1;
    while len < n {
        let mut i = 0;
        while i < n {
            for j in 0..len {
                let u = arr[i + j];
                let v = arr[i + j + len];
                arr[i + j] = u + v;
                arr[i + j + len] = u - v;
            }
            i += len << 1;
        }
        len <<= 1;
    }
    let scale = 1.0 / (n as f32).sqrt();
    for x in arr.iter_mut() {
        *x *= scale;
    }
}

/// Apply random rotation: signs then FWHT. Modifies in-place.
pub fn apply_rotation(vector: &mut [f32], signs: &[i8]) {
    let d = vector.len();
    for i in 0..d {
        vector[i] *= signs[i] as f32;
    }
    fwht(vector);
}

/// Apply inverse rotation: FWHT then signs. Modifies in-place.
pub fn apply_inverse_rotation(vector: &mut [f32], signs: &[i8]) {
    fwht(vector);
    let d = vector.len();
    for i in 0..d {
        vector[i] *= signs[i] as f32;
    }
}
