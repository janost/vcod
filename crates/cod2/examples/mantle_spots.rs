//! Finds mantle spots on a CoD2 map for `move_probe`'s mantle scripts: every
//! axial face of a world brush whose material is flagged `MANTLE_ON` or
//! `MANTLE_OVER`, a body set down 24 out from its middle facing it, and
//! `move_probe --script mantle`'s cmds run through vcod's pmove. Prints each
//! spot that mantles: the `run_probe_cod2.sh` arguments, the row, the flags
//! and the surface. Needs `COD2_DIR`.
//!
//!   cargo run -p vcod-cod2 --example mantle_spots -- mp_toujane

use glam::Vec3;
use vcod_cod2::collision::{SURF_MANTLE_ON, SURF_MANTLE_OVER};
use vcod_cod2::net::UserCmd2;
use vcod_cod2::pmove::{self, PlayerState, Rules, pmf};
use vcod_common::movetrace::MoveWorld;
use vcod_common::patch::Arith;

fn main() -> anyhow::Result<()> {
    let map = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mp_toujane".into());
    let fs = vcod_common::testing::cod2_fs().ok_or_else(|| anyhow::anyhow!("set COD2_DIR"))?;
    let data = fs
        .read(&format!("maps/mp/{map}.d3dbsp"))
        .ok_or_else(|| anyhow::anyhow!("no {map}"))?;
    let b = vcod_cod2::d3dbsp::D3dBsp::parse(&data)?;
    let w = vcod_cod2::world::from_bsp(&b);
    let cw = vcod_cod2::map::collision_world(&b, &w, &[])?;
    cw.set_arith(Arith::Stored);
    let rules = Rules {
        mantle_anims: pmove::mantle::anims(&fs),
        ..Rules::default()
    };
    let m0 = &w.models[0];
    let brushes = m0.first_brush as usize..(m0.first_brush + m0.num_brushes) as usize;
    for bi in brushes {
        let br = &w.brushes[bi];
        let sides = &w.brush_sides[br.first_side as usize..][..br.num_sides as usize];
        let bound = |k: usize| f32::from_bits(sides[k].plane_or_dist);
        let (lo, hi) = (
            Vec3::new(bound(0), bound(2), bound(4)),
            Vec3::new(bound(1), bound(3), bound(5)),
        );
        for (k, side) in sides.iter().enumerate().take(4) {
            let surf = w.materials[side.material as usize].surface_flags;
            if surf & (SURF_MANTLE_ON | SURF_MANTLE_OVER) == 0 {
                continue;
            }
            let axis = k / 2;
            let out = if k % 2 == 0 { -1.0 } else { 1.0 };
            let mut p = (lo + hi) * 0.5;
            p[axis] = if k % 2 == 0 { lo[axis] } else { hi[axis] } + out * 24.0;
            // The floor in front: a body dropped from the brush's foot.
            p.z = lo.z + 4.0;
            let t = MoveWorld::bare(&cw).box_trace(
                p,
                p - Vec3::Z * 256.0,
                pmove::PLAYER_MINS,
                pmove::PLAYER_MAXS,
                pmove::MASK_PLAYERSOLID,
            );
            if t.startsolid || t.fraction == 1.0 {
                continue;
            }
            let ground = p.z - 256.0 * t.fraction;
            let mut face = Vec3::ZERO;
            face[axis] = -out;
            let yaw = face.y.atan2(face.x).to_degrees().round();
            let spawn = Vec3::new(p.x.round(), p.y.round(), (ground + 4.0).round());
            if let Some((row, flags, rise)) = run(&cw, rules, spawn, yaw) {
                println!(
                    "{map} {} {} {} {yaw}  row {row} flags {flags:#x} rise {rise:.2} {}",
                    spawn.x,
                    spawn.y,
                    spawn.z,
                    if surf & SURF_MANTLE_OVER != 0 {
                        "over"
                    } else {
                        "on"
                    }
                );
            }
        }
    }
    Ok(())
}

/// `move_probe --script mantle` at 12 ms a cmd: a second still, 700 ms
/// forward, 100 ms jumping forward, 1500 ms forward. Returns the mantle's
/// row, flags and rise.
fn run(
    cw: &vcod_common::collision::CollisionWorld,
    rules: Rules,
    spawn: Vec3,
    yaw: f32,
) -> Option<(i32, u32, f32)> {
    let mut ps = PlayerState::spawn(spawn, yaw, 0);
    let mut old = UserCmd2::default();
    let yaw_short = ((yaw * 65536.0 / 360.0) as i32) & 0xffff;
    let mut t = 0;
    for (ms, buttons, fwd) in [
        (1000, 0, 0),
        (700, 0, 127),
        (100, 0x400, 127),
        (1500, 0, 127),
    ] {
        for _ in 0..ms / 12 {
            t += 12;
            let cmd = UserCmd2 {
                server_time: t,
                buttons,
                weapon: 0,
                angles: [0, yaw_short, 0],
                forward_move: fwd,
                ..Default::default()
            };
            let z = ps.origin.z;
            pmove::player_step(&mut ps, &cmd, &mut old, MoveWorld::bare(cw), rules, 1.0);
            if ps.pm_flags & pmf::MANTLE != 0 {
                return Some((ps.mantle.trans_index, ps.mantle.flags, ps.origin.z - z));
            }
        }
    }
    None
}
