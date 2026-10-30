//! CoD2's frame archive, what a killcam replays: every server frame's
//! followable playerstates, entities and roster while script has
//! `setarchive(true)` on (`SV_ArchiveSnapshot`, Mac 0x194da8), the lookup
//! `SV_GetArchivedClientInfo` (0x195a06) and `SV_BuildClientSnapshot`
//! (its archive arm inlined in `SV_SendClientSnapshot`, Mac 0x195f84) make of an age, and the time shift a replayed
//! record carries (docs/research/cod2-killcam.md).

use std::collections::BTreeMap;
use std::rc::Rc;

use vcod_cod2::net::delta::{
    ClientState2, HudElem2, MAX_HUDELEMS, MAX_OBJECTIVES, Objective2, PS_FIELDS, PlayerState2,
    field_index,
};
use vcod_cod2::net::fields::{HUDELEM_FIELDS, PLAYER_STATE_FIELDS};
use vcod_common::net::msg::EntityState;

use crate::engine::archive::FrameRing;
use crate::game::temp_entity::Scope;

pub use crate::engine::archive::MAX_FRAMES;
/// `sv_fps`, which turns an age into frames.
const SV_FPS: i32 = 1000 / crate::cod2::FRAME_MS;

/// One client's view as `GetFollowPlayerState` (0x1a28d2) answers it: the
/// playerstate with its current HUD half cleared (the memset of
/// `ps+0x7a8`, 0xf80 bytes). The archived HUD half is shared between
/// frames while it holds still.
#[derive(Clone, Debug, PartialEq)]
pub struct ArchivedPs {
    f: [u32; PS_FIELDS],
    stats: [i32; 6],
    ammo: [i32; 64],
    ammoclip: [i32; 64],
    objectives: [Objective2; MAX_OBJECTIVES],
    hud: Rc<[HudElem2; MAX_HUDELEMS]>,
    /// The client's team when archived, which the follow's spectate
    /// permission reads (the archived `clientState` team).
    pub team: i32,
}

impl ArchivedPs {
    pub fn new(ps: &PlayerState2, team: i32) -> ArchivedPs {
        ArchivedPs {
            f: ps.f,
            stats: ps.stats,
            ammo: ps.ammo,
            ammoclip: ps.ammoclip,
            objectives: ps.objectives,
            hud: Rc::new(ps.hud_archival),
            team,
        }
    }

    fn to_wire(&self) -> PlayerState2 {
        PlayerState2 {
            f: self.f,
            stats: self.stats,
            ammo: self.ammo,
            ammoclip: self.ammoclip,
            objectives: self.objectives,
            hud_archival: *self.hud,
            ..Default::default()
        }
    }
}

/// One frame's snapshot sources, each entity in CoD2's numbers (vcod's
/// `EntityState` over `PROTOCOL_118`'s table) with its broadcast flag, as
/// `Cod2Server::world_entities` built them.
#[derive(Default)]
pub struct Frame {
    /// `svs.time` when it was archived.
    pub time: i32,
    pub entities: BTreeMap<u32, Rc<(EntityState, bool)>>,
    /// The temp entities' scopes, by number.
    pub scopes: Vec<(u32, Scope)>,
    pub roster: Rc<Vec<ClientState2>>,
    /// By slot, every client `GetFollowPlayerState` answered for.
    pub clients: Vec<Option<Rc<ArchivedPs>>>,
}

/// What `SV_GetArchivedClientInfo` answers.
#[derive(Debug, PartialEq)]
pub enum ClientInfo {
    /// The archived record, its times shifted to now.
    Archived(Box<PlayerState2>, i32),
    /// No archived frame and an age of 0 or less: the live client, if
    /// it is active and playing.
    Live,
    None,
}

/// The archive: frames kept since the level loaded, the newest last.
#[derive(Default)]
pub struct Cod2Archive {
    ring: FrameRing<Frame>,
}

impl Cod2Archive {
    /// Keeps `frame` as the newest, dropping the oldest past
    /// [`MAX_FRAMES`]; an entity or HUD unchanged since the last frame
    /// shares its storage.
    pub fn push(&mut self, mut frame: Frame) {
        if let Some(last) = self.ring.last() {
            for (n, e) in frame.entities.iter_mut() {
                if let Some(o) = last.entities.get(n)
                    && o == e
                {
                    *e = o.clone();
                }
            }
            if frame.roster == last.roster {
                frame.roster = last.roster.clone();
            }
            for (c, o) in frame.clients.iter_mut().zip(&last.clients) {
                if let (Some(c), Some(o)) = (c.as_mut(), o)
                    && c.hud == o.hud
                {
                    Rc::make_mut(c).hud = o.hud.clone();
                }
            }
        }
        self.ring.push(frame);
    }

