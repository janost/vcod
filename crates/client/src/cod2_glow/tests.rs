use super::*;
use wgpu::util::DeviceExt;

/// At radius 1.25 (glow level 0 at a width divisible by four) the first
/// pair shares the centre texel: 0.5 * 1 at 0 and exp(-0.32) at 1, the
/// second pair exp(-1.28) at 2 and exp(-2.88) at 3, normalised to 0.5.
#[test]
fn points_follow_the_gaussian_and_share_the_centre() {
    let t = points_1d(1.25, 160, 160, 2);
    let w = |x: f32| (-0.5 * x * x / (1.25 * 1.25)).exp();
    let (a, b) = (0.5 * w(0.0) + w(1.0), w(2.0) + w(3.0));
    let n = 0.5 / (a + b);
    assert_eq!(t.len(), 2);
    assert!((t[0][1] - a * n).abs() < 1e-6, "{t:?}");
    assert!((t[1][1] - b * n).abs() < 1e-6, "{t:?}");
    assert!((t[0][0] - w(1.0) / a / 160.0).abs() < 1e-7, "{t:?}");
    assert!((t[1][0] - (2.0 * w(2.0) + 3.0 * w(3.0)) / b / 160.0).abs() < 1e-7);
    // an even ratio moves the grid half a texel
    let d = points_1d(1.25, 640, 160, 2);
    assert!((d[0][0] * 640.0 - (0.5 * w(0.5) + 1.5 * w(1.5)) / (w(0.5) + w(1.5))).abs() < 1e-4);
}

/// Eight pairs at a radius of 2.7 run past 0.01 by the sixth: the trim
/// keeps the first pair under the floor and drops the rest.
#[test]
fn points_trim_after_the_first_small_pair() {
    let t = points_1d(2.7, 160, 160, 8);
    let first_small = t.iter().position(|p| p[1] < TAP_FLOOR).unwrap();
    assert_eq!(first_small, t.len() - 1, "{t:?}");
    assert!(t.len() < 8);
}

/// Retail's defaults at 1280 wide: level 0's 1.25 quarter texels in one
/// 2D pass, level 1's remaining sqrt(3^2 - 1.25^2) as a vertical then a
/// horizontal 1D pass; with a sky the first apply bleeds it at 0.5, the
/// second blooms only. Without a sky both bloom.
#[test]
fn default_plan_is_two_levels() {
    let s = GlowSettings::default();
    let p = plan(&s, 1280, 720, true).unwrap();
    assert_eq!(p.setup, [0.9, 1.0 / (1.0 - 0.9), 0.0, 0.75]);
    assert_eq!(p.chains.len(), 2);
    assert_eq!(p.chains[0].len(), 1);
    assert_eq!(p.chains[0][0].taps.len(), 8);
    assert_eq!(p.chains[1].len(), 2);
    assert!(
        p.chains[1][0]
            .taps
            .iter()
            .all(|t| t[0] == 0.0 && t[1] != 0.0)
    );
    assert!(
        p.chains[1][1]
            .taps
            .iter()
            .all(|t| t[1] == 0.0 && t[0] != 0.0)
    );
    assert_eq!(
        p.applies,
        vec![
            Apply {
                sky_bleed: 0.5,
                bloom: 1.0
            },
            Apply {
                sky_bleed: 0.0,
                bloom: 1.0
            }
        ]
    );
    let dark = plan(&s, 1280, 720, false).unwrap();
    assert!(dark.applies.iter().all(|a| a.sky_bleed == 0.0));
    // a 2D pass reads each of its eight weights twice: they sum to a half
    let sum: f32 = p.chains[0][0].taps.iter().map(|t| t[3]).sum();
    assert!((sum - 0.5).abs() < 1e-6, "{sum}");
    let off = GlowSettings {
        bloom: [0.0; 2],
        ..s
    };
    assert!(plan(&off, 1280, 720, false).is_none());
}

/// A radius past one 1D pass takes several, each 6.5 off in quadrature.
#[test]
fn wide_radius_splits_into_passes() {
    let c = chain(20.0, 20.0, 400, 300);
    // y and x alternate while both stay wide
    assert!(c.len() >= 6, "{}", c.len());
    assert!(c.len() <= MAX_CHAIN);
}

