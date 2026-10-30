//! What a CoD2 connection shows the rest of the client, decoded once per
//! snapshot out of the wire states ([`super::Parser`]).
//!
//! The client's renderer-side code reads it through
//! `vcod::play::cod2::Cod2Session::frame()`, a [`Frame`]: the newest
//! [`SnapshotView`], the one before it and the render time between them.
//! Everything is in CoD2's own numbering ([`super::events`],
//! [`super::flags`]); angles are degrees, positions world units, times the
//! server's milliseconds.
//!
//! - [`Frame::entity_place`] places an entity at the render time: a
//!   `TR_INTERPOLATE` one, or a client's `TR_LINEAR_STOP` (players), lerps
//!   between the two snapshots as `CG_CalcEntityLerpPositions` does; every
//!   other one evaluates its trajectory at the render time
//!   ([`Trajectory2::evaluate`]), riding the script mover under it. An
//!   entity new in the snapshot or whose teleport bit flipped starts from
//!   its newer state ([`teleported`]).
//! - [`EntityView`] carries what a draw needs: `etype`, `model_index` (a
//!   1-based [`ConfigStrings::model`] index for general entities, script
//!   models, items and turrets), `eflags`, `legs_anim` / `torso_anim`
//!   (10-bit, the toggle bit 0x200 included), `weapon` (1-based into
//!   [`ConfigStrings::weapon_name`]), `client_num` for a player or corpse,
//!   and `raw`, the full 59-word state for anything else
//!   (`raw.i32("fTorsoPitch")`).
//! - [`PlayerView`] is the local playerstate: origin, eye height, view
//!   angles, `pm_type`, `client_num` and [`PlayerView::followed`].
//! - [`ConfigStrings`] is the live table: the gamestate's strings with every
//!   `d` / `x` `y` `z` update applied. Models and materials are looked up by
//!   their 1-based index, through the protocol's layout
//!   ([`super::protocol::CS_LAYOUT_118`]).

use std::collections::VecDeque;

use glam::Vec3;

use super::delta::{ClientState2, EntityState2, PlayerState2};
use super::events::{ET_SCRIPTMOVER, event_of};
use super::flags::EF_TELEPORT_BIT;
use super::parse::{Gamestate2, Parsed, Parser, Snapshot2};
use super::protocol::{CS_LAYOUT_118, CS_WEAPONS};
use crate::trajectory::{TR_INTERPOLATE, TR_LINEAR_STOP, Trajectory2};
use vcod_common::net::protocol::CsRange;

/// One entity of a snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityView {
    pub number: u32,
    pub etype: i32,
    /// The event a temp entity carries (`eType - ET_EVENTS`).
    pub event: Option<i32>,
    pub pos: Trajectory2,
    pub apos: Trajectory2,
    /// `es.index`: a model index, or an item or effect index by `etype`.
    pub model_index: i32,
    pub eflags: i32,
    pub legs_anim: i32,
    pub torso_anim: i32,
    pub weapon: i32,
    pub client_num: i32,
    pub ground_entity: i32,
    pub raw: EntityState2,
}

impl EntityView {
    pub fn new(es: &EntityState2) -> Self {
        let etype = es.i32("eType");
        EntityView {
            number: es.number,
            etype,
            event: event_of(etype),
            pos: Trajectory2::read(es, "pos"),
            apos: Trajectory2::read(es, "apos"),
            model_index: es.i32("index"),
            eflags: es.i32("eFlags"),
            legs_anim: es.i32("legsAnim"),
            torso_anim: es.i32("torsoAnim"),
            weapon: es.i32("weapon"),
            client_num: es.i32("clientNum"),
            ground_entity: es.i32("groundEntityNum"),
            raw: *es,
        }
    }
}

/// The local client's playerstate.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerView {
    pub command_time: i32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub viewangles: [f32; 3],
    /// `ANGLE2SHORT` units; a cmd's angles are the view's less these.
    pub delta_angles: [i32; 3],
    /// `viewHeightCurrent`, the eye over `origin`.
    pub view_height: f32,
    pub pm_type: i32,
    pub pm_flags: i32,
    pub eflags: i32,
    /// Whose view this is: the local client's own, or the one it follows.
    pub client_num: i32,
    /// The gamestate's client number, this connection's slot.
    pub own_client_num: i32,
    pub weapon: i32,
    pub raw: PlayerState2,
}