    pub fn frame(&self, index: i32) -> Option<&Rc<Frame>> {
        self.ring.frame(index)
    }

    /// Both lookups' arithmetic for a positive age with the archive on:
    /// the frame `sv_fps * age / 1000` back, clamped to the last
    /// [`MAX_FRAMES`] and to the first frame with the age rewritten to
    /// match, then the first kept frame from there; none sets the age to 0.
    /// VERIFIED, the stores in 0x195a06 and 0x195f84.
    pub fn lookup(&self, age_ms: &mut i32) -> Option<i32> {
        self.ring.lookup(age_ms, SV_FPS)
    }

    /// `SV_GetArchivedClientInfo` (0x195a06) for `client` at `age_ms`,
    /// `on` the archive flag and `now` `svs.time`. A frame found answers
    /// for the client or fails, with no live fallback; only an age of 0
    /// or less falls back to the live client. VERIFIED.
    pub fn client_info(&self, on: bool, client: usize, age_ms: &mut i32, now: i32) -> ClientInfo {
        if on
            && *age_ms > 0
            && let Some(i) = self.lookup(age_ms)
        {
            let f = self.ring.frame(i).expect("a found frame is kept");
            return match f.clients.get(client).and_then(Option::as_ref) {
                Some(ps) => {
                    let mut w = ps.to_wire();
                    shift_ps(&mut w, now - f.time, now);
                    ClientInfo::Archived(Box::new(w), ps.team)
                }
                None => ClientInfo::None,
            };
        }
        if *age_ms > 0 {
            ClientInfo::None
        } else {
            ClientInfo::Live
        }
    }
}

/// The origin in an archived playerstate.
pub fn ps_origin(ps: &PlayerState2) -> glam::Vec3 {
    let f = |n| f32::from_bits(ps.f[field_index(&PLAYER_STATE_FIELDS, n)]);
    glam::Vec3::new(f("origin[0]"), f("origin[1]"), f("origin[2]"))
}

/// `SV_GetClientPositionAtTime` (lnxded 0x8099dac): where a client stood
/// at `time`, `now` being `svs.time`. `origin_at` is
/// `SV_GetArchivedClientInfo` for an age, which it may rewrite. The older
/// bracket starts two frames past the age `now` snapped to a frame puts
/// `time` at and walks older, the newer one frame past and walks newer,
/// ten tries each; the two are lerped by `time % frame` over the ages
/// between them, and one alone answers as it is (cod2-combat.md 2.3).
/// With the archive off only an age walked down to 0 finds anything,
/// the live origin: no rewind.
/// Two brackets found at one age divide by 0 on retail's x87 and answer
/// NaN; vcod answers [`Rewind::Nowhere`] (cod2-combat.md 2.3.3).
/// `frame` is `1000 / sv_fps`: CoD2's 50, CoD4's dvar (1.7's walk at
/// 0x81798be divides by it too, cod4-server-frame.md 8).
pub fn position_at_time(
    now: i32,
    time: i32,
    frame: i32,
    mut origin_at: impl FnMut(&mut i32) -> Option<glam::Vec3>,
) -> Option<Rewind> {
    let back = ((now / frame) * frame - time) / frame;
    let mut age = (back + 2) * frame;
    let mut older_age = age;
    let mut older = None;
    for _ in 0..10 {
        if let Some(o) = origin_at(&mut age) {
            (older, older_age) = (Some(o), age);
            break;
        }
        age += frame;
    }
    let mut age = (back + 1) * frame;
    let mut newer_age = age;
    let mut newer = None;
    for _ in 0..10 {
        if let Some(o) = origin_at(&mut age) {
            (newer, newer_age) = (Some(o), age);
            break;
        }
        age -= frame;
    }
    let (a, b, f) = match (older, newer) {
        (None, None) => return None,
        (None, Some(b)) => (glam::Vec3::ZERO, b, 1.0f32),
        (Some(a), None) => (a, glam::Vec3::ZERO, 0.0),
        (Some(_), Some(_)) if older_age == newer_age => return Some(Rewind::Nowhere),
        (Some(a), Some(b)) => (a, b, (time % frame) as f32 / (older_age - newer_age) as f32),
    };
    // 0x809bf50's lerp in the x87's width, stored a float.
    let lerp =
        |a: f32, b: f32| (f64::from(b) * f64::from(f) + (1.0 - f64::from(f)) * f64::from(a)) as f32;
    Some(Rewind::At(glam::Vec3::new(
        lerp(a.x, b.x),
        lerp(a.y, b.y),
        lerp(a.z, b.z),
    )))
}

