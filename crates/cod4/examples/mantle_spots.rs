//! Finds a map's mantle spots for a capture: every vertical axial face of a
//! `CONTENTS_MANTLE` brush whose side is `MANTLE_ON` or `MANTLE_OVER`, a
//! body set down 24 units in front of it facing it, then run forward and
//! jump through `vcod_cod4::pmove` (docs/research/cod4-pmove.md 17).
//! Prints each spot the mantle starts from, with its rise, its row and
//! where it ends.
//!
//!     COD4_DIR=... cargo run -p vcod-cod4 --example mantle_spots [mp_crash]

use glam::Vec3;
use vcod_cod4::assets::Assets;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::pmove::mantle::{self, CONTENTS_MANTLE, SURF_MANTLE_ON, SURF_MANTLE_OVER};
use vcod_cod4::pmove::{self, Cmd, PLAYER_MAXS, PLAYER_MINS, PlayerState, Rules, WeaponMove, pmf};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::walk;
use vcod_cod4::zone::Zone;

fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or_else(|| "mp_crash".into());
    let install = cod4_install().ok_or_else(|| anyhow::anyhow!("set COD4_DIR"))?;
    let z = Zone::open_path(&install.zone("english", "common_mp"))?;
    let common: Assets = walk(z, Default::default())?.assets;
    mantle::install(&[&common]).ok_or_else(|| anyhow::anyhow!("no mantle clips"))?;
    let (_, walked) = Map::load(&install, "english", &map)?;
    let world = ClipMap::from_assets(&walked.assets)?;
    let rules = Rules::default();
    let surf = |m: u16| world.world.materials.get(usize::from(m)).map_or(0, |s| s.0);
    for b in world.model_brushes(0) {
        let br = &world.world.brushes[usize::from(b)];
        if br.contents & CONTENTS_MANTLE == 0 {
            continue;
        }
        for axis in 0..2 {
            for is_max in 0..2 {
                let s = surf(br.axial_material[is_max][axis]);
                if s & (SURF_MANTLE_ON | SURF_MANTLE_OVER) == 0 {
                    continue;
                }
                let out = if is_max == 1 { 1.0 } else { -1.0 };
                let mut p = (br.mins + br.maxs) * 0.5;
                p[axis] = if is_max == 1 {
                    br.maxs[axis]
                } else {
                    br.mins[axis]
                } + out * 24.0;
                // The floor under the spot.
                let top = Vec3::new(p.x, p.y, br.maxs.z + 8.0);
                let low = Vec3::new(p.x, p.y, br.mins.z - 128.0);
                let t = world.trace(top, low, PLAYER_MINS, PLAYER_MAXS, pmove::MASK_PLAYERSOLID);
                if t.startsolid || t.fraction >= 1.0 {
                    continue;
                }
                let floor = top.z + (low.z - top.z) * t.fraction + 0.125;
                let mut face = Vec3::ZERO;
                face[axis] = -out;
                let yaw = face.y.atan2(face.x).to_degrees();
                if let Some(end) = try_spot(&world, rules, Vec3::new(p.x, p.y, floor), yaw) {
                    println!(
                        "brush {b} {} side {}{}: from ({:.1} {:.1} {:.2}) yaw {yaw:.0}, top {:.1} (rise {:.1}), {end}",
                        if s & SURF_MANTLE_OVER != 0 {
                            "over"
                        } else {
                            "on"
                        },
                        if is_max == 1 { "+" } else { "-" },
                        ["x", "y"][axis],
                        p.x,
                        p.y,
                        floor,
                        br.maxs.z,
                        br.maxs.z - floor,
                    );
                }
            }
        }
    }
    Ok(())
}

/// Walks into the wall from `at` and jumps on the fifth cmd; returns how the
/// mantle went if one started.
fn try_spot(world: &ClipMap, rules: Rules, at: Vec3, yaw: f32) -> Option<String> {
    let mut ps = PlayerState::spawn(at, 0.0, 1000);
    ps.move_speed_scale_multiplier = 1.0;
    let mut old = Cmd::default();
    let short = ((f64::from(yaw) * 65536.0 / 360.0).round() as i64 & 0xffff) as u16;
    let mut started = None;
    for n in 1..60 {
        let cmd = Cmd {
            server_time: 1000 + 50 * n,
            buttons: if n >= 5 { pmove::button::JUMP } else { 0 },
            angles: [0, short, 0],
            forward_move: 127,
            ..Default::default()
        };
        pmove::player_step(&mut ps, &cmd, &mut old, world, rules, WeaponMove::NONE);
        if ps.pm_flags & pmf::MANTLE != 0 && started.is_none() {
            started = Some((n, ps.mantle));
        }
        if started.is_some() && ps.pm_flags & pmf::MANTLE == 0 {
            let (n0, st) = started?;
            return Some(format!(
                "row {} flags {:x}, {} ms, ends ({:.1} {:.1} {:.2})",
                st.trans_index,
                st.flags,
                (n - n0) * 50,
                ps.origin.x,
                ps.origin.y,
                ps.origin.z
            ));
        }
    }
    started.map(|_| "unfinished".to_string())
}