impl PlayerView {
    pub fn new(ps: &PlayerState2, own_client_num: i32) -> Self {
        let v = |k: &str| [0, 1, 2].map(|i| ps.f32(&format!("{k}[{i}]")));
        PlayerView {
            command_time: ps.i32("commandTime"),
            origin: ps.origin(),
            velocity: v("velocity"),
            viewangles: ps.viewangles(),
            delta_angles: [0, 1, 2].map(|i| ps.i32(&format!("delta_angles[{i}]"))),
            view_height: ps.f32("viewHeightCurrent"),
            pm_type: ps.i32("pm_type"),
            pm_flags: ps.i32("pm_flags"),
            eflags: ps.i32("eFlags"),
            client_num: ps.i32("clientNum"),
            own_client_num,
            weapon: ps.i32("weapon"),
            raw: ps.clone(),
        }
    }

    /// The client a spectator follows: `ps.clientNum` other than ours, as
    /// Q3's `cg.snap->ps.clientNum != cg.clientNum` test reads it.
    pub fn followed(&self) -> Option<i32> {
        (self.client_num != self.own_client_num).then_some(self.client_num)
    }

    /// The eye: origin plus the view height.
    pub fn eye(&self) -> [f32; 3] {
        let [x, y, z] = self.origin;
        [x, y, z + self.view_height]
    }
}

/// One decoded snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotView {
    pub message_num: i32,
    pub server_time: i32,
    pub snap_flags: u8,
    pub ps: PlayerView,
    /// Ascending by number.
    pub entities: Vec<EntityView>,
    pub clients: Vec<ClientState2>,
}

impl SnapshotView {
    pub fn new(s: &Snapshot2, own_client_num: i32) -> Self {
        SnapshotView {
            message_num: s.message_num,
            server_time: s.server_time,
            snap_flags: s.snap_flags,
            ps: PlayerView::new(&s.ps, own_client_num),
            entities: s.entities.iter().map(EntityView::new).collect(),
            clients: s.clients.clone(),
        }
    }

    pub fn entity(&self, number: u32) -> Option<&EntityView> {
        self.entities
            .binary_search_by_key(&number, |e| e.number)
            .ok()
            .map(|i| &self.entities[i])
    }
}

/// The configstring table, kept live.
#[derive(Clone, Debug, Default)]
pub struct ConfigStrings {
    strings: Vec<String>,
    /// An `x` chunk run waiting for its `z`.
    big: Option<(usize, String)>,
}

impl ConfigStrings {
    pub fn from_gamestate(gs: &Gamestate2) -> Self {
        let mut strings = vec![String::new(); super::ops::MAX_CONFIGSTRINGS];
        for (&i, s) in &gs.configstrings {
            strings[usize::from(i)] = s.clone();
        }
        ConfigStrings { strings, big: None }
    }

    pub fn get(&self, i: usize) -> &str {
        self.strings.get(i).map_or("", String::as_str)
    }

    /// Every slot, empty ones included, in index order.
    pub fn all(&self) -> &[String] {
        &self.strings
    }

    /// A server command: `d <index> <text>` (`SV_SetConfigstring`
    /// 0x8092780, `%c %i %s`, the text unquoted to the end), or a long one
    /// in `x` `y` `z` chunks (cod2-protocol.md 3.4). Returns the slot it
    /// changed; any other command is left alone.
    pub fn apply_command(&mut self, text: &[u8]) -> Option<usize> {
        let text = super::latin1(text);
        let letter = text.chars().next()?;
        if !matches!(letter, 'd' | 'x' | 'y' | 'z') || text.as_bytes().get(1) != Some(&b' ') {
            return None;
        }
        let (idx, val) = text[2..].split_once(' ').unwrap_or((&text[2..], ""));
        let i: usize = idx.parse().ok()?;
        if i >= self.strings.len() {
            return None;
        }
        match letter {
            'd' => {
                self.strings[i] = val.to_string();
                Some(i)
            }
            'x' => {
                self.big = Some((i, val.to_string()));
                None
            }
            _ => {
                let (bi, s) = self.big.as_mut().filter(|(bi, _)| *bi == i)?;
                s.push_str(val);
                if letter == 'y' {
                    return None;
                }
                let bi = *bi;
                self.strings[bi] = self.big.take()?.1;
                Some(bi)
            }
        }
    }