/// What `SV_GetClientPositionAtTime` answered with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rewind {
    /// Where the client stood.
    At(glam::Vec3),
    /// Retail's NaN: both brackets at one age (the archive empty, or
    /// holding fewer frames than the lag reaches back). The client is
    /// left where he stands and no round of the shot strikes him.
    Nowhere,
}

impl Rewind {
    pub fn at(self) -> Option<glam::Vec3> {
        match self {
            Rewind::At(o) => Some(o),
            Rewind::Nowhere => None,
        }
    }
}

/// The playerstate times a replay moves forward when non-zero
/// (`ps+0x0`, `+0x10`, `+0x44`, `+0x70`, `+0xfc`, `+0x5dc`).
const PS_TIMES: [&str; 6] = [
    "commandTime",
    "pm_time",
    "foliageSoundTime",
    "jumpTime",
    "viewHeightLerpTime",
    "shellshockTime",
];

/// The archived HUD half's times (`+0x28`, `+0x48`, `+0x60`, `+0x68` of
/// each element at `ps+0x1728`).
const HUD_TIMES: [&str; 4] = ["fadeStartTime", "scaleStartTime", "moveStartTime", "time"];

/// 0x195a06's shift: `shift` added to each non-zero time, `deltaTime`
/// whatever it held, and a fade start past `now` held at `now`. VERIFIED.
pub fn shift_ps(ps: &mut PlayerState2, shift: i32, now: i32) {
    for n in PS_TIMES {
        let i = field_index(&PLAYER_STATE_FIELDS, n);
        if ps.f[i] != 0 {
            ps.f[i] = ps.f[i].wrapping_add(shift as u32);
        }
    }
    let i = field_index(&PLAYER_STATE_FIELDS, "deltaTime");
    ps.f[i] = ps.f[i].wrapping_add(shift as u32);
    let fade = field_index(&HUDELEM_FIELDS, "fadeStartTime");
    for h in &mut ps.hud_archival {
        for n in HUD_TIMES {
            let i = field_index(&HUDELEM_FIELDS, n);
            if h[i] != 0 {
                h[i] = h[i].wrapping_add(shift as u32);
            }
        }
        if h[fade] as i32 > now {
            h[fade] = now as u32;
        }
    }
}