/// CoD4's radius is a quarter of the quarter's virtual units: mp_bog's
/// 25.27 at 1080 high is 3.55 quarter texels, past one 2D pass, so a
/// vertical and a horizontal 1D pass; mp_crash_snow's 7 at 768 high is
/// 0.7, one 2D pass; mp_citystreets' 1 at 480 high is under the least
/// radius, the setup alone. Off at intensity or radius 0.
#[test]
fn cod4_plan_scales_the_radius_twice() {
    let g = |radius| Cod4Glow {
        cutoff: 0.925522,
        desaturation: 0.75,
        intensity: 1.42171,
        radius,
    };
    let film = crate::post::Film::OFF.setup(1.0);
    let p = plan_cod4(&g(25.2691), film, 1920, 1080).unwrap();
    assert_eq!(p.film, Some(film));
    assert!(p.applies.is_empty());
    assert_eq!(p.setup, [0.925522, 1.0 / (1.0 - 0.925522), 0.0, 0.75]);
    assert_eq!(p.chains.len(), 1);
    let c = &p.chains[0];
    assert_eq!(c.len(), 2, "{c:?}");
    assert!(c[0].taps.iter().all(|t| t[0] == 0.0));
    assert!(c[1].taps.iter().all(|t| t[1] == 0.0));
    let r = 25.2691 * 0.25 * 0.25 * 1080.0 / 480.0;
    assert_eq!(c[0], chain(r, r, 480, 270)[0]);
    let snow = plan_cod4(&g(7.0), film, 1024, 768).unwrap();
    assert_eq!(snow.chains[0].len(), 1);
    assert_eq!(snow.chains[0][0], filter_2d(0.7, (256, 192), (256, 192)));
    let streets = plan_cod4(&g(1.0), film, 640, 480).unwrap();
    assert!(streets.chains[0].is_empty());
    assert!(
        plan_cod4(
            &Cod4Glow {
                intensity: 0.0,
                ..g(7.0)
            },
            film,
            640,
            480
        )
        .is_none()
    );
    assert!(plan_cod4(&g(0.0), film, 640, 480).is_none());
}

// ---- the chain on a software adapter against the HLSL on the CPU ----

#[derive(Clone)]
struct Img {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Img {
    fn new(w: usize, h: usize) -> Img {
        Img {
            w,
            h,
            px: vec![[0.0; 4]; w * h],
        }
    }

    /// `tex2D` with a linear filter and a clamped edge, texel i's centre at
    /// (i + 0.5) / size.
    fn sample(&self, uv: [f32; 2]) -> [f32; 4] {
        let fx = uv[0] * self.w as f32 - 0.5;
        let fy = uv[1] * self.h as f32 - 0.5;
        let (x0, y0) = (fx.floor(), fy.floor());
        let (ax, ay) = (fx - x0, fy - y0);
        let at = |x: f32, y: f32| {
            let x = (x as i64).clamp(0, self.w as i64 - 1) as usize;
            let y = (y as i64).clamp(0, self.h as i64 - 1) as usize;
            self.px[y * self.w + x]
        };
        let mut out = [0.0; 4];
        for (k, o) in out.iter_mut().enumerate() {
            let top = at(x0, y0)[k] * (1.0 - ax) + at(x0 + 1.0, y0)[k] * ax;
            let bot = at(x0, y0 + 1.0)[k] * (1.0 - ax) + at(x0 + 1.0, y0 + 1.0)[k] * ax;
            *o = top * (1.0 - ay) + bot * ay;
        }
        out
    }