    /// The model of a 1-based `G_ModelIndex` index (`xmodel/...`).
    pub fn model(&self, index: i32) -> Option<&str> {
        self.block(CsRange::Model, index)
    }

    /// The material of a 1-based `G_ShaderIndex` index.
    pub fn material(&self, index: i32) -> Option<&str> {
        self.block(CsRange::Shader, index)
    }

    /// The effect of a 1-based `G_EffectIndex` index (`fx/...`).
    pub fn effect(&self, index: i32) -> Option<&str> {
        self.block(CsRange::Effect, index)
    }

    /// A 1-based `CS_WEAPONS` index's weapon name (`kar98k_mp`).
    pub fn weapon_name(&self, index: i32) -> Option<&str> {
        let i = usize::try_from(index).ok()?.checked_sub(1)?;
        self.get(CS_WEAPONS)
            .split(' ')
            .nth(i)
            .filter(|s| !s.is_empty())
    }

    /// The script menu a `t <n>` opens: 0-based from the block's first
    /// slot, as `GScr_PrecacheMenu` fills it (cod2-protocol.md 9).
    pub fn script_menu(&self, n: usize) -> Option<&str> {
        let (first, last) = CS_LAYOUT_118.bounds(CsRange::Menu);
        let s = self.get(first + n);
        (first + n <= last && !s.is_empty()).then_some(s)
    }

    /// Serverinfo's `mapname`.
    pub fn map_name(&self) -> Option<&str> {
        super::oob::info_value_for_key(self.get(0), "mapname")
    }

    fn block(&self, r: CsRange, index: i32) -> Option<&str> {
        let s = self.get(CS_LAYOUT_118.slot(r, index)?);
        (!s.is_empty()).then_some(s)
    }
}

/// `cl.snapshots`' depth (`PACKET_BACKUP`): `CL_GetSnapshot` (Mac
/// 0x17a02c) refuses a snapshot 32 messages behind the newest.
const PACKET_BACKUP: i32 = 32;
/// The parse rings' depth: `CL_GetSnapshot` refuses a snapshot whose
/// entities or clients the 2048-slot rings have since overwritten.
const MAX_PARSE: u64 = 0x800;
/// `SNAPFLAG_NOT_ACTIVE`: the cgame starts on no snapshot carrying it.
const SNAPFLAG_NOT_ACTIVE: u8 = 2;
/// `SNAPFLAG_SERVERCOUNT`, toggled by each server restart: a flip starts
/// the cgame over on that snapshot.
const SNAPFLAG_SERVERCOUNT: u8 = 4;

/// Where the cgame is in the snapshots, by message number:
/// `cg.snap`, `cg.nextSnap` (the same when no newer one was read yet) and
/// `cgs.processedSnapshotNum`, the last one asked of `CL_GetSnapshot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub cur: i32,
    pub next: i32,
    pub processed: i32,
}

/// A connection's view state: the live configstrings and the snapshots the
/// cgame may still read. Feed it every [`Parsed`] the parser returns.
#[derive(Clone, Debug, Default)]
pub struct NetView {
    pub configstrings: ConfigStrings,
    pub client_num: i32,
    /// The snapshot before [`Self::snap`] (`cl.snap` before the newest).
    pub prev: Option<SnapshotView>,
    /// The newest snapshot (`cl.snap`).
    pub snap: Option<SnapshotView>,
    /// Bumped by each gamestate: a new level (or the first) to load.
    pub gamestates: u32,
    /// The kept snapshots before `prev`, oldest first.
    older: VecDeque<SnapshotView>,
    /// Each kept snapshot's message number and where its entities and
    /// clients start in the parse rings.
    starts: VecDeque<(i32, u64, u64)>,
    /// The parse rings' ends: every entity and client state parsed.
    parsed: (u64, u64),
    /// [`Self::process`]'s cursor; `None` until the cgame starts.
    cgame: Option<Cursor>,
}