/// The entity times an archived snapshot moves forward when non-zero:
/// `pos.trTime`, `apos.trTime`, `time`, `time2` (`+0x10`, `+0x34`,
/// `+0x54`, `+0x58`, Mac 0x195f84). VERIFIED.
pub fn shift_entity(e: &mut vcod_cod2::net::EntityState2, shift: i32) {
    for n in ["pos.trTime", "apos.trTime", "time", "time2"] {
        let i = vcod_cod2::net::delta::field_index(&vcod_cod2::net::fields::ENTITY_STATE_FIELDS, n);
        if e.f[i] != 0 {
            e.f[i] = e.f[i].wrapping_add(shift as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ps(command_time: u32) -> PlayerState2 {
        let mut ps = PlayerState2::default();
        ps.f[field_index(&PLAYER_STATE_FIELDS, "commandTime")] = command_time;
        ps
    }

    /// `n` frames 50 ms apart, the newest at `n * 50`, client 1 in each.
    fn archive(n: i32) -> Cod2Archive {
        let mut a = Cod2Archive::default();
        for i in 1..=n {
            a.push(Frame {
                time: i * 50,
                clients: vec![None, Some(Rc::new(ArchivedPs::new(&ps(i as u32 * 50), 1)))],
                ..Default::default()
            });
        }
        a
    }

    /// The stock killcam's 9 s, less a -8 `psoffsettime`, is 180 frames
    /// back, shifted by the 9000 ms the record is old on the next frame
    /// (the one not archived yet).
    #[test]
    fn a_killcam_age_reads_the_record_nine_seconds_back() {
        let a = archive(400);
        let mut age = 9008;
        let ClientInfo::Archived(w, team) = a.client_info(true, 1, &mut age, 401 * 50) else {
            panic!()
        };
        assert_eq!((age, team), (9008, 1));
        assert_eq!(w.i32("commandTime"), 221 * 50 + 9000);
        assert_eq!(w.i32("deltaTime"), 9000);
        assert_eq!(
            a.client_info(true, 0, &mut 9000, 400 * 50),
            ClientInfo::None
        );
    }

    #[test]
    fn an_age_past_the_level_is_trimmed_and_none_falls_back_live() {
        let a = archive(60);
        let mut age = 9000;
        assert_eq!(a.lookup(&mut age), Some(0));
        assert_eq!(age, 3000);
        let mut age = 20;
        assert_eq!(a.lookup(&mut age), None);
        assert_eq!(age, 0);
        assert_eq!(a.client_info(true, 1, &mut 0, 0), ClientInfo::Live);
        assert_eq!(a.client_info(false, 1, &mut 50, 0), ClientInfo::None);
        let mut long = archive(2000);
        let mut age = 100_000;
        assert_eq!(long.lookup(&mut age), Some(2000 - MAX_FRAMES));
        assert_eq!(age, 60_000);
        long.push(Frame::default());
        assert!(long.frame(2000 - MAX_FRAMES).is_none());
    }

    /// The antilag's lookup: with the archive on, a time between two
    /// frames lerps them (a frame's own time reads the frame before); off, it finds only the live origin, and from
    /// nine frames back nothing at all.
    #[test]
    fn the_rewind_reads_the_archive_and_without_it_the_live_origin() {
        let mut a = Cod2Archive::default();
        for i in 1..=20 {
            let mut p = ps(i as u32 * 50);
            p.f[field_index(&PLAYER_STATE_FIELDS, "origin[0]")] = (i as f32 * 10.0).to_bits();
            a.push(Frame {
                time: i * 50,
                clients: vec![None, Some(Rc::new(ArchivedPs::new(&p, 1)))],
                ..Default::default()
            });
        }
        let live = glam::Vec3::new(-1.0, 0.0, 0.0);
        let at = |on: bool, time: i32| {
            position_at_time(1000, time, 1000 / SV_FPS, |age| {
                match a.client_info(on, 1, age, 1000) {
                    ClientInfo::Archived(ps, _) => Some(ps_origin(&ps)),
                    ClientInfo::Live => Some(live),
                    ClientInfo::None => None,
                }
            })
            .map(|r| r.at().expect("placed"))
        };
        assert_eq!(at(true, 970).map(|o| o.x), Some(194.0));
        // `time % frame` is 0 on a frame's own time: the frame before it.
        assert_eq!(at(true, 899).map(|o| o.x), Some(179.8));
        assert_eq!(at(true, 900).map(|o| o.x), Some(170.0));
        assert_eq!(at(false, 970), Some(live));
        assert_eq!(at(false, 1000 - 449), Some(live));
        assert_eq!(at(false, 1000 - 450), None);
    }
    /// Too few frames for the lag, or none since the archive came on: the
    /// two brackets land on one age, retail's 0 / 0. From `back + 2`
    /// frames on the lerp has two ages to divide by.
    #[test]
    fn a_rewind_past_the_archive_start_places_no_one() {
        let at = |frames: i32| {
            let mut a = Cod2Archive::default();
            for i in 1..=frames {
                let mut p = ps(i as u32 * 50);
                p.f[field_index(&PLAYER_STATE_FIELDS, "origin[0]")] = (i as f32).to_bits();
                a.push(Frame {
                    time: i * 50,
                    clients: vec![Some(Rc::new(ArchivedPs::new(&p, 1)))],
                    ..Default::default()
                });
            }
            let now = frames * 50 + 1000;
            position_at_time(now, now - 200, 1000 / SV_FPS, |age| {
                match a.client_info(true, 0, age, now) {
                    ClientInfo::Archived(ps, _) => Some(ps_origin(&ps)),
                    ClientInfo::Live => Some(glam::Vec3::splat(-1.0)),
                    ClientInfo::None => None,
                }
            })
        };
        assert_eq!(at(0), Some(Rewind::Nowhere));
        assert_eq!(at(5), Some(Rewind::Nowhere));
        assert!(at(6).and_then(Rewind::at).is_some_and(|o| o.is_finite()));
    }
}
