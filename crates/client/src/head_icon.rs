//! The sprites over a player's head: the icon a script sets with
//! `.headicon` / `.headiconteam`, then one of the killcam marker, the
//! disconnected icon and the talk balloon. Retail's player sprite pass is
//! cgame 0x300274d0; docs/research/cod11-gametypes-re-bel.md, section 7.3.
//! A United Offensive vehicle draws its own icon (uo cgame 0x30021771,
//! coduo-diff.md 9).

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;
use vcod_common::net::flags::{
    EF_DEAD, EF_DISCONNECTED, EF_NODRAW, EF_TALK, ET_PLAYER, ET_VEHICLE_CORPSE_V22, ET_VEHICLE_V22,
    TEAM_SPECTATOR,
};
use vcod_common::net::msg::{ClientState, EntityState};
use vcod_common::net::protocol::{Cod1Wire, CsRange, ENTITYNUM_NONE};

use crate::fx::sim::FxQuad;

/// Above the `Bip01 Head` bone.
const ABOVE_HEAD: f32 = 18.0;
/// Above the origin, for a body with no head bone.
const ABOVE_ORIGIN: f32 = 72.0;
/// Sprite half-extent.
const RADIUS: f32 = 6.66;
/// How far the second sprite sits over a drawn head icon (0x30027565).
const STACK: f32 = 16.0;
/// The talk balloon hangs this much lower than the other second sprites
/// (0x30069414).
const TALK_DROP: f32 = 5.0;
/// A vehicle's icon: this over its origin (plus `ABOVE_ORIGIN`), this
/// half-extent, never distance scaled (uo cgame 0x3002182a, 0x300213c0).
const VEHICLE_HEIGHT: f32 = 100.0;
const VEHICLE_RADIUS: f32 = 20.0;
const TALK_SHADER: &str = "headiconTalkBalloon";
const DISCONNECTED_SHADER: &str = "headiconDisconnected";
const KILLCAM_SHADER: &str = "headiconYouInKillCam";

/// Whether a viewer on `viewer_team` sees an icon limited to `icon_team`
/// (0 is everyone). A spectator sees every icon.
pub fn visible(icon: i32, icon_team: i32, viewer_team: i32) -> bool {
    icon != 0 && (icon_team == 0 || viewer_team == TEAM_SPECTATOR || icon_team == viewer_team)
}

/// The second sprite after a head icon, if any (0x3002756d..0x30027629).
#[derive(Debug, PartialEq)]
enum Second<'a> {
    /// Our own body while the view is someone else's.
    Killcam,
    Disconnected,
    /// A quick-chat line's icon (`cent+0x220` until `cent+0x224`).
    Voice(&'a str),
    Talk,
}

