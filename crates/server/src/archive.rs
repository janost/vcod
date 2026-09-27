//! The frame archive the killcam replays: the engine's ring of every frame's
//! playerstates, entities and roster, the lookup that turns a client's
//! `archivetime` into a frame and trims it, and the time shift a replayed
//! frame carries (docs/research/cod11-spectator-follow.md, section 12).

use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use vcod_common::bsp::Visibility;
use vcod_common::net::msg::{self, hud_field};
use vcod_common::net::protocol::Protocol;

use crate::game::temp_entity::Scope;

/// How far back a request reaches, in frames (`cod_lnxded` 0x808ef09).
pub const MAX_FRAMES: i32 = 0x4b0;
/// `sv_fps`, which turns an age into frames.
const SV_FPS: i32 = 1000 / crate::server::FRAME_MS;

/// What one frame's snapshots take their entities and roster from, built once
/// per frame and kept by the archive as it was sent.
#[derive(Clone, Default)]
pub struct WorldFrame {
    /// Culled per client against its PVS: the map's entities, the bodies and
    /// every linked player.
    pub culled: BTreeMap<u32, Rc<msg::EntityState>>,
    /// The frame's temp entities, each with the snapshots it is scoped to.
    pub temps: Vec<(Scope, Rc<msg::EntityState>)>,
    /// `SVF_BROADCAST` entities outside the temp block, the missiles: sent
    /// with no cull.
    pub broadcast: BTreeMap<u32, Rc<msg::EntityState>>,
    pub roster: Rc<BTreeMap<u32, msg::ClientState>>,
}

impl WorldFrame {
    /// The entity list of a snapshot whose `ps.clientNum` is `client_num` and
    /// whose eye is `eye`: that client's own entity left out, the
    /// single-client scopes tested against it, the rest culled from the eye
    /// unless broadcast. `shift` is added to every non-zero time a replayed
    /// entity carries (0x808f130's archive arm).
    pub fn entities_for(
        &self,
        client_num: usize,
        eye: [f32; 3],
        vis: Option<&Visibility>,
        shift: i32,
        p: &Protocol,
    ) -> BTreeMap<u32, msg::EntityState> {
        let from = vis.map(|v| v.cluster_at(eye));
        let seen = |e: &msg::EntityState| match (vis, from) {
            (Some(v), Some(from)) => crate::world::entity_visible(v, from, e, p),
            _ => true,
        };
        let times = (shift != 0).then(|| entity_time_fields(p));
        let take = |e: &msg::EntityState| {
            let mut e = e.clone();
            if let Some(times) = &times {
                shift_times(&mut e.fields, times, shift);
            }
            e
        };
        let mut out = BTreeMap::new();
        for (n, e) in &self.culled {
            if *n != client_num as u32 && seen(e) {
                out.insert(*n, take(e));
            }
        }
        for (scope, e) in &self.temps {
            if scope.admits(client_num) && (*scope == Scope::Broadcast || seen(e)) {
                out.insert(e.number, take(e));
            }
        }
        for (n, e) in &self.broadcast {
            out.insert(*n, take(e));
        }
        out
    }
}

/// One client's own view as `GetFollowPlayerState` answers it at the end of a
/// frame: its playerstate with the archived HUD half and the objectives, and
/// where the copy's eye, view and feet were.
#[derive(Clone, Debug, PartialEq)]
pub struct ArchivedView {
    pub ps: msg::PlayerState,
    pub eye: [f32; 3],
    /// Degrees, wire convention.
    pub angles: [f32; 3],
    pub origin: [f32; 3],
}

/// One archived frame.
pub struct Frame {
    /// `svs.time` when it was archived.
    pub time: i32,
    pub world: WorldFrame,
    /// By slot: the view of every client whose own view was on. A client not
    /// connected, spectating or at intermission has none, and a request for
    /// it fails (0x808ef7c).
    pub clients: Vec<Option<Rc<ArchivedView>>>,
}

/// Where a follow's copy comes from (`trap_GetArchivedPlayerState`,
/// 0x808ef7c).
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// The client as that frame archived it.
    Archived {
        frame: i32,
        view: Rc<ArchivedView>,
    },
    /// No frame for the age and an age below 1: the client's live state.
    Live,
    None,
}