impl NetView {
    /// Apply what one message brought: a gamestate resets everything, then
    /// its server commands' configstring updates, then its snapshot.
    pub fn update(&mut self, parser: &Parser, parsed: &Parsed) {
        if parsed.gamestate
            && let Some(gs) = &parser.gamestate
        {
            self.configstrings = ConfigStrings::from_gamestate(gs);
            self.client_num = gs.client_num;
            self.prev = None;
            self.snap = None;
            self.older.clear();
            self.starts.clear();
            self.cgame = None;
            self.gamestates += 1;
        }
        for (_, text) in &parsed.commands {
            self.configstrings.apply_command(text);
        }
        let Some(s) = parsed.snapshot.and_then(|n| parser.frame(n)) else {
            return;
        };
        let start = self.parsed;
        self.parsed.0 += s.entities.len() as u64;
        self.parsed.1 += s.clients.len() as u64;
        if s.valid
            && self
                .snap
                .as_ref()
                .is_none_or(|old| s.server_time > old.server_time)
        {
            self.push(SnapshotView::new(s, self.client_num), start);
        }
    }

    /// Keep `v` as the newest snapshot, its entities and clients parsed from
    /// `start`; drop what `CL_GetSnapshot` can no longer return.
    pub fn push(&mut self, v: SnapshotView, start: (u64, u64)) {
        self.starts.push_back((v.message_num, start.0, start.1));
        if let Some(p) = self.prev.take() {
            self.older.push_back(p);
        }
        self.prev = self.snap.replace(v);
        while self.older.front().is_some_and(|o| !self.readable(o)) {
            self.older.pop_front();
        }
        let first = self.chain().next().map_or(0, |s| s.message_num);
        while self.starts.front().is_some_and(|&(n, ..)| n < first) {
            self.starts.pop_front();
        }
    }

    /// Nothing drawn lerps from before now: a `fast_restart`'s level
    /// spawns its entities again.
    pub fn reset_lerp(&mut self) {
        self.prev = None;
        self.older.clear();
        self.cgame = None;
    }

    /// Every kept snapshot, oldest first.
    fn chain(&self) -> impl Iterator<Item = &SnapshotView> {
        self.older
            .iter()
            .chain(self.prev.as_ref())
            .chain(self.snap.as_ref())
    }

    /// Whether `CL_GetSnapshot` (Mac 0x17a02c) still returns `s`: under 32
    /// messages behind the newest, its entities and clients not yet
    /// overwritten in the 2048-slot parse rings.
    fn readable(&self, s: &SnapshotView) -> bool {
        let Some(newest) = &self.snap else {
            return false;
        };
        // A view built by hand (a test) kept no parse starts.
        let rings = self
            .starts
            .iter()
            .find(|(n, ..)| *n == s.message_num)
            .is_none_or(|&(_, ents, clients)| {
                self.parsed.0 - ents < MAX_PARSE && self.parsed.1 - clients < MAX_PARSE
            });
        newest.message_num - s.message_num < PACKET_BACKUP && rings
    }

    fn by_num(&self, n: i32) -> Option<&SnapshotView> {
        self.chain()
            .find(|s| s.message_num == n)
            .filter(|s| self.readable(s))
    }

    /// `CG_ReadNextSnapshot`: the first readable snapshot past message
    /// `after` (any, for `None`), skipping those `CL_GetSnapshot` refuses.
    fn read_after(&self, after: Option<i32>) -> Option<&SnapshotView> {
        self.chain()
            .filter(|s| after.is_none_or(|a| s.message_num > a))
            .find(|s| self.readable(s))
    }

