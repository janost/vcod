//! `--save-pickup`: the item capture and its fixture.

use super::combat::{PULSE_HOLD, WEAPONSTATE_READY, traj_str, weapon_cs_index};
use super::fixture::{
    FixturePhase, ITEMS_FIXTURE_DIR, cmd_fields, nonzero_pairs, pairs_str, serverinfo_key,
    tagged_name, vec_str, write_fixture_file, write_phase_blocks,
};
use super::*;

/// How long the capture waits past the join for the gsc probe's first
/// teleport before it gives up on the run.
const PICKUP_WAIT: Duration = Duration::from_secs(30);

const PICKUP_STAND: Duration = Duration::from_secs(1);

const PICKUP_AIM: Duration = Duration::from_secs(1);

/// Held on the target before a use tap goes out, so the tap's cmd carries the
/// settled view rather than the previous phase's.
const PICKUP_SETTLE: Duration = Duration::from_millis(300);

const PICKUP_AFTER_USE1: Duration = Duration::from_millis(1500);

const PICKUP_TOUCH2_WAIT: Duration = Duration::from_secs(10);

const PICKUP_TOUCH2: Duration = Duration::from_secs(2);

const PICKUP_SWITCH: Duration = Duration::from_secs(5);

/// The two taps on the dropped carbine, from the swap's own tap: one inside
/// the dropper's 1000 ms lockout and one past it.
const PICKUP_EARLY_TAP: Duration = Duration::from_millis(500);

const PICKUP_LATE_TAP: Duration = Duration::from_millis(1500);

const PICKUP_LATE: Duration = Duration::from_secs(3);

const PICKUP_WATCH: Duration = Duration::from_secs(3);

/// A jump this far between two snapshots is the gsc probe's teleport.
const PICKUP_JUMP: f32 = 200.0;

/// Items recorded around the probe, per snapshot.
const PICKUP_ITEM_RADIUS: f32 = 256.0;

/// mp_carentan's two fg42s, xy, off the entity lump. The gsc probe teleports
/// onto their origins; which one `getentarray` hands it first is not pinned,
/// so either counts as the first.
const PICKUP_FG42_XY: [[f32; 2]; 2] = [[468.0, -822.0], [838.0, 2222.0]];

/// Standing this close in xy to one of those is standing on it. The first
/// teleport lands before the join settles, so no jump is ever seen for it.
const PICKUP_ON_FG42: f32 = 48.0;

/// Which of [`PICKUP_FG42_XY`] `origin` stands on, if any.
fn pickup_on_fg42(origin: [f32; 3]) -> Option<usize> {
    PICKUP_FG42_XY
        .iter()
        .position(|&[x, y]| horiz_dist(origin, [x, y, origin[2]]) <= PICKUP_ON_FG42)
}

/// `--save-pickup`'s phases, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum PickupPhase {
    #[default]
    Wait,
    Stand,
    Aim,
    Use1,
    Touch2,
    Switch,
    Use2,
    Early,
    Late,
    Watch,
    Done,
}

impl PickupPhase {
    pub(super) fn label(self) -> &'static str {
        match self {
            PickupPhase::Wait => "wait",
            PickupPhase::Stand => "stand",
            PickupPhase::Aim => "aim",
            PickupPhase::Use1 => "use1",
            PickupPhase::Touch2 => "touch2",
            PickupPhase::Switch => "switch",
            PickupPhase::Use2 => "use2",
            PickupPhase::Early => "early",
            PickupPhase::Late => "late",
            PickupPhase::Watch => "watch",
            PickupPhase::Done => "done",
        }
    }
}

impl FixturePhase for PickupPhase {
    fn order(self) -> u8 {
        self as u8
    }
    fn label(self) -> &'static str {
        PickupPhase::label(self)
    }
}