/// The engine's archive: on only once script has called `setarchive(true)`,
/// cleared by every map load and restart (0x808b4ac, 0x808b2c8).
#[derive(Default)]
pub struct Archive {
    on: bool,
    frames: VecDeque<Rc<Frame>>,
    /// Frames archived since the level loaded.
    count: i32,
}

impl Archive {
    pub fn is_on(&self) -> bool {
        self.on
    }

    pub fn set_on(&mut self, on: bool) {
        self.on = on;
    }

    /// The level boundary's reset: off, and nothing kept.
    pub fn clear(&mut self) {
        *self = Archive::default();
    }

    /// Keeps `frame` as the newest, dropping the oldest past [`MAX_FRAMES`].
    /// An entity or roster identical to the last frame's shares its storage.
    pub fn push(&mut self, mut frame: Frame) {
        if let Some(last) = self.frames.back() {
            share_unchanged(&mut frame.world.culled, &last.world.culled);
            share_unchanged(&mut frame.world.broadcast, &last.world.broadcast);
            if frame.world.roster == last.world.roster {
                frame.world.roster = last.world.roster.clone();
            }
        }
        if self.frames.len() >= MAX_FRAMES as usize {
            self.frames.pop_front();
        }
        self.frames.push_back(Rc::new(frame));
        self.count += 1;
    }

    /// The frame at an absolute index, while it is still kept.
    pub fn frame(&self, index: i32) -> Option<&Rc<Frame>> {
        let first = self.count - self.frames.len() as i32;
        let i = index.checked_sub(first)?;
        usize::try_from(i).ok().and_then(|i| self.frames.get(i))
    }

    /// `0x808eeb8`: the frame `age_ms` back. An age reaching past the last
    /// [`MAX_FRAMES`] or before the first frame is trimmed to what is kept,
    /// and an age that finds no frame at all becomes 0; both are written back
    /// through `age_ms`, which is the script's `archivetime`. Off, or asked
    /// for an age below 1, it answers nothing and leaves the age alone.
    pub fn lookup(&self, age_ms: &mut i32) -> Option<i32> {
        if !self.on || *age_ms < 1 {
            return None;
        }
        let cur = self.count;
        let back = (i64::from(SV_FPS) * i64::from(*age_ms) / 1000).min(i64::from(i32::MAX));
        let mut index = cur.saturating_sub(back as i32);
        let floor = cur - MAX_FRAMES;
        if index < floor {
            *age_ms = (cur - floor) * 1000 / SV_FPS;
            index = floor;
        }
        if index < 0 {
            index = 0;
            *age_ms = cur * 1000 / SV_FPS;
        }
        if index < cur && self.frame(index).is_some() {
            return Some(index);
        }
        *age_ms = 0;
        None
    }

    /// `trap_GetArchivedPlayerState` (0x808ef7c) for `client` at `age_ms`,
    /// trimming the age as [`Self::lookup`] does. `live` is whether the
    /// client could be followed live, which is what an age below 1 with no
    /// frame falls back to.
    pub fn player_state(&self, client: usize, age_ms: &mut i32, live: bool) -> Source {
        match self.lookup(age_ms) {
            Some(frame) => match self
                .frame(frame)
                .and_then(|f| f.clients.get(client).cloned().flatten())
            {
                Some(view) => Source::Archived { frame, view },
                None => Source::None,
            },
            None if *age_ms < 1 && live => Source::Live,
            None => Source::None,
        }
    }
}

fn share_unchanged(
    new: &mut BTreeMap<u32, Rc<msg::EntityState>>,
    old: &BTreeMap<u32, Rc<msg::EntityState>>,
) {
    for (n, e) in new.iter_mut() {
        if let Some(o) = old.get(n) {
            if o == e {
                *e = o.clone();
            }
        }
    }
}

/// The entity times a replay shifts: `pos.trTime`, `apos.trTime`, `time` and
/// `time2` (the four adds in 0x808f130's archive arm).
fn entity_time_fields(p: &Protocol) -> Vec<usize> {
    ["pos.trTime", "apos.trTime", "time", "time2"]
        .iter()
        .filter_map(|n| msg::EntityState::field_index(p, n))
        .collect()
}

