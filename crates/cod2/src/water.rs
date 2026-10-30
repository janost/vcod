//! CoD2's water height map (docs/research/cod2-materials.md 9.12): a
//! Phillips spectrum seeded once per water def (`R_CreateWaterSetup`), run
//! to the view's time by retail's sine table and inverse FFT, its magnitude
//! stored in an L8 image with a box-filtered mip chain each frame
//! (`RB_UploadWaterTexture`). `water.hlsl` reads it as `normalMapSampler`.
//! Every step keeps retail's f32 operation order.

use std::sync::OnceLock;
use vcod_common::material::WaterLit;
use vcod_common::rng::GlibcRand;

/// The gravity `R_LoadWaterSetup`'s caller stores in every setup
/// (0x44480000).
pub const GRAVITY: f32 = 800.0;

/// `rg`'s sine table (1024 steps of a turn), and `_FFT_Init`'s 8-bit
/// reversal and 256-step twiddle tables.
struct Tables {
    sine: [f32; 1024],
    bitrev: [u32; 256],
    twiddle: [[f32; 2]; 256],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let sine = std::array::from_fn(|i| {
            (f64::from(i as f32 * 0.351_562_5) * 0.017_453_292_519_943_295).sin() as f32
        });
        let bitrev = std::array::from_fn(|i| (i as u32 as u8).reverse_bits().into());
        let twiddle = std::array::from_fn(|i| {
            let a = (i as f64 * 0.024_543_692_606_170_26) as f32;
            [a.cos(), a.sin()]
        });
        Tables {
            sine,
            bitrev,
            twiddle,
        }
    })
}

/// `GaussianRandom` (mac 0x2cf48): the polar method over two `rand()`
/// draws scaled by 2^-31.
pub fn gaussian(rng: &mut GlibcRand) -> (f32, f32) {
    let unit = |r: u32| {
        let r = r as f32;
        (r * 4.656_613e-10 + r * 4.656_613e-10) - 1.0
    };
    let (x, y, s) = loop {
        let x = unit(rng.next_u31());
        let y = unit(rng.next_u31());
        let s = x * x + y * y;
        if s <= 1.0 {
            break (x, y, s);
        }
    };
    let s64 = f64::from(s);
    let f = ((s64.ln() * -2.0) / s64) as f32;
    let f = f.sqrt();
    (x * f, f * y)
}

/// `_FFT` (mac 0xfa080): the in-place radix-2 transform of `1 << bits`
/// complex values `stride` apart, a radix-4 first stage, twiddles
/// `e^{+i 2 pi k / 256}` (the inverse sense, unscaled).
fn fft(data: &mut [[f32; 2]], bits: u32, stride: usize) {
    let t = tables();
    let n = 1usize << bits;
    for i in 0..n {
        let j = (t.bitrev[i] >> (8 - bits)) as usize;
        if j < i {
            data.swap(i * stride, j * stride);
        }
    }
    let end = n * stride;
    let mut base = 0;
    while base < end {
        let [x0r, x0i] = data[base];
        let [x1r, x1i] = data[base + stride];
        let [x2r, x2i] = data[base + 2 * stride];
        let [x3r, x3i] = data[base + 3 * stride];
        data[base] = [x0r + x1r + x2r + x3r, x0i + x1i + x2i + x3i];
        data[base + 2 * stride] = [(x0r + x1r) - (x2r + x3r), (x0i + x1i) - (x2i + x3i)];
        data[base + stride] = [(x0r - x1r) - (x2i - x3i), (x2r - x3r) + (x0i - x1i)];
        data[base + 3 * stride] = [(x0r - x1r) + (x2i - x3i), (x0i - x1i) - (x2r - x3r)];
        base += 4 * stride;
    }
    if n < 5 {
        return;
    }
    let (mut half, mut size, mut shift) = (4usize, 8usize, 5u32);
    loop {
        for k in 0..half {
            let [c, s] = t.twiddle[k << shift];
            let mut u = stride * k;
            while u < end {
                let q = u + half * stride;
                let [qr, qi] = data[q];
                let tr = qr * c - qi * s;
                let ti = qi * c + qr * s;
                let [pr, pi] = data[u];
                data[q] = [pr - tr, pi - ti];
                data[u] = [tr + pr, ti + pi];
                u += stride * size;
            }
        }
        half = size;
        if n <= half {
            return;
        }
        shift -= 1;
        size = half * 2;
    }
}

