//! The head icon a script sets with `.headicon` / `.headiconteam`, drawn as
//! a camera-facing sprite over the player. Retail's player sprite pass is
//! cgame 0x300274d0; docs/research/cod11-gametypes-re-bel.md, section 7.3.

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;
use vcod_common::net::flags::{EF_NODRAW, ET_PLAYER};
use vcod_common::net::msg::{ClientState, EntityState};
use vcod_common::net::protocol::{CsRange, Protocol};

use crate::fx::sim::FxQuad;

/// `iHeadIcon` n names configstring `HEAD_ICON_BASE + n`.
const HEAD_ICON_BASE: usize = CsRange::HeadIcon.bounds().0 - 1;
/// Above the `Bip01 Head` bone.
const ABOVE_HEAD: f32 = 18.0;
/// Above the origin, for a body with no head bone.
const ABOVE_ORIGIN: f32 = 72.0;
/// Sprite half-extent.
const RADIUS: f32 = 6.66;
const TEAM_SPECTATOR: i32 = 3;
/// eFlags bit 1, which the player draw (0x30028210) skips with `EF_NODRAW`.
const EF_SKIP_DRAW: i32 = 1;

/// Whether a viewer on `viewer_team` sees an icon limited to `icon_team`
/// (0 is everyone). A spectator sees every icon.
pub fn visible(icon: i32, icon_team: i32, viewer_team: i32) -> bool {
    icon != 0 && (icon_team == 0 || viewer_team == TEAM_SPECTATOR || icon_team == viewer_team)
}

/// The sprite's centre: over the head bone when the body has one.
pub fn anchor(head: Option<Vec3>, origin: Vec3) -> Vec3 {
    match head {
        Some(h) => h + Vec3::Z * ABOVE_HEAD,
        None => origin + Vec3::Z * ABOVE_ORIGIN,
    }
}

/// A `RADIUS` sprite at `centre`, texture top-left at the view's top-left.
pub fn sprite(centre: Vec3, right: Vec3, up: Vec3, shader: &str) -> FxQuad {
    let (r, u) = (right * RADIUS, up * RADIUS);
    let corners = [
        centre - r + u,
        centre + r + u,
        centre + r - u,
        centre - r - u,
    ];
    FxQuad {
        verts: corners.map(|c| c.to_array()),
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        rgba: [1.0; 4],
        shader: shader.to_string(),
    }
}

/// Everything a frame needs to place head icons.
pub struct Scene<'a> {
    pub protocol: &'a Protocol,
    pub entities: &'a BTreeMap<u32, EntityState>,
    pub clients: &'a BTreeMap<u32, ClientState>,
    pub configstrings: &'a [String],
    /// `ps.clientNum`: whose team filters the icons.
    pub viewer: i32,
    /// Drawn players' head bones and every drawn entity's origin, from
    /// `entities::BuiltScene`. An entity in neither was not drawn.
    pub heads: &'a HashMap<u32, Vec3>,
    pub entity_pos: &'a HashMap<u32, Vec3>,
}

/// One sprite per drawn player whose icon the viewer may see. Both the
/// player and the viewer need a roster entry, as retail's `infoValid`.
pub fn quads(s: &Scene, right: Vec3, up: Vec3) -> Vec<FxQuad> {
    let p = s.protocol;
    let Some(viewer_team) = u32::try_from(s.viewer)
        .ok()
        .and_then(|n| s.clients.get(&n))
        .map(|c| c.field_i32(p, "team"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (&num, ent) in s.entities {
        let int = |name: &str| ent.field_i32(p, name);
        if int("eType") != ET_PLAYER
            || int("eFlags") & (EF_NODRAW | EF_SKIP_DRAW) != 0
            || !visible(int("iHeadIcon"), int("iHeadIconTeam"), viewer_team)
        {
            continue;
        }
        let known = u32::try_from(int("clientNum")).is_ok_and(|c| s.clients.contains_key(&c));
        let Some(&origin) = s.entity_pos.get(&num).filter(|_| known) else {
            continue;
        };
        let cs = HEAD_ICON_BASE + int("iHeadIcon") as usize;
        let Some(shader) = s.configstrings.get(cs).filter(|n| !n.is_empty()) else {
            continue;
        };
        let centre = anchor(s.heads.get(&num).copied(), origin);
        out.push(sprite(centre, right, up, shader));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::PROTOCOL_V1;

    #[test]
    fn team_filter_follows_the_sprite_pass() {
        assert!(!visible(0, 0, 1), "no icon");
        assert!(visible(1, 0, 1), "everyone");
        assert!(visible(1, 2, 2), "own team");
        assert!(!visible(1, 2, 1), "other team");
        assert!(visible(1, 1, TEAM_SPECTATOR), "spectator sees all");
    }

    #[test]
    fn anchor_prefers_the_head_bone() {
        let origin = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(anchor(None, origin), Vec3::new(1.0, 2.0, 75.0));
        assert_eq!(
            anchor(Some(Vec3::new(5.0, 6.0, 60.0)), origin),
            Vec3::new(5.0, 6.0, 78.0)
        );
    }

    #[test]
    fn sprite_is_upright_and_radius_wide() {
        let q = sprite(Vec3::ZERO, Vec3::Y, Vec3::Z, "icon");
        assert_eq!(q.verts[0], [0.0, -RADIUS, RADIUS]); // top-left
        assert_eq!(q.uvs[0], [0.0, 0.0]);
        assert_eq!(q.verts[2], [0.0, RADIUS, -RADIUS]);
        assert_eq!(q.uvs[2], [1.0, 1.0]);
    }

    #[test]
    fn quads_resolve_the_configstring_and_skip_hidden_players() {
        let p = &PROTOCOL_V1;
        let player = |client: i32, icon: i32, team: i32, eflags: i32| {
            let mut e = EntityState::null(p);
            let mut set = |n: &str, v: i32| {
                e.fields[EntityState::field_index(p, n).unwrap()] = v;
            };
            set("eType", ET_PLAYER);
            set("clientNum", client);
            set("iHeadIcon", icon);
            set("iHeadIconTeam", team);
            set("eFlags", eflags);
            e
        };
        let entities = BTreeMap::from([
            (1, player(1, 1, 0, 0)),
            (2, player(2, 1, 1, 0)),         // axis only; viewer is allies
            (3, player(3, 1, 0, EF_NODRAW)), // not drawn
            (4, player(9, 1, 0, 0)),         // no roster entry
        ]);
        let clients: BTreeMap<u32, ClientState> = [0, 1, 2, 3]
            .into_iter()
            .map(|n| (n, ClientState::named(p, n, 2, "x")))
            .collect();
        let mut configstrings = vec![String::new(); 64];
        configstrings[HEAD_ICON_BASE + 1] = "gfx/hud/headicon@re_objcarrier".into();
        let entity_pos: HashMap<u32, Vec3> = (1..=4).map(|n| (n, Vec3::ZERO)).collect();
        let heads = HashMap::from([(1, Vec3::new(0.0, 0.0, 60.0))]);
        let scene = Scene {
            protocol: p,
            entities: &entities,
            clients: &clients,
            configstrings: &configstrings,
            viewer: 0,
            heads: &heads,
            entity_pos: &entity_pos,
        };
        let q = quads(&scene, Vec3::Y, Vec3::Z);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].shader, "gfx/hud/headicon@re_objcarrier");
        assert_eq!(q[0].verts[0][2], 78.0 + RADIUS);
    }
}