/// The item capture: stand on an unowned fg42, aim at it, take it with use;
/// take the second one's ammo by touch; swap the carbine for a panzerfaust
/// and back, with one tap inside the dropper's lockout. The walking is the
/// gsc probe's two teleports.
#[derive(Default)]
pub(super) struct PickupProbe {
    pub(super) phase: PickupPhase,
    phase_started: Option<Instant>,
    settled_at: Option<Instant>,
    last_origin: Option<[f32; 3]>,
    /// Which of [`PICKUP_FG42_XY`] the first teleport landed on, when the
    /// wait saw it by position.
    first_fg42: Option<usize>,
    /// The touch2 phase's teleport, once seen.
    touch2_at: Option<Instant>,
    /// Absolute `(yaw, pitch)` in wire shorts.
    aim: Option<(i32, i32)>,
    /// When the current phase's use tap goes down.
    tap_at: Option<Instant>,
    /// The swap's own tap, which the two carbine taps are timed from.
    use2_tap: Option<Instant>,
    /// What the last `a <index>` named: a retail client switches to what it
    /// just picked up, so the byte rides every cmd until `ps.weapon` reads it.
    select: Option<u8>,
    /// The switch phase's own request.
    switch_to: Option<u8>,
    idx_fg42: u8,
    idx_carbine: u8,
    idx_panzer: u8,
    /// `(phase, ms, wire cmd, asked view)` per cmd sent from `stand` on.
    cmds: Vec<(PickupPhase, u128, net::msg::UserCmd, [i32; 3])>,
    /// Every trace, item and server line, in arrival order, with its phase.
    lines: Vec<(PickupPhase, u128, String)>,
    stations: Vec<(PickupPhase, [f32; 3], [f32; 3])>,
    traced: Option<u32>,
    pub(super) notes: Vec<String>,
    /// Every phase entered, in order.
    visited: Vec<PickupPhase>,
    /// The last cmds sent before the first spawned snapshot, of which those
    /// past its `commandTime` ran after the spawn and go in the log.
    before_spawn: std::collections::VecDeque<(net::msg::UserCmd, [i32; 3])>,
    /// The first spawned snapshot's message, once seen.
    spawned: Option<u32>,
    /// What each phase took: an `a <index>` or a pickup event on the
    /// playerstate's ring.
    took: Vec<(PickupPhase, String)>,
    events: vcod_common::net::events::EventTracker,
}

impl PickupProbe {
    pub(super) fn running(&self) -> bool {
        self.phase != PickupPhase::Done
    }

    pub(super) fn use_configstrings(&mut self, configstrings: &[String]) {
        let idx = |n: &str| weapon_cs_index(configstrings, n).map_or(0, |(i, _)| i);
        self.idx_fg42 = idx("fg42_mp");
        self.idx_carbine = idx("m1carbine_mp");
        self.idx_panzer = idx("panzerfaust_mp");
    }

    fn ms(&self, now: Instant) -> u128 {
        self.settled_at
            .map_or(0, |t| now.saturating_duration_since(t).as_millis())
    }

    pub(super) fn cmd(&self, now: Instant) -> net::msg::UserCmd {
        let mut cmd = net::msg::NULL_USERCMD;
        if let Some((yaw, pitch)) = self.aim {
            cmd.angles = [pitch, yaw, 0];
        }
        if self
            .tap_at
            .is_some_and(|t| now >= t && now.duration_since(t) < PULSE_HOLD)
        {
            cmd.buttons |= BUTTON_USE;
        }
        cmd
    }

    /// The weapon byte, or `None` to follow `ps.weapon`.
    pub(super) fn weapon_byte(&mut self, ps_weapon: u8) -> Option<u8> {
        for w in [&mut self.select, &mut self.switch_to] {
            if *w == Some(ps_weapon) {
                *w = None;
            }
        }
        self.select.or(self.switch_to)
    }

    pub(super) fn on_server_command(&mut self, now: Instant, tokens: &[String]) {
        if tokens.first().map(String::as_str) == Some("a") {
            self.select = tokens
                .get(1)
                .and_then(|t| t.parse().ok())
                .filter(|&i| i != 0);
            self.took.push((self.phase, tokens.join(" ")));
        }
        let ms = self.ms(now);
        self.lines.push((
            self.phase,
            ms,
            format!("!server ms={ms} {}", escape_ctl(&tokens.join(" "))),
        ));
    }

