//! Planar-free, interleaved RGB f32 image used throughout the pipeline.

use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct ImageF32 {
    pub w: usize,
    pub h: usize,
    /// Interleaved RGB, row-major, len = w*h*3.
    pub data: Vec<f32>,
}

impl ImageF32 {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            data: vec![0.0; w * h * 3],
        }
    }

    pub fn filled(w: usize, h: usize, c: [f32; 3]) -> Self {
        let mut img = Self::new(w, h);
        img.data
            .chunks_exact_mut(3)
            .for_each(|p| p.copy_from_slice(&c));
        img
    }

    #[inline]
    pub fn idx(&self, x: usize, y: usize) -> usize {
        (y * self.w + x) * 3
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> [f32; 3] {
        let i = self.idx(x, y);
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, c: [f32; 3]) {
        let i = self.idx(x, y);
        self.data[i..i + 3].copy_from_slice(&c);
    }

    /// Clamped integer fetch.
    #[inline]
    pub fn get_clamped(&self, x: isize, y: isize) -> [f32; 3] {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.get(x, y)
    }

    /// Bilinear sample at pixel-space coordinates (pixel centers at +0.5).
    pub fn sample(&self, fx: f32, fy: f32) -> [f32; 3] {
        let x = fx - 0.5;
        let y = fy - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let tx = x - x0;
        let ty = y - y0;
        let (x0, y0) = (x0 as isize, y0 as isize);
        let a = self.get_clamped(x0, y0);
        let b = self.get_clamped(x0 + 1, y0);
        let c = self.get_clamped(x0, y0 + 1);
        let d = self.get_clamped(x0 + 1, y0 + 1);
        let mut o = [0.0; 3];
        for k in 0..3 {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bot = c[k] + (d[k] - c[k]) * tx;
            o[k] = top + (bot - top) * ty;
        }
        o
    }

    /// Sample with normalized coordinates (0..1).
    pub fn sample_uv(&self, u: f32, v: f32) -> [f32; 3] {
        self.sample(u * self.w as f32, v * self.h as f32)
    }

    /// Apply a per-pixel function in parallel, giving (x, y, &mut [f32;3]).
    pub fn par_map_pixels<F>(&mut self, f: F)
    where
        F: Fn(usize, usize, &mut [f32]) + Sync + Send,
    {
        let w = self.w;
        self.data
            .par_chunks_exact_mut(w * 3)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, p) in row.chunks_exact_mut(3).enumerate() {
                    f(x, y, p);
                }
            });
    }

    /// Box downsample by an integer factor (area average).
    pub fn downsample(&self, factor: usize) -> ImageF32 {
        if factor <= 1 {
            return self.clone();
        }
        let nw = self.w.div_ceil(factor).max(1);
        let nh = self.h.div_ceil(factor).max(1);
        let mut out = ImageF32::new(nw, nh);
        out.data
            .par_chunks_exact_mut(nw * 3)
            .enumerate()
            .for_each(|(oy, row)| {
                let y0 = oy * factor;
                let y1 = (y0 + factor).min(self.h);
                for ox in 0..nw {
                    let x0 = ox * factor;
                    let x1 = (x0 + factor).min(self.w);
                    let mut acc = [0.0f32; 3];
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let p = self.get(x, y);
                            acc[0] += p[0];
                            acc[1] += p[1];
                            acc[2] += p[2];
                        }
                    }
                    let n = ((y1 - y0) * (x1 - x0)) as f32;
                    row[ox * 3..ox * 3 + 3].copy_from_slice(&[acc[0] / n, acc[1] / n, acc[2] / n]);
                }
            });
        out
    }

    /// Bilinear resize to an exact size (used to upsample low-res buffers).
    pub fn resize(&self, w: usize, h: usize) -> ImageF32 {
        let mut out = ImageF32::new(w, h);
        let sx = self.w as f32 / w as f32;
        let sy = self.h as f32 / h as f32;
        out.par_map_pixels(|x, y, p| {
            let c = self.sample((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
            p.copy_from_slice(&c);
        });
        out
    }
}

/// Single-channel f32 image (masks, CoC maps, noise fields).
#[derive(Clone, Debug)]
pub struct MaskF32 {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl MaskF32 {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            data: vec![0.0; w * h],
        }
    }

    #[inline]
    pub fn get_clamped(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.data[y * self.w + x]
    }

    pub fn sample(&self, fx: f32, fy: f32) -> f32 {
        let x = fx - 0.5;
        let y = fy - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let tx = x - x0;
        let ty = y - y0;
        let (x0, y0) = (x0 as isize, y0 as isize);
        let a = self.get_clamped(x0, y0);
        let b = self.get_clamped(x0 + 1, y0);
        let c = self.get_clamped(x0, y0 + 1);
        let d = self.get_clamped(x0 + 1, y0 + 1);
        let top = a + (b - a) * tx;
        let bot = c + (d - c) * tx;
        top + (bot - top) * ty
    }

    pub fn sample_uv(&self, u: f32, v: f32) -> f32 {
        self.sample(u * self.w as f32, v * self.h as f32)
    }
}