/// Which second sprite `num` carries. `own` is our client number, `viewer`
/// the viewed one (`ps.clientNum`), `voice` its live quick-chat icon; the
/// voice icon, else the talk balloon, shows only to a teammate or a
/// spectator.
fn second<'a>(
    num: u32,
    eflags: i32,
    team: i32,
    own: i32,
    viewer: i32,
    viewer_team: i32,
    voice: Option<&'a str>,
) -> Option<Second<'a>> {
    if num as i32 == own && viewer != own {
        Some(Second::Killcam)
    } else if eflags & EF_DISCONNECTED != 0 {
        Some(Second::Disconnected)
    } else if team != viewer_team && viewer_team != TEAM_SPECTATOR {
        None
    } else if let Some(shader) = voice {
        Some(Second::Voice(shader))
    } else if eflags & EF_TALK != 0 {
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
    pub protocol: &'a Cod1Wire,
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
    /// The vehicle `ps.clientNum` sits in (UO).
    pub riding: Option<u32>,
    /// A player's live quick-chat icon (`QuickChat::sprite`).
    pub voice: &'a dyn Fn(u32) -> Option<&'a str>,
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
        if matches!(int("eType"), ET_VEHICLE_V22 | ET_VEHICLE_CORPSE_V22)
            && p.et_events > ET_VEHICLE_CORPSE_V22
        {
            out.extend(vehicle_icon(s, num, ent, viewer_team, right, up));
            continue;
        }
        // The player draw (0x30028210) skips both.
        if int("eType") != ET_PLAYER || int("eFlags") & (EF_NODRAW | EF_DEAD) != 0 {
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
        let voice = (s.voice)(num);
        match second(
            num,
            int("eFlags"),
            team,
            s.own,
            s.viewer,
            viewer_team,
            voice,
        ) {
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
            Some(Second::Voice(shader)) => {
                out.push(sprite(anchor(head, origin, height), right, up, shader));
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

/// The icon over a drawn UO vehicle: its own `iHeadIcon`, which the
/// server copies from a tank's driver, unless it has no driver, the viewer
/// drives it or sits in it (uo cgame 0x30021771..0x3002183c).
fn vehicle_icon(
    s: &Scene,
    num: u32,
    ent: &EntityState,
    viewer_team: i32,
    right: Vec3,
    up: Vec3,
) -> Option<FxQuad> {
    let p = s.protocol;
    let int = |name: &str| ent.field_i32(p, name);
    let driver = int("otherEntityNum");
    if driver == ENTITYNUM_NONE as i32 || driver == s.viewer || s.riding == Some(num) {
        return None;
    }
    // Both the driver's roster entry and that of its entity's `clientNum`.
    let driver = u32::try_from(driver).ok()?;
    s.clients.get(&driver)?;
    let client = s
        .entities
        .get(&driver)
        .map_or(driver as i32, |e| e.field_i32(p, "clientNum"));
    s.clients.get(&u32::try_from(client).ok()?)?;
    let icon = int("iHeadIcon");
    if !visible(icon, int("iHeadIconTeam"), viewer_team) {
        return None;
    }
    let shader = s
        .configstrings
        .get(p.cs.base(CsRange::HeadIcon) + icon as usize)
        .filter(|n| !n.is_empty())?;
    let origin = s.entity_pos.get(&num)?;
    let centre = *origin + Vec3::Z * (VEHICLE_HEIGHT + ABOVE_ORIGIN);
    Some(sized_sprite(centre, VEHICLE_RADIUS, right, up, shader))
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
            riding: None,
            voice: &|_| None,
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
            second(3, talk | EF_DISCONNECTED, 1, 3, 5, 1, None),
            Some(Second::Killcam)
        );
        assert_eq!(
            second(3, talk, 1, 3, 3, 1, None),
            Some(Second::Talk),
            "own view"
        );
        // Disconnected shows to anyone and beats the balloon.
        assert_eq!(
            second(4, talk | EF_DISCONNECTED, 1, 0, 0, 2, None),
            Some(Second::Disconnected)
        );
        // The balloon: teammates and spectators only.
        assert_eq!(second(4, talk, 1, 0, 0, 1, None), Some(Second::Talk));
        assert_eq!(second(4, talk, 1, 0, 0, 2, None), None);
        assert_eq!(
            second(4, talk, 1, 0, 0, TEAM_SPECTATOR, None),
            Some(Second::Talk)
        );
        assert_eq!(
            second(4, talk, 0, 0, 0, 0, None),
            Some(Second::Talk),
            "free for all"
        );
        assert_eq!(second(4, 0, 1, 0, 0, 1, None), None);
        // A quick-chat icon takes the balloon's place, for the same viewers.
        let v = Some("headiconVoiceChat");
        assert_eq!(
            second(4, talk, 1, 0, 0, 1, v),
            Some(Second::Voice("headiconVoiceChat"))
        );
        assert_eq!(second(4, 0, 1, 0, 0, 2, v), None);
        assert_eq!(
            second(4, EF_DISCONNECTED, 1, 0, 0, 1, v),
            Some(Second::Disconnected)
        );
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
            riding: None,
            voice: &|_| None,
        };
        let q = quads(&scene, Vec3::Y, Vec3::Z);
        let centre = |q: &FxQuad| q.verts[0][2] - RADIUS;
        assert_eq!(q.len(), 3);
        assert_eq!((&*q[0].shader, centre(&q[0])), ("icon", 78.0));
        // Head icon height 16, less the balloon's 5, over the head's 18.
        assert_eq!((&*q[1].shader, centre(&q[1])), (TALK_SHADER, 89.0));
        assert_eq!((&*q[2].shader, centre(&q[2])), (TALK_SHADER, 73.0));
    }

    #[test]
    fn a_tank_shows_its_drivers_icon_to_others() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let ent = |etype: i32, other: i32, icon: i32, team: i32| {
            let mut e = EntityState::null(p);
            let mut set = |n: &str, v: i32| {
                e.fields[EntityState::field_index(p, n).unwrap()] = v;
            };
            set("eType", etype);
            set("clientNum", 5);
            set("otherEntityNum", other);
            set("iHeadIcon", icon);
            set("iHeadIconTeam", team);
            e
        };
        let entities = BTreeMap::from([
            (5, ent(ET_PLAYER, ENTITYNUM_NONE as i32, 0, 0)),
            (100, ent(ET_VEHICLE_V22, 5, 1, 1)),
            (101, ent(ET_VEHICLE_V22, 0, 1, 1)), // the viewer drives it
            (102, ent(ET_VEHICLE_V22, ENTITYNUM_NONE as i32, 1, 1)),
        ]);
        let clients: BTreeMap<u32, ClientState> = [0, 5]
            .into_iter()
            .map(|n| (n, ClientState::named(p, n, 1, "x")))
            .collect();
        let mut configstrings = vec![String::new(); 128];
        configstrings[p.cs.base(CsRange::HeadIcon) + 1] = "headicon".into();
        let entity_pos: HashMap<u32, Vec3> = (100..=102)
            .map(|n| (n, Vec3::new(0.0, 0.0, 30.0)))
            .collect();
        let heads = HashMap::new();
        let mut scene = Scene {
            protocol: p,
            entities: &entities,
            clients: &clients,
            configstrings: &configstrings,
            viewer: 0,
            own: 0,
            eye: Vec3::ZERO,
            heads: &heads,
            entity_pos: &entity_pos,
            riding: None,
            voice: &|_| None,
        };
        let q = quads(&scene, Vec3::Y, Vec3::Z);
        assert_eq!(q.len(), 1);
        assert_eq!(&*q[0].shader, "headicon");
        assert_eq!(
            q[0].verts[0],
            [0.0, -VEHICLE_RADIUS, 202.0 + VEHICLE_RADIUS]
        );
        // A passenger of the tank sees none over it.
        scene.riding = Some(100);
        assert!(quads(&scene, Vec3::Y, Vec3::Z).is_empty());
    }
}