    pub(super) fn record(&mut self, now: Instant, sent: net::msg::UserCmd, asked: [i32; 3]) {
        match self.phase {
            PickupPhase::Done => {}
            PickupPhase::Wait if self.spawned.is_none() => {
                if self.before_spawn.len() == 8 {
                    self.before_spawn.pop_front();
                }
                self.before_spawn.push_back((sent, asked));
            }
            _ => {
                let ms = self.ms(now);
                self.cmds.push((self.phase, ms, sent, asked));
            }
        }
    }

    /// A snapshot before the settle: from the first spawned one on, one
    /// `!spawn` line each, so the spawn teleport's fall replays on the cmds
    /// that ran it (cod11-items.md 13.5).
    pub(super) fn before_settle(&mut self, snap: &net::snapshot::Snapshot) {
        let p = wire();
        let f = |n: &str| snap.ps.field_i32(p, n);
        if self.spawned.is_none() {
            if f("pm_type") != PM_NORMAL {
                return;
            }
            let ct = f("commandTime");
            let past: Vec<_> = self
                .before_spawn
                .drain(..)
                .filter(|(c, _)| c.server_time > ct)
                .collect();
            for (c, v) in past {
                self.cmds.push((PickupPhase::Wait, 0, c, v));
            }
        } else if self.spawned == Some(snap.message_num) {
            return;
        }
        self.spawned = Some(snap.message_num);
        self.lines.push((
            PickupPhase::Wait,
            0,
            format!(
                "!spawn t={} ct={} pm_type={} origin={}",
                snap.server_time,
                f("commandTime"),
                f("pm_type"),
                vec_str(snap.ps.origin(p)),
            ),
        ));
    }

    fn enter(&mut self, now: Instant, next: PickupPhase) {
        if next == self.phase {
            return;
        }
        println!(
            "PICKUP: {} -> {} at +{}ms",
            self.phase.label(),
            next.label(),
            self.ms(now)
        );
        self.phase = next;
        self.phase_started = Some(now);
        self.visited.push(next);
        if next == PickupPhase::Done {
            self.note_takes();
        }
        self.tap_at = match next {
            PickupPhase::Use1 | PickupPhase::Use2 => Some(now + PICKUP_SETTLE),
            PickupPhase::Early => self.use2_tap.map(|t| t + PICKUP_EARLY_TAP),
            PickupPhase::Late => self.use2_tap.map(|t| t + PICKUP_LATE_TAP),
            _ => None,
        };
        if next == PickupPhase::Use2 {
            self.use2_tap = self.tap_at;
        }
        if next == PickupPhase::Switch {
            self.switch_to = Some(self.idx_carbine).filter(|&i| i != 0);
        }
    }

    /// A phase that should have taken something and saw neither an `a` nor a
    /// pickup event would otherwise leave a fixture that reads as complete.
    /// `early` taps inside the dropper's lockout and should take nothing, but
    /// a neighbouring panzerfaust can answer it.
    fn note_takes(&mut self) {
        let took = |phase| {
            self.took
                .iter()
                .filter(|(p, _)| *p == phase)
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>()
        };
        let mut notes = Vec::new();
        for phase in [PickupPhase::Touch2, PickupPhase::Use2, PickupPhase::Late] {
            if self.visited.contains(&phase) && took(phase).is_empty() {
                notes.push(format!("# BROKEN {} took nothing", phase.label()));
            }
        }
        let early = took(PickupPhase::Early);
        if !early.is_empty() {
            notes.push(format!("# early took {}", early.join(", ")));
        }
        self.notes.extend(notes);
    }

    /// The item with `index` nearest `eye`, where the snapshot carries one.
    fn nearest_item(snap: &net::snapshot::Snapshot, index: u8, eye: [f32; 3]) -> Option<[f32; 3]> {
        let p = wire();
        snap.entities
            .values()
            .filter(|e| e.field_i32(p, "eType") == crate::entities::ET_ITEM)
            .filter(|e| e.field_i32(p, "index") == i32::from(index))
            .map(|e| {
                let at: [f32; 3] = Trajectory::read(e, p, "pos")
                    .evaluate(snap.server_time)
                    .into();
                at
            })
            .min_by(|a, b| dist(*a, eye).total_cmp(&dist(*b, eye)))
    }