    /// `CG_ProcessSnapshots` (Mac 0x1df762) at cg time `t` from `from`:
    /// the cursor it leaves and the frame's time. With no snapshot read yet
    /// the cgame starts on the first readable one not flagged
    /// `SNAPFLAG_NOT_ACTIVE`. While `cg.nextSnap` is `cg.snap` it reads the
    /// next; one whose `SNAPFLAG_SERVERCOUNT` flipped becomes both
    /// (`CG_SetInitialSnapshot`), and with none to read the time is held at
    /// least at `cg.snap`'s. Otherwise it is done once `t` sits in
    /// `[cg.snap, cg.nextSnap)`, else `cg.nextSnap` becomes `cg.snap`.
    fn select(&self, from: Option<Cursor>, t: i32) -> Option<(Cursor, i32)> {
        let mut c = match from.filter(|c| self.by_num(c.cur).is_some()) {
            Some(c) => c,
            None => {
                let mut after = None;
                loop {
                    let s = self.read_after(after)?;
                    if s.snap_flags & SNAPFLAG_NOT_ACTIVE == 0 {
                        let n = s.message_num;
                        break Cursor {
                            cur: n,
                            next: n,
                            processed: n,
                        };
                    }
                    after = Some(s.message_num);
                }
            }
        };
        if self.by_num(c.next).is_none() {
            c.next = c.cur;
        }
        loop {
            let cur = self.by_num(c.cur)?;
            if c.next == c.cur {
                let Some(n) = self.read_after(Some(c.processed.max(c.cur))) else {
                    return Some((c, t.max(cur.server_time)));
                };
                c.processed = n.message_num;
                if (n.snap_flags ^ cur.snap_flags) & SNAPFLAG_SERVERCOUNT != 0 {
                    c.cur = n.message_num;
                    c.next = n.message_num;
                    continue;
                }
                c.next = n.message_num;
            }
            let next = self.by_num(c.next)?;
            if t >= cur.server_time && t < next.server_time {
                return Some((c, t));
            }
            c.cur = c.next;
        }
    }

    /// `CG_ProcessSnapshots` once a frame at cg time `t`: moves the cursor
    /// and returns the frame's time and the message numbers it asked
    /// `CL_GetSnapshot` for, each with whether one came back (what
    /// `CG_AddLagometerSnapshotInfo` records). Message numbers no
    /// snapshot was kept for (lost, or refused as invalid) count as refused.
    pub fn process(&mut self, t: i32) -> (i32, Vec<(i32, bool)>) {
        let Some((c, time)) = self.select(self.cgame, t) else {
            return (t, Vec::new());
        };
        // The first process reports every kept message from the oldest.
        let first = self.chain().next().map_or(c.processed, |s| s.message_num);
        let from = self.cgame.map_or(first, |o| o.processed + 1);
        let read = (from..=c.processed)
            .map(|n| (n, self.by_num(n).is_some()))
            .collect();
        self.cgame = Some(c);
        (time, read)
    }

    /// The cgame's cursor, `None` before [`Self::process`] ran.
    pub fn cursor(&self) -> Option<Cursor> {
        self.cgame
    }

    /// The view at cg time `t`: the pair [`Self::process`] left the cgame
    /// on, carried on to `t` the same way (without moving the cursor).
    /// Before any process it starts as the cgame would. With no readable
    /// snapshot to start on, the newest alone.
    pub fn frame(&self, t: i32) -> Option<Frame<'_>> {
        let newest = self.snap.as_ref()?;
        let Some((c, time)) = self.select(self.cgame, t) else {
            return Some(Frame {
                snap: newest,
                prev: None,
                time_ms: t.max(newest.server_time),
                frac: 0.0,
                configstrings: &self.configstrings,
            });
        };
        let cur = self.by_num(c.cur)?;
        let next = self.by_num(c.next)?;
        // `CG_SetFrameInterpolation` (Mac 0x1633be): 0 for one snapshot,
        // never below 0.
        let frac = if next.server_time == cur.server_time {
            0.0
        } else {
            ((time - cur.server_time) as f32 / (next.server_time - cur.server_time) as f32).max(0.0)
        };
        Some(Frame {
            snap: next,
            prev: (c.next != c.cur).then_some(cur),
            time_ms: time,
            frac,
            configstrings: &self.configstrings,
        })
    }
}

