// xmodel_ray <model> x0 y0 z0 x1 y1 z1: every collision triangle the segment crosses, at bind.
use glam::Vec3;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let fs = vcod_common::testing::uo_fs().expect("uo");
    let m = vcod_common::model::load(&fs, &a[1]).unwrap();
    let f: Vec<f32> = a[2..8].iter().map(|s| s.parse().unwrap()).collect();
    let (p, q) = (Vec3::new(f[0], f[1], f[2]), Vec3::new(f[3], f[4], f[5]));
    let d = q - p;
    for (si, s) in m.collision.iter().enumerate() {
        for (ti, t) in s.tris.iter().enumerate() {
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            let den = n.dot(d);
            if den.abs() < 1e-9 {
                continue;
            }
            let u = n.dot(t[0] - p) / den;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let x = p + d * u;
            let inside = (0..3).all(|i| (t[(i + 1) % 3] - t[i]).cross(x - t[i]).dot(n) >= 0.0);
            if inside {
                println!(
                    "surf {si} bone {} tri {ti} frac {u:.4} at {x:?} facing {}",
                    s.bone,
                    if den < 0.0 { "toward" } else { "away" }
                );
            }
        }
    }
}