    fn trace(&mut self, ms: u128, snap: &net::snapshot::Snapshot) {
        let p = wire();
        let f = |n: &str| snap.ps.field_i32(p, n);
        let origin = snap.ps.origin(p);
        self.lines.push((
            self.phase,
            ms,
            format!(
                "!trace ms={ms} serverTime={} pm_type={} weapon={} weaponstate={} weapons={} \
slots={},{} clip={} ammo={} health={} hint={}:{}:{} eventSequence={} events={},{},{},{} \
eventParms={},{},{},{} origin={} viewangles={}",
                snap.server_time,
                f("pm_type"),
                f("weapon"),
                f("weaponstate"),
                weapon_bits(&snap.ps, p),
                f("weaponslots[0]"),
                f("weaponslots[4]"),
                pairs_str(&nonzero_pairs(&snap.ps.arrays.ammoclip)),
                pairs_str(&nonzero_pairs(&snap.ps.arrays.ammo)),
                snap.ps.health(),
                f("serverCursorHint"),
                f("serverCursorHintVal"),
                f("serverCursorHintString"),
                f("eventSequence"),
                ev_v1(f("events[0]")),
                ev_v1(f("events[1]")),
                ev_v1(f("events[2]")),
                ev_v1(f("events[3]")),
                f("eventParms[0]"),
                f("eventParms[1]"),
                f("eventParms[2]"),
                f("eventParms[3]"),
                vec_str(origin),
                vec_str(snap.ps.viewangles(p)),
            ),
        ));
        for (&num, e) in &snap.entities {
            if e.field_i32(p, "eType") != crate::entities::ET_ITEM {
                continue;
            }
            let pos = Trajectory::read(e, p, "pos");
            let at: [f32; 3] = pos.evaluate(snap.server_time).into();
            if dist(at, origin) > PICKUP_ITEM_RADIUS {
                continue;
            }
            let apos = Trajectory::read(e, p, "apos");
            let g = |n: &str| e.field_i32(p, n);
            self.lines.push((
                self.phase,
                ms,
                format!(
                    "!item ms={ms} num={num} index={} clientNum={} eFlags={} groundEntityNum={} pos={} apos={}",
                    g("index"),
                    g("clientNum"),
                    g("eFlags"),
                    g("groundEntityNum"),
                    traj_str(&pos),
                    traj_str(&apos),
                ),
            ));
        }
    }

    /// Feeds the newest snapshot in. Returns true once the run is done.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let p = wire();
        self.settled_at.get_or_insert(now);
        let phase_started = *self.phase_started.get_or_insert(now);
        let in_phase = now.duration_since(phase_started);
        let ms = self.ms(now);
        let origin = snap.ps.origin(p);
        let h = snap.ps.field_f32(p, "viewHeightCurrent");
        let eye = [
            origin[0],
            origin[1],
            origin[2] + if h > 0.0 { h } else { EYE_HEIGHT },
        ];
        let jumped = self
            .last_origin
            .is_some_and(|o| dist(o, origin) > PICKUP_JUMP);
        self.last_origin = Some(origin);
        let on_fg42 = pickup_on_fg42(origin);

        if snap.ps.field_i32(p, "pm_type") == PM_DEAD {
            self.notes
                .push(format!("# BROKEN died in {}", self.phase.label()));
            self.enter(now, PickupPhase::Done);
            return true;
        }