/// One render frame's view of the connection.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// `cg.nextSnap`: the snapshot whose entities are drawn, the newest
    /// once the frame's time passed every other.
    pub snap: &'a SnapshotView,
    /// `cg.snap`, the snapshot before it the frame lerps from; `None` when
    /// there is no newer one to lerp to (the two are the same).
    pub prev: Option<&'a SnapshotView>,
    /// The frame's time, `cg.time`: in `[prev, snap)` while lerping, else
    /// at least `snap`'s, past it when the clock runs ahead of the
    /// snapshots.
    pub time_ms: i32,
    /// `cg.frameInterpolation`: where `time_ms` sits from `prev` (0) to
    /// `snap` (1); 0 with no `prev`.
    pub frac: f32,
    pub configstrings: &'a ConfigStrings,
}

fn lerp3(a: [f32; 3], b: [f32; 3], f: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * f)
}

/// Angles lerped the short way round, degrees.
/// Whether two states of one entity straddle a teleport: their
/// [`EF_TELEPORT_BIT`]s differ, and the client snaps to the newer.
pub fn teleported(older_eflags: i32, newer_eflags: i32) -> bool {
    (older_eflags ^ newer_eflags) & EF_TELEPORT_BIT != 0
}

fn lerp_angles(a: Vec3, b: Vec3, f: f32) -> Vec3 {
    Vec3::from_array([0, 1, 2].map(|i| {
        let d = (b[i] - a[i] + 180.0).rem_euclid(360.0) - 180.0;
        a[i] + d * f
    }))
}