/// One water def's spectrum: `water_t`'s `H0` and angular frequency per
/// texel, row-major from the most negative frequency (no shift: the field
/// comes out signed by `(-1)^(x + y)`, which the magnitude drops).
#[derive(Clone, Debug)]
pub struct WaterSetup {
    /// Columns (`water_t + 0xc`) and rows (`+ 0x10`), both `textureWidth`.
    pub width: usize,
    pub height: usize,
    h0: Vec<[f32; 2]>,
    omega: Vec<f32>,
}

impl WaterSetup {
    /// `R_CreateWaterSetup` (mac 0x109c00) over the def `R_LoadWaterSetup`'s
    /// caller fills (mac 0x10459e): both sizes `textureWidth`, x length
    /// `horizontalWorldLength` on the rows, `verticalWorldLength` on the
    /// columns, [`GRAVITY`]. One Gaussian pair is drawn per texel, also
    /// where the wind zeroes it.
    pub fn new(w: &WaterLit, rng: &mut GlibcRand) -> Self {
        let side = w.width.clamp(4, 256) as usize;
        assert!(side.is_power_of_two(), "water width {side}");
        let (cols, rows) = (side as i32, side as i32);
        let [dir_x, dir_y] = w.wind_direction;
        let v2 = w.wind_speed * w.wind_speed;
        let g = GRAVITY;
        let d_row = (std::f64::consts::TAU / f64::from(rows as f32 * w.lengths[0])) as f32;
        let d_col = (std::f64::consts::TAU / f64::from(cols as f32 * w.lengths[1])) as f32;
        let mut h0 = Vec::with_capacity(side * side);
        let mut omega = Vec::with_capacity(side * side);
        for r in -rows / 2..rows / 2 {
            let kr = d_row * r as f32;
            for c in -cols / 2..cols / 2 {
                let kc = c as f32 * d_col;
                let (gr, gi) = gaussian(rng);
                let k2 = kc * kc + kr * kr;
                let kd = kr * dir_x + kc * dir_y;
                // retail branches on `0 < k.d`, so NaN zeroes too
                if kd <= 0.0 || kd.is_nan() {
                    h0.push([0.0; 2]);
                    omega.push(0.0);
                    continue;
                }
                let e = f64::from(-1.0 / (((v2 * v2) / g) * k2)).exp() as f32;
                let a = (((w.amplitude * e) / (k2 * k2 * k2)) * kd * kd * 0.5).sqrt() * w.amplitude;
                h0.push([a * gr, a * gi]);
                omega.push((k2.sqrt() * g).sqrt());
            }
        }
        WaterSetup {
            width: side,
            height: side,
            h0,
            omega,
        }
    }

    /// The complex field at `time` seconds (`backEnd + 0x3bc`, the view's
    /// time): each texel's `H0` real part by the table's cosine and its
    /// imaginary part by its sine of `omega t` in 1024ths of a turn, then
    /// an FFT along every row and down every column.
    pub fn field(&self, time: f32) -> Vec<[f32; 2]> {
        self.field_by(|om| {
            ((f64::from(om * time) * 162.974_661_726_100_83) as f32 + 0.5).floor() as i32
        })
    }

    /// The setup of a spectrum a CoD4 zone stores (`water_t`'s `H0` and
    /// `wTerm`, `width` square, row-major as [`Self::new`] lays it out).
    pub fn from_spectrum(width: usize, h0: Vec<[f32; 2]>, omega: Vec<f32>) -> Self {
        assert!(width.is_power_of_two() && h0.len() == width * width && omega.len() == h0.len());
        WaterSetup {
            width,
            height: width,
            h0,
            omega,
        }
    }

    /// CoD4 1.7's field at `time` seconds (iw3mp.exe 0x616240): the time
    /// scaled by 162.97466 and stored, each texel's phase its `wTerm` times
    /// that, truncated, in 1024ths of a turn; the FFTs as [`Self::field`].
    pub fn field_cod4(&self, time: f32) -> Vec<[f32; 2]> {
        let scaled = (f64::from(time) * 162.974_655_151_367_2) as f32;
        self.field_by(|om| (f64::from(om) * f64::from(scaled)) as i64 as i32)
    }