        let target = match self.phase {
            PickupPhase::Aim | PickupPhase::Use1 => Some(self.idx_fg42),
            PickupPhase::Use2 => Some(self.idx_panzer),
            PickupPhase::Early | PickupPhase::Late => Some(self.idx_carbine),
            _ => None,
        };
        // Latched once the tap is down: a taken item leaves the wire, and the
        // view must not swing to whatever `nearest_item` finds next.
        let tapped = self.tap_at.is_some_and(|t| now >= t);
        if !(tapped && self.aim.is_some()) {
            self.aim = Some(
                match target.and_then(|i| Self::nearest_item(snap, i, eye)) {
                    Some(at) => {
                        let (yaw, pitch) = aim_at(eye, at);
                        (yaw & 0xffff, pitch & 0xffff)
                    }
                    None => {
                        let v = snap.ps.viewangles(p);
                        (deg_to_short(v[1]) & 0xffff, deg_to_short(v[0]) & 0xffff)
                    }
                },
            );
        }

        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            self.trace(ms, snap);
        }
        for ev in drain_v1(&mut self.events, snap, p) {
            let own = ev.entity_num == u32::MAX;
            if own
                && matches!(
                    ev.event,
                    crate::fx::registry::EV_ITEM_PICKUP | crate::fx::registry::EV_AMMO_PICKUP
                )
            {
                self.took
                    .push((self.phase, format!("event {} parm {}", ev.event, ev.parm)));
            }
        }

        let weapon = snap.ps.field_i32(p, "weapon") as u8;
        let ready = snap.ps.field_i32(p, "weaponstate") == WEAPONSTATE_READY;
        let since_use2 = self.use2_tap.map(|t| now.saturating_duration_since(t));
        let next = match self.phase {
            PickupPhase::Wait if jumped || on_fg42.is_some() => {
                self.first_fg42 = on_fg42;
                self.stations
                    .push((PickupPhase::Stand, origin, snap.ps.viewangles(p)));
                PickupPhase::Stand
            }
            PickupPhase::Wait if in_phase >= PICKUP_WAIT => {
                self.notes.push(
                    "# BROKEN no teleport: run client-probes/probe_pickup with \
+set probe_teleport 1 +set scr_allow_fg42 1"
                        .to_string(),
                );
                PickupPhase::Done
            }
            PickupPhase::Stand if in_phase >= PICKUP_STAND => PickupPhase::Aim,
            PickupPhase::Aim if in_phase >= PICKUP_AIM => PickupPhase::Use1,
            PickupPhase::Use1 if in_phase >= PICKUP_SETTLE + PICKUP_AFTER_USE1 => {
                PickupPhase::Touch2
            }
            PickupPhase::Touch2 => {
                let on_second = on_fg42.is_some() && on_fg42 != self.first_fg42;
                if (jumped || on_second) && self.touch2_at.is_none() {
                    self.touch2_at = Some(now);
                    self.stations
                        .push((PickupPhase::Touch2, origin, snap.ps.viewangles(p)));
                }
                match self.touch2_at {
                    Some(t) if now.duration_since(t) >= PICKUP_TOUCH2 => PickupPhase::Switch,
                    None if in_phase >= PICKUP_TOUCH2_WAIT => {
                        self.notes
                            .push("# BROKEN no second teleport: fg42 #1 was not taken".to_string());
                        PickupPhase::Done
                    }
                    _ => PickupPhase::Touch2,
                }
            }
            PickupPhase::Switch
                if (weapon == self.idx_carbine && ready) || in_phase >= PICKUP_SWITCH =>
            {
                PickupPhase::Use2
            }
            PickupPhase::Use2
                if since_use2.is_some_and(|d| d >= PICKUP_EARLY_TAP - PICKUP_SETTLE) =>
            {
                PickupPhase::Early
            }
            PickupPhase::Early
                if since_use2.is_some_and(|d| d >= PICKUP_LATE_TAP - PICKUP_SETTLE) =>
            {
                PickupPhase::Late
            }
            PickupPhase::Late if since_use2.is_some_and(|d| d >= PICKUP_LATE_TAP + PICKUP_LATE) => {
                PickupPhase::Watch
            }
            PickupPhase::Watch if in_phase >= PICKUP_WATCH => PickupPhase::Done,
            p => p,
        };
        self.enter(now, next);
        self.phase == PickupPhase::Done
    }
}