impl Frame<'_> {
    /// The older snapshot's state of the same entity, the one the cgame
    /// draws from (`cent->currentState`); the newest's for an entity new in
    /// [`Self::snap`] or whose teleport bit flipped (`CG_SetNextSnap`'s
    /// `CG_ResetEntity`, cod2-protocol.md 12.3).
    fn current<'s>(&'s self, e: &'s EntityView) -> &'s EntityView {
        self.prev
            .and_then(|p| p.entity(e.number))
            .filter(|old| !teleported(old.eflags, e.eflags))
            .unwrap_or(e)
    }

    /// The older snapshot's time, `cg.snap->serverTime`.
    fn older_ms(&self) -> i32 {
        self.prev.map_or(self.snap.server_time, |p| p.server_time)
    }

    /// Whether `CG_CalcEntityLerpPositions` (Mac 0x163c40) lerps it
    /// between the snapshots: `TR_INTERPOLATE`, or a client's
    /// `TR_LINEAR_STOP` (number below 64).
    pub fn interpolates(e: &EntityView) -> bool {
        e.pos.tr_type == TR_INTERPOLATE || (e.pos.tr_type == TR_LINEAR_STOP && e.number < 64)
    }

    /// Where an entity of [`Self::snap`] is drawn, origin and angles
    /// (degrees). An interpolated one sits at the lerp of its two states,
    /// each evaluated at its own snapshot's time
    /// (`CG_InterpolateEntityPosition`, Mac 0x1636d2); one new in `snap`
    /// at its state there. Anything else evaluates its older state at the
    /// render time, carried by the script mover it stands on
    /// (`CG_AdjustPositionForMover`, Mac 0x163b00, translation only).
    /// Retail then hands a player's yaw to his client info and zeroes the
    /// pitch and roll; that is the player drawer's.
    pub fn entity_place(&self, e: &EntityView) -> (Vec3, Vec3) {
        let cur = self.current(e);
        if Self::interpolates(cur) {
            let (pos, ang) = (
                e.pos.evaluate(self.snap.server_time),
                e.apos.evaluate(self.snap.server_time),
            );
            if std::ptr::eq(cur, e) {
                return (pos, ang);
            }
            let from = self.older_ms();
            return (
                cur.pos.evaluate(from).lerp(pos, self.frac),
                lerp_angles(cur.apos.evaluate(from), ang, self.frac),
            );
        }
        let mut origin = cur.pos.evaluate(self.time_ms);
        let ground = cur.ground_entity;
        if (1..=1021).contains(&ground)
            && let Some(mover) = self
                .snap
                .entity(ground as u32)
                .filter(|m| m.etype == ET_SCRIPTMOVER)
        {
            let mover = self.current(mover).pos;
            origin += mover.evaluate(self.time_ms) - mover.evaluate(self.older_ms());
        }
        (origin, cur.apos.evaluate(self.time_ms))
    }

    /// [`Self::entity_place`]'s origin.
    pub fn entity_origin(&self, e: &EntityView) -> Vec3 {
        self.entity_place(e).0
    }

    /// The local eye, lerped between the two playerstates.
    pub fn eye(&self) -> [f32; 3] {
        match self.prev {
            Some(p)
                if p.ps.client_num == self.snap.ps.client_num
                    && !teleported(p.ps.eflags, self.snap.ps.eflags) =>
            {
                lerp3(p.ps.eye(), self.snap.ps.eye(), self.frac)
            }
            _ => self.snap.ps.eye(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::delta::field_index;
    use crate::net::fields::ENTITY_STATE_FIELDS;

    #[test]
    fn configstring_updates_and_chunks() {
        let mut cs = ConfigStrings {
            strings: vec![String::new(); 2048],
            big: None,
        };
        assert_eq!(cs.apply_command(b"d 335 xmodel/prop_barrel"), Some(335));
        assert_eq!(cs.model(1), Some("xmodel/prop_barrel"));
        assert_eq!(cs.apply_command(b"x 1 \\a\\1"), None);
        assert_eq!(cs.apply_command(b"y 1 \\b"), None);
        assert_eq!(cs.apply_command(b"z 1 \\2"), Some(1));
        assert_eq!(cs.get(1), "\\a\\1\\b\\2");
        assert_eq!(cs.apply_command(b"d 7 defaultweapon_mp kar98k_mp"), Some(7));
        assert_eq!(cs.weapon_name(2), Some("kar98k_mp"));
        assert_eq!(cs.weapon_name(3), None);
        assert_eq!(cs.apply_command(b"v ui_x \"0\""), None);
        assert_eq!(cs.apply_command(b"d 1 "), Some(1));
        assert_eq!(cs.get(1), "");
    }

    /// A player entity (lerped, number under 64) at `x` with `eflags`.
    fn body(x: f32, eflags: i32) -> EntityView {
        let mut es = EntityState2::null(3);
        let mut set = |name: &str, v: u32| es.f[field_index(&ENTITY_STATE_FIELDS, name)] = v;
        set("eType", 1);
        set("pos.trType", TR_LINEAR_STOP as u32);
        set("pos.trBase[0]", x.to_bits());
        set("eFlags", eflags as u32);
        EntityView::new(&es)
    }

    fn snap(time: i32, entities: Vec<EntityView>) -> SnapshotView {
        SnapshotView {
            message_num: 0,
            server_time: time,
            snap_flags: 0,
            ps: PlayerView::new(&PlayerState2::default(), 0),
            entities,
            clients: Vec::new(),
        }
    }

    /// Half way between two snapshots a body lerps, unless its teleport bit
    /// flipped between them (a spawn, a teleport) or it was not in the
    /// older one: then it stands where the newer puts it.
    #[test]
    fn teleport_bit_and_entering_snap_to_the_newer_state() {
        let cs = ConfigStrings {
            strings: vec![String::new(); 2048],
            big: None,
        };
        let at = |older: Vec<EntityView>, newer: EntityView| {
            let (a, b) = (snap(1000, older), snap(1050, vec![newer]));
            let f = Frame {
                snap: &b,
                prev: Some(&a),
                time_ms: 1025,
                frac: 0.5,
                configstrings: &cs,
            };
            f.entity_origin(&b.entities[0]).x
        };
        assert_eq!(at(vec![body(0.0, 0)], body(100.0, 0)), 50.0);
        assert_eq!(at(vec![body(0.0, 0x40)], body(100.0, 0)), 50.0);
        assert_eq!(at(vec![body(0.0, 0)], body(100.0, EF_TELEPORT_BIT)), 100.0);
        assert_eq!(at(vec![body(0.0, EF_TELEPORT_BIT)], body(100.0, 0)), 100.0);
        assert_eq!(at(vec![], body(100.0, 0)), 100.0);
        assert!(teleported(0x42, 0x40));
        assert!(!teleported(0x42, 0x2));
    }

    /// Message `n` at `50 * n` ms, one body at x `n`.
    fn numbered(n: i32, flags: u8) -> SnapshotView {
        let mut s = snap(50 * n, vec![body(n as f32, 0)]);
        s.message_num = n;
        s.snap_flags = flags;
        s
    }

    fn view_of(nums: &[(i32, u8)]) -> NetView {
        let mut v = NetView::default();
        for &(n, flags) in nums {
            v.push(numbered(n, flags), (0, 0));
        }
        v
    }

    /// The drawn pair, frac and time at `t` after processing there.
    fn at(v: &mut NetView, t: i32) -> (Option<i32>, i32, f32, i32) {
        let (time, _) = v.process(t);
        let f = v.frame(time).unwrap();
        (
            f.prev.map(|p| p.message_num),
            f.snap.message_num,
            f.frac,
            f.time_ms,
        )
    }

    /// A time before the newest two lerps the two around it, as the cgame
    /// walks its kept snapshots one at a time; past the newest it holds it
    /// and the time runs on.
    #[test]
    fn the_cgame_lerps_the_pair_around_its_time() {
        let mut v = view_of(&[(1, 0), (2, 0), (3, 0), (4, 0)]);
        assert_eq!(at(&mut v, 60), (Some(1), 2, 0.2, 60));
        assert_eq!(at(&mut v, 125), (Some(2), 3, 0.5, 125));
        assert_eq!(at(&mut v, 230), (None, 4, 0.0, 230));
        // Once past a snapshot the cgame never goes back to it.
        assert_eq!(at(&mut v, 160), (None, 4, 0.0, 200));
        v.push(numbered(5, 0), (0, 0));
        assert_eq!(at(&mut v, 210), (Some(4), 5, 0.2, 210));
    }

    /// A lost message's number is refused and skipped: the pair straddles
    /// it. The cgame never starts on a `SNAPFLAG_NOT_ACTIVE` snapshot, and a
    /// `SNAPFLAG_SERVERCOUNT` flip starts it over on the flipped one.
    #[test]
    fn lost_inactive_and_restarted_snapshots() {
        let mut v = view_of(&[(1, SNAPFLAG_NOT_ACTIVE), (2, 0), (4, 0)]);
        let read = vec![(1, true), (2, true), (3, false), (4, true)];
        assert_eq!(v.process(110), (110, read));
        assert_eq!(at(&mut v, 150), (Some(2), 4, 0.5, 150));
        // A time before the cgame's first snapshot walks on to the newest
        // and holds there (`CG_ProcessSnapshots`' clamp).
        let mut v = view_of(&[(1, 0), (2, 0)]);
        assert_eq!(at(&mut v, 20), (None, 2, 0.0, 100));
        let mut v = view_of(&[
            (1, 0),
            (2, 0),
            (3, SNAPFLAG_SERVERCOUNT),
            (4, SNAPFLAG_SERVERCOUNT),
        ]);
        assert_eq!(at(&mut v, 60), (Some(1), 2, 0.2, 60));
        assert_eq!(at(&mut v, 160), (Some(3), 4, 0.2, 160));
    }

    /// `CL_GetSnapshot` refuses a snapshot 32 messages behind the newest.
    #[test]
    fn snapshots_32_behind_are_gone() {
        let v = view_of(&(1..=40).map(|n| (n, 0)).collect::<Vec<_>>());
        assert_eq!(v.chain().next().unwrap().message_num, 9);
        assert_eq!(v.frame(460).unwrap().prev.unwrap().message_num, 9);
    }

    #[test]
    fn angles_lerp_the_short_way() {
        let at = |yaw| Vec3::new(0.0, yaw, 0.0);
        assert_eq!(lerp_angles(at(350.0), at(10.0), 0.5).y, 360.0);
    }
}
