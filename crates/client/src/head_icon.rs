//! The sprites over a player's head: the icon a script sets with
//! `.headicon` / `.headiconteam`, then one of the killcam marker, the
//! disconnected icon and the talk balloon. Retail's player sprite pass is
//! cgame 0x300274d0; docs/research/cod11-gametypes-re-bel.md, section 7.3.

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;
use vcod_common::net::flags::{EF_NODRAW, EF_TALK, ET_PLAYER};
use vcod_common::net::msg::{ClientState, EntityState};
use vcod_common::net::protocol::{CsRange, Protocol};

use crate::fx::sim::FxQuad;

/// Above the `Bip01 Head` bone.
const ABOVE_HEAD: f32 = 18.0;
/// Above the origin, for a body with no head bone.
const ABOVE_ORIGIN: f32 = 72.0;
/// Sprite half-extent.
const RADIUS: f32 = 6.66;
const TEAM_SPECTATOR: i32 = 3;
/// How far the second sprite sits over a drawn head icon (0x30027565).
const STACK: f32 = 16.0;
/// The talk balloon hangs this much lower than the other second sprites
/// (0x30069414).
const TALK_DROP: f32 = 5.0;
/// `eFlags` 0x1000: `ClientEndFrame` sets it on a client whose last cmd is
/// over 1000 ms old (game.mp 0x50e98).
const EF_DISCONNECTED: i32 = 0x1000;
const TALK_SHADER: &str = "headiconTalkBalloon";
const DISCONNECTED_SHADER: &str = "headiconDisconnected";
const KILLCAM_SHADER: &str = "headiconYouInKillCam";
/// eFlags bit 1, which the player draw (0x30028210) skips with `EF_NODRAW`.
const EF_SKIP_DRAW: i32 = 1;

/// Whether a viewer on `viewer_team` sees an icon limited to `icon_team`
/// (0 is everyone). A spectator sees every icon.
pub fn visible(icon: i32, icon_team: i32, viewer_team: i32) -> bool {
    icon != 0 && (icon_team == 0 || viewer_team == TEAM_SPECTATOR || icon_team == viewer_team)
}

/// The second sprite after a head icon, if any (0x3002756d..0x30027629).
#[derive(Debug, PartialEq)]
enum Second {
    /// Our own body while the view is someone else's.
    Killcam,
    Disconnected,
    Talk,
}

/// Which second sprite `num` carries. `own` is our client number, `viewer`
/// the viewed one (`ps.clientNum`); the talk balloon shows only to a
/// teammate or a spectator. Retail's voice chat icon would take the talk
/// balloon's place; vcod draws none.
fn second(
    num: u32,
    eflags: i32,
    team: i32,
    own: i32,
    viewer: i32,
    viewer_team: i32,
) -> Option<Second> {
    if num as i32 == own && viewer != own {
        Some(Second::Killcam)
    } else if eflags & EF_DISCONNECTED != 0 {
        Some(Second::Disconnected)
    } else if (team == viewer_team || viewer_team == TEAM_SPECTATOR) && eflags & EF_TALK != 0 {
        Some(Second::Talk)
    } else {
        None
    }
}

/// The sprite's centre `height` over the head bone when the body has one.
pub fn anchor(head: Option<Vec3>, origin: Vec3, height: f32) -> Vec3 {
    match head {
        Some(h) => h + Vec3::Z * (height + ABOVE_HEAD),
        None => origin + Vec3::Z * (height + ABOVE_ORIGIN),
    }
}

/// The killcam marker's distance scale and the raised centre that goes
/// with it (0x300273d2..0x3002743b).
fn distance_scaled(centre: Vec3, eye: Vec3) -> (Vec3, f32) {
    let scale = (centre.distance(eye) * (1.0 / 256.0) + 0.2).max(0.6);
    (centre + Vec3::Z * (8.0 * scale - 8.0), RADIUS * scale)
}