/// The pickup fixture: header, notes, then one `[phase <label>]` block per
/// phase, its station first and its cmds and lines interleaved by ms. Named
/// `dm` whatever cs 0 says: retail runs the capture as gametype
/// `probe_pickup`, which is `dm` underneath.
pub(super) fn write_pickup_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    pk: &PickupProbe,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let (map, gametype) = (key("mapname"), key("g_gametype"));
    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server: what a weapon pickup does to a player and to the items.\n",
        retail_build()
    ));
    out.push_str(&format!(
        "# map {map}, gametype {gametype}, joined {}, weapon {}\n",
        join.team, join.weapon
    ));
    out.push_str("# Two shells, the gsc probe first:\n");
    out.push_str("#   COD_LNXDED_HOME=<absolute, no '+'> SECS=150 \\\n");
    out.push_str(
        "#       tools/run_probe.sh client-probes/probe_pickup mp_carentan +set probe_teleport 1 +set scr_allow_fg42 1\n",
    );
    out.push_str(
        "#   cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-pickup --probe-secs 120\n",
    );
    out.push_str(
        "# scr_allow_fg42 1 is not stock: default_mp.cfg sets it 0, and _teams::restrictPlacedWeapons\n\
# then deletes both fg42s at map load. A run without it has no fg42 to take.\n",
    );
    out.push_str(
        "# The gsc probe puts the player on the first fg42 at spawn and on the second two\n",
    );
    out.push_str(
        "# seconds after the first is taken; every jump in origin here is one of those.\n",
    );
    out.push_str(&format!(
        "# Phases: stand {} ms; aim {} ms; use1 taps use {} ms in; touch2 holds {} ms past\n",
        PICKUP_STAND.as_millis(),
        PICKUP_AIM.as_millis(),
        PICKUP_SETTLE.as_millis(),
        PICKUP_TOUCH2.as_millis()
    ));
    out.push_str(&format!(
        "# the second teleport; switch asks for the carbine; use2 taps on the nearest panzerfaust;\n\
# early and late tap on the dropped carbine {} and {} ms after use2's tap.\n",
        PICKUP_EARLY_TAP.as_millis(),
        PICKUP_LATE_TAP.as_millis()
    ));
    out.push_str("# !cmd view= is the absolute view asked for (wire shorts, before delta_angles); angles= went on the wire.\n");
    out.push_str("# [phase wait] holds every cmd from the first spawned snapshot's commandTime on and a !spawn\n");
    out.push_str("# line per snapshot until the settle: t serverTime, ct commandTime.\n");
    out.push_str("# !trace clip and ammo are the non-zero entries as index:value; hint is serverCursorHint:Val:String.\n");
    out.push_str("# !item is every eType 3 entity within 256 units; pos and apos are type,time,base xyz,delta xyz.\n");
    out.push_str("# !server is every server command, tokens joined, control bytes as \\xNN.\n");
    for n in &pk.notes {
        out.push_str(n);
        out.push('\n');
    }
    pickup_phase_blocks(&mut out, pk);
    let path = write_fixture_file(
        ITEMS_FIXTURE_DIR,
        &tagged_name(&format!("{map}-dm-pickup"), tag),
        &out,
        overwrite,
    )?;
    println!("pickup: wrote {path}");
    Ok(())
}

