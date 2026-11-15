//! glibc's `sinf` and `cosf` (since 2.28, `sysdeps/ieee754/flt-32`): a
//! polynomial in `f64` off a quadrant reduction, under 0.6 ulp and not
//! correctly rounded. A retail Linux server calls the host's `libm`, so
//! where a binary calls `sinf` or `cosf` (CoD4 1.7's `AngleVectors`), a
//! correctly rounded `sin` lands an ulp off now and then
//! (cod4-pmove.md 13.2).

struct Poly {
    sign: [f64; 4],
    c: [f64; 5],
    s: [f64; 3],
}

const fn f(bits: u64) -> f64 {
    f64::from_bits(bits)
}

/// `__sincosf_table`'s polynomials: quadrants 0 and 1, then 2 and 3 (the
/// cosine's coefficients negated). The bits of glibc's hex constants.
const C: [f64; 5] = [
    1.0,
    f(0xbfdf_ffff_fd0c_621c),
    f(0x3fa5_5553_e106_8f19),
    f(0xbf56_c087_e89a_359d),
    f(0x3ef9_9343_027b_f8c3),
];
const S: [f64; 3] = [
    f(0xbfc5_5554_5995_a603),
    f(0x3f81_1076_0523_0bc4),
    f(0xbf29_94eb_3774_cf24),
];
const TABLE: [Poly; 2] = [
    Poly {
        sign: [1.0, -1.0, -1.0, 1.0],
        c: C,
        s: S,
    },
    Poly {
        sign: [1.0, -1.0, -1.0, 1.0],
        c: [-C[0], -C[1], -C[2], -C[3], -C[4]],
        s: S,
    },
];

/// 2 / pi scaled by 2^24, pi / 2, and pi / 2^63.
const HPI_INV: f64 = f(0x4164_5f30_6dc9_c883);
const HPI: f64 = f(0x3ff9_21fb_5444_2d18);
const PI63: f64 = f(0x3c19_21fb_5444_2d18);
/// 4 / pi, 192 bits, as overlapping words (`__inv_pio4`).
const INV_PIO4: [u32; 24] = [
    0xa2, 0xa2f9, 0xa2f983, 0xa2f9836e, 0xf9836e4e, 0x836e4e44, 0x6e4e4415, 0x4e441529, 0x441529fc,
    0x1529fc27, 0x29fc2757, 0xfc2757d1, 0x2757d1f5, 0x57d1f534, 0xd1f534dd, 0xf534ddc0, 0x34ddc0db,
    0xddc0db62, 0xc0db6295, 0xdb629599, 0x6295993c, 0x95993c43, 0x993c4390, 0x3c439041,
];

fn abstop12(x: f32) -> u32 {
    (x.to_bits() >> 20) & 0x7ff
}

/// `sinf_poly`: the sine polynomial for an even quadrant, else the cosine's.
fn poly(x: f64, x2: f64, p: &Poly, n: i32) -> f32 {
    if n & 1 == 0 {
        let x3 = x * x2;
        let s1 = p.s[1] + x2 * p.s[2];
        let x7 = x3 * x2;
        let s = x + x3 * p.s[0];
        (s + x7 * s1) as f32
    } else {
        let x4 = x2 * x2;
        let c2 = p.c[3] + x2 * p.c[4];
        let c1 = p.c[0] + x2 * p.c[1];
        let x6 = x4 * x2;
        let c = c1 + x4 * p.c[2];
        (c + x6 * c2) as f32
    }
}

/// `reduce_fast`, for |x| under 120: x less its nearest multiple of pi / 2.
fn reduce_fast(x: f64) -> (f64, i32) {
    let r = x * HPI_INV;
    let n = ((r as i32).wrapping_add(0x80_0000)) >> 24;
    (x - f64::from(n) * HPI, n)
}

/// `reduce_large`: the same off 4 / pi's bits, for |x| from 120.
fn reduce_large(xi: u32) -> (f64, i32) {
    let arr = &INV_PIO4[((xi >> 26) & 15) as usize..];
    let shift = (xi >> 23) & 7;
    let xi = ((xi & 0xff_ffff) | 0x80_0000) << shift;
    let res0 = u64::from(xi.wrapping_mul(arr[0]));
    let res1 = u64::from(xi) * u64::from(arr[4]);
    let res2 = u64::from(xi) * u64::from(arr[8]);
    let res0 = ((res2 >> 32) | (res0 << 32)).wrapping_add(res1);
    let n = (res0.wrapping_add(1 << 61)) >> 62;
    let res0 = res0.wrapping_sub(n << 62);
    ((res0 as i64) as f64 * PI63, n as i32)
}

/// `sinf` (cos: `odd` 1, the quadrant moved by one).
fn sin_or_cos(y: f32, odd: i32) -> f32 {
    let x = f64::from(y);
    let top = abstop12(y);
    if top < abstop12(std::f32::consts::FRAC_PI_4) {
        if top < abstop12(f32::from_bits(0x3980_0000)) {
            // Under 2^-12: sin x is x, cos x is 1.
            return if odd == 0 { y } else { 1.0 };
        }
        return poly(x, x * x, &TABLE[0], odd);
    }
    // `q` picks the sign and the table, `n` the polynomial: the large
    // path reduces |y| and folds the sign into `q` alone.
    let (x, n, q) = if top < abstop12(120.0) {
        let (x, n) = reduce_fast(x);
        (x, n, n)
    } else if top < abstop12(f32::INFINITY) {
        let (x, n) = reduce_large(y.to_bits());
        (x, n, n + (y.to_bits() >> 31) as i32)
    } else {
        return f32::NAN;
    };
    let p = &TABLE[usize::from(q & 2 != 0)];
    let s = p.sign[(q & 3) as usize];
    poly(x * s, x * x, p, n ^ odd)
}

/// glibc's `sinf`.
pub fn sinf(y: f32) -> f32 {
    sin_or_cos(y, 0)
}

/// glibc's `cosf`.
pub fn cosf(y: f32) -> f32 {
    sin_or_cos(y, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linux's `f32::sin` and `f32::cos` are the host glibc's own.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn matches_the_hosts_glibc() {
        let mut bits = 0x1234_5678u32;
        let check = |x: f32| {
            assert_eq!(sinf(x).to_bits(), x.sin().to_bits(), "sinf({x:e})");
            assert_eq!(cosf(x).to_bits(), x.cos().to_bits(), "cosf({x:e})");
        };
        for _ in 0..2_000_000 {
            bits ^= bits << 13;
            bits ^= bits >> 17;
            bits ^= bits << 5;
            let x = f32::from_bits(bits);
            if x.is_finite() {
                check(x);
            }
            // The radians a view's degrees turn into.
            check((bits as f32 / u32::MAX as f32 - 0.5) * 20.0);
        }
    }

    /// 1.7's `AngleVectors` at a wobbling rocket's yaw 100.63505: `cosf`
    /// is an ulp off the correctly rounded -0.18455263 (cod4-gsc.md 3.2).
    #[test]
    fn rounds_as_glibc_where_it_is_not_correct() {
        let x = f32::from_bits(0x3fe0_d224);
        assert_eq!(cosf(x).to_bits(), 0xbe3c_fb5e);
        assert_eq!((f64::from(x).cos() as f32).to_bits(), 0xbe3c_fb5d);
    }
}