    /// The field off each texel's phase index (`phase(omega)`).
    fn field_by(&self, phase: impl Fn(f32) -> i32) -> Vec<[f32; 2]> {
        let sine = &tables().sine;
        let mut out: Vec<[f32; 2]> = self
            .h0
            .iter()
            .zip(&self.omega)
            .map(|(&[hr, hi], &om)| {
                if om == 0.0 {
                    return [0.0; 2];
                }
                let p = (phase(om) & 0x3ff) as usize;
                [hr * sine[(p + 0xff) & 0x3ff], hi * sine[p]]
            })
            .collect();
        let (w, h) = (self.width, self.height);
        let (wb, hb) = (w.trailing_zeros(), h.trailing_zeros());
        for row in out.chunks_exact_mut(w) {
            fft(row, wb, 1);
        }
        for col in 0..w {
            fft(&mut out[col..], hb, w);
        }
        out
    }

    /// The L8 image's levels at `time`: the field's magnitude over the
    /// texel count, saturated, in 255ths rounded half up; each further
    /// level `R_DownsampleMipMapBilinear`'s truncated 2 x 2 mean, down to
    /// 1 x 1.
    pub fn levels(&self, time: f32) -> Vec<Vec<u8>> {
        let n = (self.width * self.height) as f32;
        let top: Vec<u8> = self
            .field(time)
            .iter()
            .map(|&[re, im]| {
                let v = (re * re + im * im).sqrt() / n;
                let b = if v - 1.0 < 0.0 { v * 255.0 } else { 255.0 };
                (b + 0.5).floor() as u8
            })
            .collect();
        self.mips(top)
    }

    /// CoD4 1.7's L8 levels at `time` (iw3mp.exe 0x616420): the field's
    /// magnitude times the stored inverse of the texel count, 1 past 1,
    /// times 255.99899 truncated; the mips as [`Self::levels`].
    pub fn levels_cod4(&self, time: f32) -> Vec<Vec<u8>> {
        let inv = 1.0 / (self.width * self.height) as f32;
        let top: Vec<u8> = self
            .field_cod4(time)
            .iter()
            .map(|&[re, im]| {
                let v = (im * im + re * re).sqrt() * inv;
                let v = if 1.0 - v <= 0.0 { 1.0 } else { v };
                (f64::from(v) * 255.998_992_919_921_9) as u8
            })
            .collect();
        self.mips(top)
    }