/// [`write_pickup_fixture`]'s `[phase]` blocks; a `!trace` keeps its `!item` lines after it.
fn pickup_phase_blocks(out: &mut String, pk: &PickupProbe) {
    let cmds: Vec<(PickupPhase, u128, String)> = pk
        .cmds
        .iter()
        .map(|(p, ms, c, v)| {
            let row = format!(
                "!cmd ms={ms} {} view={},{},{}",
                cmd_fields(c),
                v[0],
                v[1],
                v[2]
            );
            (*p, *ms, row)
        })
        .collect();
    write_phase_blocks(out, &pk.stations, &cmds, &pk.lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::tests::pickup_snap;

    /// The writer's old inline loop, kept as the reference [`pickup_phase_blocks`] must match.
    fn old_pickup_phase_blocks(pk: &PickupProbe) -> String {
        let mut out = String::new();
        let mut phases: Vec<PickupPhase> = Vec::new();
        for p in pk
            .cmds
            .iter()
            .map(|c| c.0)
            .chain(pk.lines.iter().map(|l| l.0))
        {
            if !phases.contains(&p) {
                phases.push(p);
            }
        }
        phases.sort_by_key(|p| *p as u8);
        for phase in phases {
            out.push_str(&format!("[phase {}]\n", phase.label()));
            for (_, o, v) in pk.stations.iter().filter(|s| s.0 == phase) {
                out.push_str(&format!(
                    "!station origin={} viewangles={}\n",
                    vec_str(*o),
                    vec_str(*v)
                ));
            }
            let mut rows: Vec<(u128, String)> = pk
            .cmds
            .iter()
            .filter(|c| c.0 == phase)
            .map(|(_, ms, c, v)| {
                (
                    *ms,
                    format!(
                        "!cmd ms={ms} st={} buttons={} wbuttons={} weapon={} up={} forward={} right={} angles={},{},{} view={},{},{}",
                        c.server_time, c.buttons, c.wbuttons, c.weapon, c.up, c.forward, c.right,
                        c.angles[0], c.angles[1], c.angles[2], v[0], v[1], v[2]
                    ),
                )
            })
            .chain(
                pk.lines
                    .iter()
                    .filter(|l| l.0 == phase)
                    .map(|(_, ms, l)| (*ms, l.clone())),
            )
            .collect();
            // Stable, so a `!trace` keeps its `!item` lines after it.
            rows.sort_by_key(|r| r.0);
            for (_, r) in rows {
                out.push_str(&r);
                out.push('\n');
            }
        }
        out
    }

    /// The `[phase]` blocks through `write_phase_blocks` are byte for byte the old loop's:
    /// phases out of order, a phase with only a station, ms ties between cmds and lines.
    #[test]
    fn pickup_phase_blocks_match_the_old_loop() {
        let c = |server_time, forward| net::msg::UserCmd {
            server_time,
            buttons: 1,
            wbuttons: 2,
            weapon: 3,
            angles: [4, -5, 65535],
            forward,
            right: -7,
            up: 8,
            ..Default::default()
        };
        let pk = PickupProbe {
            cmds: vec![
                (PickupPhase::Use1, 30, c(100, 1), [1, 2, 3]),
                (PickupPhase::Aim, 10, c(90, -1), [0, 0, 0]),
                (PickupPhase::Use1, 20, c(110, 127), [9, -9, 9]),
            ],
            lines: vec![
                (PickupPhase::Use1, 30, "!trace a".into()),
                (PickupPhase::Use1, 30, "!trace b".into()),
                (PickupPhase::Use1, 20, "!event c".into()),
                (PickupPhase::Stand, 5, "!trace d".into()),
                (PickupPhase::Use1, 30, "!event e".into()),
            ],
            stations: vec![
                (PickupPhase::Use1, [1.25, -2.0, 3.0], [0.0, 90.0, 0.0]),
                (PickupPhase::Done, [0.0; 3], [0.0; 3]),
            ],
            ..PickupProbe::default()
        };
        let mut out = String::new();
        pickup_phase_blocks(&mut out, &pk);
        assert!(out.contains("[phase "), "{out}");
        assert_eq!(out, old_pickup_phase_blocks(&pk));
    }

    /// `dm` spawns and the gsc teleports before the join settles, so the
    /// capture's first snapshot is already on the fg42 and no jump is ever
    /// seen for it. The second teleport is seen by position
    /// too, whichever fg42 `getentarray` handed the gsc first.
    #[test]
    fn pickup_starts_on_the_first_fg42_without_seeing_a_jump() {
        use net::msg::PlayerState;
        let p = &net::protocol::PROTOCOL_V1;
        let snap = |num: u32, o: [f32; 3]| {
            let mut ps = PlayerState::null(p);
            for (i, v) in o.into_iter().enumerate() {
                ps.fields[PlayerState::field_index(p, &format!("origin[{i}]")).unwrap()] =
                    v.to_bits() as i32;
            }
            net::snapshot::Snapshot {
                server_time: 1000 + 50 * num as i32,
                message_num: num,
                delta_num: -1,
                snap_flags: 0,
                ps,
                entities: BTreeMap::new(),
                clients: BTreeMap::new(),
                valid: true,
            }
        };
        let [second, first] = PICKUP_FG42_XY;
        let t0 = Instant::now();
        let mut pk = PickupProbe::default();
        assert!(!pk.step(t0, &snap(1, [first[0] + 10.0, first[1], 20.0])));
        assert_eq!(pk.phase, PickupPhase::Stand);
        assert_eq!(pk.first_fg42, Some(1));

        pk.phase = PickupPhase::Touch2;
        pk.phase_started = Some(t0);
        pk.last_origin = None;
        pk.step(t0, &snap(2, [first[0], first[1], 20.0]));
        assert!(pk.touch2_at.is_none(), "still on the first fg42");
        pk.step(t0, &snap(3, [second[0], second[1] - 30.0, 20.0]));
        assert!(pk.touch2_at.is_some(), "on the second fg42");
    }

    /// The swap's tap and the two carbine taps rise at +0, +500 and +1500 ms
    /// of the swap's own tap, on the probe's own send clock.
    #[test]
    fn pickup_taps_use_at_the_swap_and_either_side_of_the_lockout() {
        let mut pk = PickupProbe {
            phase: PickupPhase::Switch,
            idx_carbine: 10,
            ..PickupProbe::default()
        };
        let t0 = Instant::now();
        let mut rises = Vec::new();
        let mut down = false;
        // 4 ms divides every mark, so a rise lands on it rather than after it.
        for k in 0..800u32 {
            let now = t0 + Duration::from_millis(u64::from(k) * 4);
            let snap = pickup_snap(k + 1, &[("weapon", 10)], &[]);
            pk.step(now, &snap);
            let pressed = pk.cmd(now).buttons & BUTTON_USE != 0;
            if pressed && !down {
                rises.push(now);
            }
            down = pressed;
        }
        let tap = pk.use2_tap.expect("use2 was entered");
        let at: Vec<u128> = rises
            .iter()
            .map(|r| r.duration_since(tap).as_millis())
            .collect();
        assert_eq!(at, vec![0, 500, 1500]);
    }

    /// Once the use tap is down the aim holds: the taken fg42 leaves the wire
    /// and the view must not swing to the next item `nearest_item` finds.
    #[test]
    fn pickup_latches_the_aim_at_the_tap() {
        let mut pk = PickupProbe {
            phase: PickupPhase::Aim,
            idx_fg42: 6,
            ..PickupProbe::default()
        };
        let t0 = Instant::now();
        let near = [(251, 6, [100.0, 0.0, 0.0])];
        let far = [(257, 6, [0.0, 100.0, 0.0])];
        pk.step(t0, &pickup_snap(1, &[], &near));
        pk.enter(t0, PickupPhase::Use1);
        pk.step(t0, &pickup_snap(2, &[], &near));
        let aimed = pk.aim;
        assert_eq!(aimed.map(|a| a.0), Some(0), "yaw 0, at the near fg42");
        let after_tap = t0 + PICKUP_SETTLE;
        pk.step(after_tap, &pickup_snap(3, &[], &far));
        assert_eq!(pk.aim, aimed, "the aim moved after the tap");
    }

    /// A phase that should take something and did not is marked, and a take
    /// inside the owner lockout is recorded rather than marked.
    #[test]
    fn pickup_marks_a_phase_that_took_nothing() {
        let t0 = Instant::now();
        let mut pk = PickupProbe::default();
        for phase in [
            PickupPhase::Touch2,
            PickupPhase::Switch,
            PickupPhase::Use2,
            PickupPhase::Early,
        ] {
            pk.enter(t0, phase);
        }
        pk.on_server_command(t0, &["a".to_string(), "9".to_string()]);
        pk.enter(t0, PickupPhase::Late);
        pk.enter(t0, PickupPhase::Done);
        assert_eq!(
            pk.notes,
            vec![
                "# BROKEN touch2 took nothing",
                "# BROKEN use2 took nothing",
                "# BROKEN late took nothing",
                "# early took a 9",
            ]
        );
    }

    /// A retail client switches to what an `a <index>` names; the byte rides
    /// every cmd until `ps.weapon` reads it, and only then falls back to the
    /// switch phase's own request.
    #[test]
    fn pickup_holds_the_selected_weapon_until_the_playerstate_reads_it() {
        let mut pk = PickupProbe {
            switch_to: Some(10),
            ..PickupProbe::default()
        };
        pk.on_server_command(Instant::now(), &["a".to_string(), "6".to_string()]);
        assert_eq!(pk.weapon_byte(3), Some(6));
        assert_eq!(pk.weapon_byte(6), Some(10));
        assert_eq!(pk.weapon_byte(10), None, "both requests are met");
        assert_eq!(
            escape_ctl("f \"GAME_PICKUP_AMMO\u{14}WEAPON_FG42\""),
            "f \"GAME_PICKUP_AMMO\\x14WEAPON_FG42\""
        );
    }
}
