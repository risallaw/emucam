//! Deterministic counter-based hashing (same in GLSL preview: pcg-style).

#[inline]
pub fn hash64(mut x: u64) -> u64 {
    // splitmix64 finalizer
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// PCG3D hash (matches the GLSL `pcg3d` in shaders) → three u32.
#[inline]
pub fn pcg3d(mut v: [u32; 3]) -> [u32; 3] {
    for c in v.iter_mut() {
        *c = c.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    }
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    for c in v.iter_mut() {
        *c ^= *c >> 16;
    }
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v
}

/// Uniform [0,1) from a u32.
#[inline]
pub fn unit(u: u32) -> f32 {
    (u >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Approximately Gaussian (sum of 3 uniforms, unit variance) from a hash triple.
#[inline]
pub fn gauss3(h: [u32; 3]) -> f32 {
    (unit(h[0]) + unit(h[1]) + unit(h[2]) - 1.5) * 2.0
}

/// Small deterministic RNG for effects (xorshift64*).
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(hash64(seed) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}