    /// `top` and its box-filtered levels down to 1 x 1.
    fn mips(&self, top: Vec<u8>) -> Vec<Vec<u8>> {
        let mut out = vec![top];
        let (mut w, mut h) = (self.width, self.height);
        while w > 1 || h > 1 {
            let src = out.last().expect("a level");
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut next = Vec::with_capacity(nw * nh);
            for y in 0..nh {
                for x in 0..nw {
                    let at = |dx: usize, dy: usize| {
                        u32::from(src[(2 * y + dy).min(h - 1) * w + (2 * x + dx).min(w - 1)])
                    };
                    let sum = if w > 1 && h > 1 {
                        (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) >> 2
                    } else {
                        (at(0, 0) + at(1, 1)) >> 1
                    };
                    next.push(sum as u8);
                }
            }
            out.push(next);
            (w, h) = (nw, nh);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One stored amplitude transforms to the same magnitude on every
    /// texel; CoD4's phase truncates the scaled time times `wTerm`, and its
    /// byte truncates the magnitude over the count times 255.99899.
    #[test]
    fn a_cod4_spectrum_of_one_wave_is_flat() {
        let mut h0 = vec![[0.0f32; 2]; 16];
        let mut omega = vec![0.0f32; 16];
        h0[5] = [9.0, 5.0];
        omega[5] = 3.3;
        let w = WaterSetup::from_spectrum(4, h0, omega);
        let t = 1.234f32;
        let scaled = (f64::from(t) * 162.974_655_151_367_2) as f32;
        let p = ((f64::from(3.3f32) * f64::from(scaled)) as i64 & 0x3ff) as usize;
        let sine = &tables().sine;
        let (re, im) = (9.0 * sine[(p + 0xff) & 0x3ff], 5.0 * sine[p]);
        let want = ((re * re + im * im).sqrt() * (1.0 / 16.0)).min(1.0);
        let want = (f64::from(want) * 255.998_992_919_921_9) as u8;
        let levels = w.levels_cod4(t);
        assert_eq!(levels.len(), 3);
        for &b in &levels[0] {
            assert!(b.abs_diff(want) <= 1, "{b} vs {want}");
        }
        assert!(want > 10, "{want}");
    }

    /// The direct inverse DFT, `sum x_j e^{+i 2 pi j k / n}`, in f64.
    fn dft(x: &[[f32; 2]]) -> Vec<[f64; 2]> {
        let n = x.len();
        (0..n)
            .map(|k| {
                x.iter()
                    .enumerate()
                    .fold([0.0; 2], |[ar, ai], (j, &[r, i])| {
                        let a = std::f64::consts::TAU * (j * k) as f64 / n as f64;
                        let (s, c) = a.sin_cos();
                        let (r, i) = (f64::from(r), f64::from(i));
                        [ar + r * c - i * s, ai + r * s + i * c]
                    })
            })
            .collect()
    }

    #[test]
    fn fft_is_the_unscaled_inverse_dft() {
        let mut rng = GlibcRand::new(7);
        for bits in [2, 3, 6, 8] {
            let n = 1 << bits;
            let x: Vec<[f32; 2]> = (0..n)
                .map(|_| {
                    let (a, b) = gaussian(&mut rng);
                    [a, b]
                })
                .collect();
            let want = dft(&x);
            let mut got = x.clone();
            fft(&mut got, bits, 1);
            for (g, w) in got.iter().zip(&want) {
                for c in 0..2 {
                    assert!((f64::from(g[c]) - w[c]).abs() < 1e-3, "{bits}: {g:?} {w:?}");
                }
            }
            // strided: every other slot
            let mut wide = vec![[0.0f32; 2]; 2 * n];
            for (i, v) in x.iter().enumerate() {
                wide[2 * i] = *v;
            }
            fft(&mut wide, bits, 2);
            for i in 0..n {
                assert_eq!(wide[2 * i], got[i]);
            }
        }
    }

    #[test]
    fn gaussian_draws_have_unit_variance() {
        let mut rng = GlibcRand::new(1);
        let n = 20_000;
        let (mut sum, mut sq) = (0.0f64, 0.0f64);
        for _ in 0..n {
            let (a, b) = gaussian(&mut rng);
            for v in [a, b] {
                sum += f64::from(v);
                sq += f64::from(v) * f64::from(v);
            }
        }
        let mean = sum / (2 * n) as f64;
        let var = sq / (2 * n) as f64 - mean * mean;
        assert!(
            mean.abs() < 0.03 && (var - 1.0).abs() < 0.05,
            "{mean} {var}"
        );
    }

    fn harbor() -> WaterLit {
        WaterLit {
            color: [0.2, 0.3, 0.4, 1.0],
            cube: "images/elalamein_water_ft.iwi".into(),
            width: 64,
            lengths: [37.0, 37.0],
            amplitude: 0.06,
            wind_speed: 76.0,
            wind_direction: [1.0, 0.0],
        }
    }

    /// The wind zeroes every frequency not along it, and a frequency's
    /// motion follows `omega = sqrt(g |k|)`.
    #[test]
    fn the_spectrum_follows_the_wind() {
        let s = WaterSetup::new(&harbor(), &mut GlibcRand::new(1));
        assert_eq!((s.width, s.height, s.h0.len()), (64, 64, 4096));
        for r in 0..64 {
            for c in 0..64 {
                let i = r * 64 + c;
                // direction (1, 0) pairs with the row frequency
                if r <= 32 {
                    assert_eq!((s.h0[i], s.omega[i]), ([0.0; 2], 0.0), "{r} {c}");
                } else {
                    let kr = (std::f64::consts::TAU / (64.0 * 37.0)) * (r as f64 - 32.0);
                    let kc = (std::f64::consts::TAU / (64.0 * 37.0)) * (c as f64 - 32.0);
                    let want = ((kr * kr + kc * kc).sqrt() * 800.0).sqrt();
                    assert!((f64::from(s.omega[i]) - want).abs() < 1e-3 * want);
                }
            }
        }
    }

    /// The height map moves with time, and each level is the truncated
    /// mean of the one above.
    #[test]
    fn levels_move_and_mip_down() {
        let s = WaterSetup::new(&harbor(), &mut GlibcRand::new(1));
        let a = s.levels(10.0);
        let b = s.levels(10.5);
        assert_eq!(a.len(), 7);
        assert_eq!(
            a.iter().map(Vec::len).collect::<Vec<_>>(),
            [4096, 1024, 256, 64, 16, 4, 1]
        );
        assert_ne!(a[0], b[0]);
        assert!(a[0].iter().any(|&v| v > 0));
        let (t, l1) = (&a[0], &a[1]);
        let m = (u32::from(t[0]) + u32::from(t[1]) + u32::from(t[64]) + u32::from(t[65])) >> 2;
        assert_eq!(u32::from(l1[0]), m);
    }
}