    /// Every pixel of a target the size of this one, through `f(uv)`,
    /// stored as an 8-bit target stores it.
    fn draw(&mut self, f: impl Fn([f32; 2]) -> [f32; 4]) {
        for y in 0..self.h {
            for x in 0..self.w {
                let uv = [
                    (x as f32 + 0.5) / self.w as f32,
                    (y as f32 + 0.5) / self.h as f32,
                ];
                self.px[y * self.w + x] = f(uv).map(byte);
            }
        }
    }
}

fn byte(v: f32) -> f32 {
    (v.clamp(0.0, 1.0) * 255.0).round() / 255.0
}

fn luma(c: [f32; 4]) -> f32 {
    c[0] * 0.299 + c[1] * 0.587 + c[2] * 0.114
}

/// glow_setup.hlsl's pixel shader at `uv` of a `q`-sized target.
fn hlsl_setup(scene: &Img, uv: [f32; 2], q: (usize, usize), g: [f32; 4]) -> [f32; 4] {
    let (hx, hy) = (0.5 / q.0 as f32, 0.5 / q.1 as f32);
    let mut c = [0.0f32; 4];
    for (dx, dy) in [(-hx, -hy), (-hx, hy), (hx, -hy), (hx, hy)] {
        let s = scene.sample([uv[0] + dx, uv[1] + dy]);
        let i = luma(s);
        let non_sky = (4.0 * s[3]).clamp(0.0, 1.0);
        for k in 0..3 {
            let v = if i >= g[0] {
                1.0 + (s[k] - 1.0) * g[1]
            } else {
                0.0
            };
            c[k] += v * non_sky;
        }
        c[3] += 1.0 - non_sky;
    }
    let c = c.map(|v| v * 0.25);
    let i = luma(c);
    [
        c[0] + (i - c[0]) * g[3],
        c[1] + (i - c[1]) * g[3],
        c[2] + (i - c[2]) * g[3],
        c[3],
    ]
}

/// CoD4's glow_consistent_setup.hlsl at `uv` of a `q`-sized target, the
/// film constants as `Plan::film` holds them (identityLight 1).
fn hlsl_setup4(
    scene: &Img,
    uv: [f32; 2],
    q: (usize, usize),
    g: [f32; 4],
    f: [[f32; 4]; 3],
) -> [f32; 4] {
    let (hx, hy) = (0.5 / q.0 as f32, 0.5 / q.1 as f32);
    let mut c = [0.0f32; 3];
    for (dx, dy) in [(-hx, -hy), (-hx, hy), (hx, -hy), (hx, hy)] {
        let s = scene.sample([uv[0] + dx, uv[1] + dy]);
        let l = luma(s);
        let w = (l - g[0]).clamp(0.0, 1.0);
        for k in 0..3 {
            let film = (s[k] * f[0][3] + l) * (f[2][k] * l + f[1][k]) + f[0][k];
            c[k] += w * film * g[1];
        }
    }
    let c = c.map(|v| v * 0.25);
    let l = luma([c[0], c[1], c[2], 0.0]);
    [
        c[0] + (l - c[0]) * g[3],
        c[1] + (l - c[1]) * g[3],
        c[2] + (l - c[2]) * g[3],
        1.0,
    ]
}

fn hlsl_filter(src: &Img, uv: [f32; 2], pass: &FilterPass) -> [f32; 4] {
    let mut c = [0.0f32; 4];
    for t in &pass.taps {
        let a = src.sample([uv[0] + t[0], uv[1] + t[1]]);
        let b = src.sample([uv[0] - t[0], uv[1] - t[1]]);
        for k in 0..4 {
            c[k] += t[3] * (a[k] + b[k]);
        }
    }
    c
}

const W: usize = 96;
const H: usize = 64;
const SKY_RGB: [f32; 3] = [0.2, 0.4, 0.8];

/// Bright and dull blocks under a sky band (alpha 0), in bytes.
fn test_scene() -> Img {
    let mut scene = Img::new(W, H);
    for y in 0..H {
        for x in 0..W {
            scene.px[y * W + x] = if y < 12 {
                [0.55, 0.65, 0.85, 0.0]
            } else if (40..56).contains(&x) && (20..36).contains(&y) {
                [1.0, 0.98, 0.9, 1.0]
            } else if (10..18).contains(&x) && (40..60).contains(&y) {
                [1.0, 0.85, 0.3, 1.0]
            } else if (x + y) % 7 == 0 {
                [0.97, 0.97, 0.97, 1.0]
            } else {
                [0.3, 0.28, 0.25, 1.0]
            }
            .map(byte);
        }
    }
    scene
}

/// The glow of `p` on `scene` through the HLSL, every target in bytes.
fn cpu_glow(scene: &Img, p: &Plan) -> Img {
    let (qw, qh) = quarter(W as u32, H as u32);
    let q = (qw as usize, qh as usize);
    let mut cur = Img::new(q.0, q.1);
    match p.film {
        Some(f) => cur.draw(|uv| hlsl_setup4(scene, uv, q, p.setup, f)),
        None => cur.draw(|uv| hlsl_setup(scene, uv, q, p.setup)),
    }
    for pass in p.chains.iter().flatten() {
        let src = cur.clone();
        cur.draw(|uv| hlsl_filter(&src, uv, pass));
    }
    let mut want = scene.clone();
    for a in &p.applies {
        let before = want.clone();
        want.draw(|uv| {
            let b = cur.sample(uv);
            let d = before.sample(uv);
            if a.sky_bleed != 0.0 {
                let al = (b[3] * 2.0).clamp(0.0, 1.0) * a.sky_bleed;
                let mut o = [0.0; 4];
                for k in 0..3 {
                    o[k] = SKY_RGB[k] * al + b[k] * a.bloom + d[k] * (1.0 - al);
                }
                o[3] = al + d[3] * (1.0 - al);
                o
            } else {
                let mut o = [0.0; 4];
                for k in 0..3 {
                    o[k] = b[k] * a.bloom + d[k];
                }
                o[3] = b[3] + d[3];
                o
            }
        });
    }
    want
}

/// The blur of `b` from `source` (the scene before the glow) drawn over
/// `onto` with `feedbackblend`'s SrcAlpha, InvSrcAlpha.
fn cpu_blur(source: &Img, onto: &Img, b: &BlurPlan) -> Img {
    let (qw, qh) = quarter(W as u32, H as u32);
    let mut cur = Img::new(qw as usize, qh as usize);
    for (i, pass) in b.chain.iter().enumerate() {
        let src = if i == 0 { source.clone() } else { cur.clone() };
        cur.draw(|uv| hlsl_filter(&src, uv, pass));
    }
    let mut want = onto.clone();
    want.draw(|uv| {
        let c = cur.sample(uv);
        let d = onto.sample(uv);
        let a = b.alpha;
        [
            c[0] * a + d[0] * (1.0 - a),
            c[1] * a + d[1] * (1.0 - a),
            c[2] * a + d[2] * (1.0 - a),
            a * a + d[3] * (1.0 - a),
        ]
    });
    want
}

/// `Cod2Glow::encode` on `scene` on the adapter, read back in bytes.
fn gpu_post(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Img,
    glow: Option<&Plan>,
    blur: Option<&BlurPlan>,
) -> Vec<[u8; 4]> {
    let size = wgpu::Extent3d {
        width: W as u32,
        height: H as u32,
        depth_or_array_layers: 1,
    };
    let bytes: Vec<u8> = scene
        .px
        .iter()
        .flat_map(|c| c.map(|v| (v * 255.0).round() as u8))
        .collect();
    let target = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &bytes,
    );
    let sky_px: Vec<u8> = (0..6)
        .flat_map(|_| {
            [
                (SKY_RGB[0] * 255.0).round() as u8,
                (SKY_RGB[1] * 255.0).round() as u8,
                (SKY_RGB[2] * 255.0).round() as u8,
                255,
            ]
        })
        .collect();
    let cube = device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &sky_px,
        )
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
    let mut post = Cod2Glow::new(device, TARGET_FORMAT, W as u32, H as u32);
    let vp = crate::camera::view_proj_from(Vec3::ZERO, 0.0, 0.0, 0.0, 60.0, W as f32 / H as f32);
    let mut enc = device.create_command_encoder(&Default::default());
    post.encode(
        device,
        queue,
        &mut enc,
        &target.create_view(&Default::default()),
        Some(&cube),
        vp.inverse(),
        Vec3::ZERO,
        glow,
        blur,
    );
    let row = (W as u32 * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * H as u32),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        size,
    );
    queue.submit([enc.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let got = readback.slice(..).get_mapped_range().unwrap().to_vec();
    (0..W * H)
        .map(|i| {
            let at = (i / W) * row as usize + (i % W) * 4;
            [got[at], got[at + 1], got[at + 2], got[at + 3]]
        })
        .collect()
}

/// The largest colour error of `got` against `want` in byte steps, where,
/// and a histogram of the errors by whole steps (the last bin 7 and up).
fn compare(got: &[[u8; 4]], want: &Img) -> (f32, String, [usize; 8]) {
    let (mut max_err, mut worst, mut hist) = (0.0f32, String::new(), [0usize; 8]);
    for (i, g) in got.iter().enumerate() {
        let w = want.px[i];
        for k in 0..3 {
            let err = (f32::from(g[k]) - w[k] * 255.0).abs();
            hist[(err as usize).min(7)] += 1;
            if err > max_err {
                max_err = err;
                worst = format!(
                    "({}, {}) ch {k}: got {g:?}, want {:?}",
                    i % W,
                    i / W,
                    w.map(|v| v * 255.0)
                );
            }
        }
    }
    (max_err, worst, hist)
}

/// The glow's default plan on the test scene, compared texel by texel
/// after both applies, the CPU side storing every target in bytes as the
/// GPU does.
#[test]
fn glow_chain_matches_the_hlsl() {
    let Some((device, queue)) = headless() else {
        eprintln!("no adapter; skipped");
        return;
    };
    let scene = test_scene();
    // retail's defaults, with a radius large enough at this size to take
    // the 2D pass and two 1D ones
    let s = GlowSettings {
        radius: [5.0, 14.0],
        ..GlowSettings::default()
    };
    let p = plan(&s, W as u32, H as u32, true).unwrap();
    assert_eq!(p.chains[0].len(), 1);
    assert!(p.chains[1].len() >= 2, "{:?}", p.chains[1].len());
    let want = cpu_glow(&scene, &p);
    let got = gpu_post(&device, &queue, &scene, Some(&p), None);
    let (max_err, worst, hist) = compare(&got, &want);
    let glowed = (0..W * H)
        .filter(|&i| (want.px[i][0] - scene.px[i][0]).abs() > 4.0 / 255.0)
        .count();
    eprintln!(
        "max error {max_err} of 255 at {worst}, {glowed} of {} texels glowed",
        W * H
    );
    eprintln!("error histogram {hist:?}");
    // the 8-bit targets round each of the four passes, and both applies
    // read the one quarter target, so a step there shows twice
    assert!(max_err <= 4.0, "{worst}");
    assert!(
        hist[3..].iter().sum::<usize>() * 100 < W * H * 3,
        "{hist:?}"
    );
    // the glow reaches well past the bright blocks and the sky bleeds
    assert!(glowed > W * H / 4, "{glowed}");
}

/// CoD4's setup and chain on the test scene (mp_bog's set through
/// mp_crash's film, its radius raised to take two 1D passes at this
/// size), drawn back with the bloom apply at 1 so the quarter target can
/// be read: the post pass adds it the same way (`crate::post`).
#[test]
fn cod4_glow_matches_the_hlsl() {
    let Some((device, queue)) = headless() else {
        eprintln!("no adapter; skipped");
        return;
    };
    let scene = test_scene();
    let c = vcod_cod4::look::Film {
        enable: true,
        contrast: 1.4,
        brightness: 0.134,
        desaturation: 0.265,
        invert: false,
        light_tint: [1.179, 1.086, 1.024],
        dark_tint: [0.7, 1.0215, 1.265],
    }
    .consts()
    .unwrap();
    let film = crate::post::Film {
        bias: c.bias,
        base: c.base,
        delta: c.delta,
    };
    let g = Cod4Glow {
        cutoff: 0.5,
        desaturation: 0.75,
        intensity: 1.42171,
        radius: 200.0,
    };
    let mut p = plan_cod4(&g, film.setup(1.0), W as u32, H as u32).unwrap();
    assert_eq!(p.chains[0].len(), 2, "{:?}", p.chains);
    p.applies.push(Apply {
        sky_bleed: 0.0,
        bloom: 1.0,
    });
    let want = cpu_glow(&scene, &p);
    let got = gpu_post(&device, &queue, &scene, Some(&p), None);
    let (max_err, worst, hist) = compare(&got, &want);
    let glowed = (0..W * H)
        .filter(|&i| (want.px[i][0] - scene.px[i][0]).abs() > 4.0 / 255.0)
        .count();
    eprintln!("cod4: max error {max_err} of 255 at {worst}, {glowed} glowed, {hist:?}");
    assert!(max_err <= 3.0, "{worst}");
    assert!(glowed > W * H / 4, "{glowed}");
}

/// The menus' `blurWorld 5.0` at 480 high: 5 pixels, the downsample's
/// 1.39 off, the rest a quarter as wide in one 2D pass at full alpha.
/// Under three pixels the radius stays at three and the alpha ramps.
#[test]
fn blur_plan_scales_and_ramps() {
    assert!(blur_plan(0.0, 640, 480).is_none());
    let b = blur_plan(5.0, 640, 480).unwrap();
    assert_eq!(b.alpha, 1.0);
    assert_eq!(b.chain.len(), 2);
    let rest = (25.0f32 - MAX_2D_RADIUS_SQ).sqrt() * 0.25;
    assert!(rest <= MAX_2D_RADIUS);
    assert_eq!(b.chain[1], filter_2d(rest, (160, 120), (160, 120)));
    assert_eq!(b.chain[0], filter_2d(MAX_2D_RADIUS, (640, 480), (160, 120)));
    // 1080 high: 11.25 pixels, then a vertical and a horizontal pass
    let b = blur_plan(5.0, 1920, 1080).unwrap();
    assert_eq!(b.chain.len(), 3);
    assert!(b.chain[1].taps.iter().all(|t| t[0] == 0.0));
    assert!(b.chain[2].taps.iter().all(|t| t[1] == 0.0));
    // 1.5 virtual units at 480 high: half of the three pixels, alpha 128
    let b = blur_plan(1.5, 640, 480).unwrap();
    assert_eq!(b.alpha, 128.0 / 255.0);
    assert_eq!(
        b,
        BlurPlan {
            alpha: b.alpha,
            ..blur_plan(3.0, 640, 480).unwrap()
        }
    );
}

/// The blur alone at full alpha, then the glow and a ramped blur together:
/// the blur reads the scene from before the glow and fades over it.
#[test]
fn blur_matches_the_hlsl() {
    let Some((device, queue)) = headless() else {
        eprintln!("no adapter; skipped");
        return;
    };
    let scene = test_scene();
    // 10.7 pixels: the downsample and two 1D passes
    let wide = blur_plan(80.0, W as u32, H as u32).unwrap();
    assert_eq!(wide.alpha, 1.0);
    assert_eq!(wide.chain.len(), 3);
    let want = cpu_blur(&scene, &scene, &wide);
    let got = gpu_post(&device, &queue, &scene, None, Some(&wide));
    let (max_err, worst, hist) = compare(&got, &want);
    eprintln!("blur: max error {max_err} of 255 at {worst}, histogram {hist:?}");
    assert!(max_err <= 2.0, "{worst}");
    // the blur moved the blocks' edges
    let moved = (0..W * H)
        .filter(|&i| (want.px[i][0] - scene.px[i][0]).abs() > 8.0 / 255.0)
        .count();
    assert!(moved > W * H / 4, "{moved}");

    // under three pixels: the downsample and a 2D pass, alpha 57
    let ramp = blur_plan(5.0, W as u32, H as u32).unwrap();
    assert_eq!(ramp.alpha, 57.0 / 255.0);
    assert_eq!(ramp.chain.len(), 2);
    let p = plan(&GlowSettings::default(), W as u32, H as u32, true).unwrap();
    let glowed = cpu_glow(&scene, &p);
    let want = cpu_blur(&scene, &glowed, &ramp);
    let got = gpu_post(&device, &queue, &scene, Some(&p), Some(&ramp));
    let (max_err, worst, hist) = compare(&got, &want);
    eprintln!("glow + blur: max error {max_err} of 255 at {worst}, histogram {hist:?}");
    assert!(max_err <= 4.0, "{worst}");
    assert!(
        hist[3..].iter().sum::<usize>() * 100 < W * H * 3,
        "{hist:?}"
    );
}

fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let info = adapter.get_info();
    eprintln!("adapter: {} {:?}", info.name, info.backend);
    if info.backend == wgpu::Backend::Gl {
        return None;
    }
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}