/// The playerstate times a replay shifts (0x808ef7c): `deltaTime` takes the
/// age whatever it held, and these only when non-zero.
const PS_TIMES: [&str; 6] = [
    "commandTime",
    "pm_time",
    "iFoliageSoundTime",
    "jumpTime",
    "viewHeightLerpTime",
    "shellshockTime",
];

/// The four times of each archived HUD element a replay shifts.
const HUD_TIMES: [usize; 4] = [
    hud_field::FADE_START_TIME,
    hud_field::SCALE_START_TIME,
    hud_field::MOVE_START_TIME,
    hud_field::TIME,
];

fn shift_times(fields: &mut [i32], which: &[usize], shift: i32) {
    for &i in which {
        if fields[i] != 0 {
            fields[i] = fields[i].wrapping_add(shift);
        }
    }
}

/// The playerstate a replay of `view` puts on the wire, `shift` ms after it
/// was archived.
pub fn replayed_ps(view: &ArchivedView, shift: i32, p: &Protocol) -> msg::PlayerState {
    let mut ps = view.ps.clone();
    let which: Vec<usize> = PS_TIMES
        .iter()
        .filter_map(|n| msg::PlayerState::field_index(p, n))
        .collect();
    shift_times(&mut ps.fields, &which, shift);
    if let Some(i) = msg::PlayerState::field_index(p, "deltaTime") {
        ps.fields[i] = ps.fields[i].wrapping_add(shift);
    }
    for h in &mut ps.arrays.hud_archived {
        shift_times(&mut h.fields, &HUD_TIMES, shift);
    }
    ps
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::PROTOCOL_V1;

    const P: &Protocol = &PROTOCOL_V1;

    fn entity(n: u32, x: f32) -> Rc<msg::EntityState> {
        let mut e = msg::EntityState::null(P);
        e.number = n;
        e.fields[msg::EntityState::field_index(P, "pos.trBase[0]").unwrap()] = x.to_bits() as i32;
        Rc::new(e)
    }

    fn view(slot: i32, command_time: i32) -> Rc<ArchivedView> {
        let mut ps = msg::PlayerState::null(P);
        ps.fields[msg::PlayerState::field_index(P, "clientNum").unwrap()] = slot;
        ps.fields[msg::PlayerState::field_index(P, "commandTime").unwrap()] = command_time;
        Rc::new(ArchivedView {
            ps,
            eye: [0.0; 3],
            angles: [0.0; 3],
            origin: [0.0; 3],
        })
    }

    /// `n` frames at 50 ms apart, the newest at `n * 50`, client 1 in each.
    fn archive(n: i32) -> Archive {
        let mut a = Archive::default();
        a.set_on(true);
        for i in 1..=n {
            a.push(Frame {
                time: i * 50,
                world: WorldFrame::default(),
                clients: vec![None, Some(view(1, i * 50))],
            });
        }
        a
    }

    #[test]
    fn an_age_is_that_many_frames_back() {
        let a = archive(400);
        let mut age = 9000;
        assert_eq!(a.lookup(&mut age), Some(400 - 180));
        assert_eq!(age, 9000);
    }

    /// The stock killcam asks for 9 s on a level younger than that and reads
    /// back what the archive could serve.
    #[test]
    fn an_age_before_the_first_frame_is_trimmed_to_the_level_s_length() {
        let a = archive(60);
        let mut age = 9000;
        assert_eq!(a.lookup(&mut age), Some(0));
        assert_eq!(age, 3000);
    }

    #[test]
    fn an_age_past_the_kept_frames_is_trimmed_to_them() {
        let a = archive(2000);
        let mut age = 100_000;
        assert_eq!(a.lookup(&mut age), Some(2000 - MAX_FRAMES));
        assert_eq!(age, 60_000);
        assert!(a.frame(2000 - MAX_FRAMES - 1).is_none());
        assert!(a.frame(1999).is_some());
    }

    #[test]
    fn off_or_no_age_answers_nothing_and_leaves_the_age() {
        let mut a = archive(400);
        let mut age = 0;
        assert_eq!(a.lookup(&mut age), None);
        assert_eq!(age, 0);
        a.set_on(false);
        let mut age = 9000;
        assert_eq!(a.lookup(&mut age), None);
        assert_eq!(age, 9000);
    }

    /// An age under one frame lands on the frame not archived yet.
    #[test]
    fn an_age_that_finds_no_frame_becomes_zero() {
        let a = archive(400);
        let mut age = 30;
        assert_eq!(a.lookup(&mut age), None);
        assert_eq!(age, 0);
        let empty = archive(0);
        let mut age = 9000;
        assert_eq!(empty.lookup(&mut age), None);
        assert_eq!(age, 0);
    }

    #[test]
    fn a_client_the_frame_did_not_archive_fails_and_no_frame_falls_back_to_live() {
        let a = archive(400);
        let mut age = 9000;
        match a.player_state(1, &mut age, true) {
            Source::Archived { frame, view } => {
                assert_eq!(frame, 220);
                assert_eq!(view.ps.field_i32(P, "commandTime"), 221 * 50);
            }
            other => panic!("{other:?}"),
        }
        let mut age = 9000;
        assert_eq!(a.player_state(0, &mut age, true), Source::None);
        let mut age = 0;
        assert_eq!(a.player_state(0, &mut age, true), Source::Live);
        assert_eq!(a.player_state(0, &mut age, false), Source::None);
    }

    #[test]
    fn a_replay_shifts_the_times_that_are_set() {
        let mut v = (*view(1, 1000)).clone();
        let jump = msg::PlayerState::field_index(P, "jumpTime").unwrap();
        let foliage = msg::PlayerState::field_index(P, "iFoliageSoundTime").unwrap();
        v.ps.fields[jump] = 900;
        let mut h = msg::HudElem::default();
        h.set(hud_field::TIME, 5000);
        h.set(hud_field::FADE_TIME, 700);
        v.ps.arrays.hud_archived.push(h);
        v.ps.arrays.hud_current.push(h);
        let ps = replayed_ps(&v, 9000, P);
        assert_eq!(ps.field_i32(P, "commandTime"), 10_000);
        assert_eq!(ps.fields[jump], 9900);
        assert_eq!(ps.fields[foliage], 0);
        assert_eq!(ps.field_i32(P, "deltaTime"), 9000);
        assert_eq!(ps.arrays.hud_archived[0].get(hud_field::TIME), 14_000);
        assert_eq!(ps.arrays.hud_archived[0].get(hud_field::FADE_TIME), 700);
        assert_eq!(ps.arrays.hud_current[0].get(hud_field::TIME), 5000);
    }

    #[test]
    fn a_replayed_entity_s_times_shift_and_its_scope_follows_the_replayed_client() {
        let mut w = WorldFrame::default();
        let mut e = (*entity(80, 10.0)).clone();
        let tr = msg::EntityState::field_index(P, "pos.trTime").unwrap();
        e.fields[tr] = 1000;
        w.culled.insert(80, Rc::new(e));
        w.culled.insert(1, entity(1, 0.0));
        w.culled.insert(2, entity(2, 0.0));
        w.temps.push((Scope::Only(1), entity(958, 0.0)));
        w.temps.push((Scope::AllBut(1), entity(959, 0.0)));
        let got = w.entities_for(1, [0.0; 3], None, 9000, P);
        assert_eq!(got.keys().copied().collect::<Vec<_>>(), [2, 80, 958]);
        assert_eq!(got[&80].fields[tr], 10_000);
        assert_eq!(got[&2].fields[tr], 0);
    }

    #[test]
    fn an_unchanged_entity_is_kept_once() {
        let mut a = archive(0);
        let frame = |x: f32| {
            let mut w = WorldFrame::default();
            w.culled.insert(80, entity(80, 1.0));
            w.culled.insert(81, entity(81, x));
            Frame {
                time: 0,
                world: w,
                clients: Vec::new(),
            }
        };
        a.push(frame(1.0));
        a.push(frame(2.0));
        let (f0, f1) = (a.frame(0).unwrap(), a.frame(1).unwrap());
        assert!(Rc::ptr_eq(&f0.world.culled[&80], &f1.world.culled[&80]));
        assert!(!Rc::ptr_eq(&f0.world.culled[&81], &f1.world.culled[&81]));
        assert!(Rc::ptr_eq(&f0.world.roster, &f1.world.roster));
    }
}