/// A `RADIUS` sprite at `centre`, texture top-left at the view's top-left.
pub fn sprite(centre: Vec3, right: Vec3, up: Vec3, shader: &str) -> FxQuad {
    sized_sprite(centre, RADIUS, right, up, shader)
}

fn sized_sprite(centre: Vec3, radius: f32, right: Vec3, up: Vec3, shader: &str) -> FxQuad {
    let (r, u) = (right * radius, up * radius);
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
        shader: shader.into(),
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
    /// Our own client number (`cg.clientNum`).
    pub own: i32,
    /// The view origin, which scales the killcam marker.
    pub eye: Vec3,
    /// Drawn players' head bones and every drawn entity's origin, from
    /// `entities::BuiltScene`. An entity in neither was not drawn.
    pub heads: &'a HashMap<u32, Vec3>,
    pub entity_pos: &'a HashMap<u32, Vec3>,
}

/// Up to two sprites per drawn player: the head icon the viewer may see,
/// then the second sprite 16 units over it (over the head bone when no
/// icon drew). Both the player and the viewer need a roster entry, as
/// retail's `infoValid`.
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
        if int("eType") != ET_PLAYER || int("eFlags") & (EF_NODRAW | EF_SKIP_DRAW) != 0 {
            continue;
        }
        let client = u32::try_from(int("clientNum"))
            .ok()
            .and_then(|c| s.clients.get(&c));
        let (Some(client), Some(&origin)) = (client, s.entity_pos.get(&num)) else {
            continue;
        };
        let head = s.heads.get(&num).copied();
        let mut height = 0.0;
        // `iHeadIcon` n names the head icon block's slot n.
        let icon = int("iHeadIcon");
        let shader = visible(icon, int("iHeadIconTeam"), viewer_team)
            .then(|| {
                s.configstrings
                    .get(p.cs.base(CsRange::HeadIcon) + icon as usize)
            })
            .flatten()
            .filter(|n| !n.is_empty());
        if let Some(shader) = shader {
            out.push(sprite(anchor(head, origin, 0.0), right, up, shader));
            height = STACK;
        }
        let team = client.field_i32(p, "team");
        match second(num, int("eFlags"), team, s.own, s.viewer, viewer_team) {
            Some(Second::Killcam) => {
                let (centre, radius) = distance_scaled(anchor(head, origin, height), s.eye);
                out.push(sized_sprite(centre, radius, right, up, KILLCAM_SHADER));
            }
            Some(Second::Disconnected) => {
                out.push(sprite(
                    anchor(head, origin, height),
                    right,
                    up,
                    DISCONNECTED_SHADER,
                ));
            }
            Some(Second::Talk) => {
                let centre = anchor(head, origin, height - TALK_DROP);
                out.push(sprite(centre, right, up, TALK_SHADER));
            }
            None => {}
        }
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
        assert_eq!(anchor(None, origin, 0.0), Vec3::new(1.0, 2.0, 75.0));
        assert_eq!(
            anchor(Some(Vec3::new(5.0, 6.0, 60.0)), origin, 0.0),
            Vec3::new(5.0, 6.0, 78.0)
        );
        assert_eq!(anchor(None, origin, 11.0), Vec3::new(1.0, 2.0, 86.0));
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
        configstrings[p.cs.base(CsRange::HeadIcon) + 1] = "gfx/hud/headicon@re_objcarrier".into();
        let entity_pos: HashMap<u32, Vec3> = (1..=4).map(|n| (n, Vec3::ZERO)).collect();
        let heads = HashMap::from([(1, Vec3::new(0.0, 0.0, 60.0))]);
        let scene = Scene {
            protocol: p,
            entities: &entities,
            clients: &clients,
            configstrings: &configstrings,
            viewer: 0,
            own: 0,
            eye: Vec3::ZERO,
            heads: &heads,
            entity_pos: &entity_pos,
        };
        let q = quads(&scene, Vec3::Y, Vec3::Z);
        assert_eq!(q.len(), 1);
        assert_eq!(&*q[0].shader, "gfx/hud/headicon@re_objcarrier");
        assert_eq!(q[0].verts[0][2], 78.0 + RADIUS);
    }

    #[test]
    fn second_sprite_follows_the_pass_order() {
        let talk = EF_TALK;
        // Own body seen while following someone else: the killcam marker
        // wins over everything after it.
        assert_eq!(
            second(3, talk | EF_DISCONNECTED, 1, 3, 5, 1),
            Some(Second::Killcam)
        );
        assert_eq!(second(3, talk, 1, 3, 3, 1), Some(Second::Talk), "own view");
        // Disconnected shows to anyone and beats the balloon.
        assert_eq!(
            second(4, talk | EF_DISCONNECTED, 1, 0, 0, 2),
            Some(Second::Disconnected)
        );
        // The balloon: teammates and spectators only.
        assert_eq!(second(4, talk, 1, 0, 0, 1), Some(Second::Talk));
        assert_eq!(second(4, talk, 1, 0, 0, 2), None);
        assert_eq!(second(4, talk, 1, 0, 0, TEAM_SPECTATOR), Some(Second::Talk));
        assert_eq!(
            second(4, talk, 0, 0, 0, 0),
            Some(Second::Talk),
            "free for all"
        );
        assert_eq!(second(4, 0, 1, 0, 0, 1), None);
    }

    #[test]
    fn killcam_marker_grows_with_distance() {
        let near = distance_scaled(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(near, (Vec3::new(0.0, 0.0, 8.0 * 0.6 - 8.0), RADIUS * 0.6));
        let (centre, radius) = distance_scaled(Vec3::ZERO, Vec3::new(512.0, 0.0, 0.0));
        assert!((radius - RADIUS * 2.2).abs() < 1e-5);
        assert!((centre.z - (8.0 * 2.2 - 8.0)).abs() < 1e-5);
    }

    #[test]
    fn talk_balloon_stacks_over_the_head_icon() {
        let p = &PROTOCOL_V1;
        let player = |client: i32, icon: i32, eflags: i32| {
            let mut e = EntityState::null(p);
            let mut set = |n: &str, v: i32| {
                e.fields[EntityState::field_index(p, n).unwrap()] = v;
            };
            set("eType", ET_PLAYER);
            set("clientNum", client);
            set("iHeadIcon", icon);
            set("eFlags", eflags);
            e
        };
        let entities = BTreeMap::from([(1, player(1, 1, EF_TALK)), (2, player(2, 0, EF_TALK))]);
        let clients: BTreeMap<u32, ClientState> = [0, 1, 2]
            .into_iter()
            .map(|n| (n, ClientState::named(p, n, 2, "x")))
            .collect();
        let mut configstrings = vec![String::new(); 64];
        configstrings[p.cs.base(CsRange::HeadIcon) + 1] = "icon".into();
        let entity_pos: HashMap<u32, Vec3> = (1..=2).map(|n| (n, Vec3::ZERO)).collect();
        let heads: HashMap<u32, Vec3> = (1..=2).map(|n| (n, Vec3::new(0.0, 0.0, 60.0))).collect();
        let scene = Scene {
            protocol: p,
            entities: &entities,
            clients: &clients,
            configstrings: &configstrings,
            viewer: 0,
            own: 0,
            eye: Vec3::ZERO,
            heads: &heads,
            entity_pos: &entity_pos,
        };
        let q = quads(&scene, Vec3::Y, Vec3::Z);
        let centre = |q: &FxQuad| q.verts[0][2] - RADIUS;
        assert_eq!(q.len(), 3);
        assert_eq!((&*q[0].shader, centre(&q[0])), ("icon", 78.0));
        // Head icon height 16, less the balloon's 5, over the head's 18.
        assert_eq!((&*q[1].shader, centre(&q[1])), (TALK_SHADER, 89.0));
        assert_eq!((&*q[2].shader, centre(&q[2])), (TALK_SHADER, 73.0));
    }
}
