//! `cg_shadows 1`: the `markShadow` blob under each drawn player,
//! `CG_PlayerShadow` (cgame 0x30027630). A temporary mark placed every
//! frame, never pooled. Facts and addresses:
//! docs/research/cod11-events-and-fx.md, section 9.7.

use glam::Vec3;
use vcod_common::collision::CollisionWorld;

use crate::marks::MarkRequest;

/// `CG_RegisterGraphics` (0x30020da0) registers it by this name.
pub const SHADOW_SHADER: &str = "markShadow";
/// The trace's mask (0x3002765f).
const SHADOW_MASK: u32 = 0x2810011;
/// How far below the origin the trace looks (0x300695a0).
const SHADOW_DEPTH: f32 = 64.0;
const SHADOW_RADIUS: f32 = 16.0;

/// The mark under a player standing at `origin` (its drawn `lerpOrigin`)
/// seen from `viewer` (`cg.snap->ps.origin`), or none: nothing within 64
/// units below, a start in solid, or a viewer 250 units away or closer or
/// over 1024. Its grey level fades in from 250 to 512 units and out again
/// to 1024, times the height left under the trace. The trace is trap 0x26,
/// `CM_BoxTrace` on model 0 (0x3002768f): the world alone, so a blob falls
/// through a door, a mover or a vehicle to the ground under it.
pub fn player_shadow(world: &CollisionWorld, viewer: Vec3, origin: Vec3) -> Option<MarkRequest> {
    let tr = world.world_point_trace(origin, origin - Vec3::Z * SHADOW_DEPTH, SHADOW_MASK);
    if tr.fraction == 1.0 || tr.fraction == 0.0 {
        return None;
    }
    let grey = distance_fade(viewer.distance(origin))? * (1.0 - tr.fraction);
    Some(MarkRequest {
        shader: SHADOW_SHADER.to_string(),
        origin: tr.endpos,
        dir: tr.normal,
        orientation: 0.0,
        rgba: [grey, grey, grey, 1.0],
        alpha_fade: false,
        radius: SHADOW_RADIUS,
        life_ms: -1,
    })
}

/// 0x30027701..0x3002775d: none at 250 or nearer, a ramp up over 250..512
/// (`1 / 262`, 0x30069710), down over 512..1024 (`1 / 512`, 0x30069700),
/// none past 1024.
fn distance_fade(dist: f32) -> Option<f32> {
    let dist = f64::from(dist);
    let fade = if dist <= 250.0 || dist > 1024.0 {
        return None;
    } else if dist < 512.0 {
        (dist - 250.0) * (1.0 / 262.0)
    } else {
        1.0 - (dist - 512.0) * (1.0 / 512.0)
    };
    Some(fade as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_fade_matches_the_cgame_ramps() {
        assert_eq!(distance_fade(0.0), None);
        assert_eq!(distance_fade(250.0), None);
        assert!((distance_fade(381.0).unwrap() - 0.5).abs() < 1e-6);
        assert!((distance_fade(511.999).unwrap() - 1.0).abs() < 1e-4);
        assert_eq!(distance_fade(512.0), Some(1.0));
        assert!((distance_fade(768.0).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(distance_fade(1024.0), Some(0.0));
        assert_eq!(distance_fade(1024.5), None);
    }

    /// A player standing on a submodel (a mover's roof 48 up) casts his
    /// blob through it onto the world's floor, or none when the floor is
    /// past 64 units; the world's own brush still takes it.
    #[test]
    fn the_shadow_trace_passes_submodels_to_the_world_under_them() {
        let world = vcod_common::collision::submodel_test_world(
            "{\n\"classname\" \"script_brushmodel\"\n\"model\" \"*1\"\n}",
            &[([-32.0, -32.0, 0.0], [32.0, 32.0, 48.0])],
        );
        let viewer = Vec3::new(512.0, 0.0, 48.0);
        // the full trace meets the roof; the shadow's does not
        let roof = world.point_trace(Vec3::Z * 56.0, Vec3::Z * -8.0, SHADOW_MASK, false);
        assert!((roof.endpos.z - 48.0).abs() < 0.2, "{:?}", roof.endpos);
        let on_roof = player_shadow(&world, viewer, Vec3::Z * 49.0);
        let blob = on_roof.expect("the floor is 48 under the roof");
        assert!(blob.origin.z.abs() < 0.2, "{:?}", blob.origin);
        assert!(player_shadow(&world, viewer, Vec3::Z * 70.0).is_none());
        let feet = Vec3::new(200.0, 0.0, 1.0);
        let floor = player_shadow(&world, feet + Vec3::X * 512.0, feet).unwrap();
        assert!(
            floor.origin.z.abs() < 0.2 && floor.rgba[0] > 0.95,
            "{floor:?}"
        );
    }

    #[test]
    fn a_player_on_carentan_ground_casts_a_blob_that_fades_with_height() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let Some(raw) = fs.read("maps/mp/mp_carentan.bsp") else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&raw).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let (spawn, _) = vcod_common::bsp::find_spawn(&bsp.entities).unwrap();
        let spawn = Vec3::from_array(spawn);
        let down = world.point_trace(spawn, spawn - Vec3::Z * 256.0, SHADOW_MASK, false);
        assert!(down.fraction < 1.0);
        let feet = down.endpos + Vec3::Z * 0.125;
        let viewer = feet + Vec3::new(512.0, 0.0, 0.0);
        let on_ground = player_shadow(&world, viewer, feet).unwrap();
        assert_eq!(on_ground.shader, SHADOW_SHADER);
        assert_eq!(on_ground.radius, 16.0);
        assert!((on_ground.origin.z - down.endpos.z).abs() < 0.5);
        assert!(on_ground.dir.z > 0.7, "{:?}", on_ground.dir);
        assert!(on_ground.rgba[0] > 0.95);
        let jumping = player_shadow(&world, viewer, feet + Vec3::Z * 32.0).unwrap();
        assert!((jumping.rgba[0] - 0.5).abs() < 0.02, "{:?}", jumping.rgba);
        assert!(player_shadow(&world, viewer, feet + Vec3::Z * 70.0).is_none());
        assert!(player_shadow(&world, feet, feet).is_none());
    }
}
