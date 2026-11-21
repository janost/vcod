//! `--save-hit` and `--probe-target`: the shooter, the target and their fixtures.

use super::combat::{
    FRAG_WEAPON, MissileSample, MissileWatch, PULSE_HOLD, PULSE_MARGIN, PULSE_PERIOD, WeaponSwitch,
    data_secs, weapon_cs_index,
};
use super::fixture::{
    PLAYERSTATE_FIXTURE_DIR, nonzero_pairs, pairs_str, serverinfo_key, tagged_name, vec_str,
    write_fixture_file,
};
use super::*;

/// The target's script. It kills itself rather than waiting to be shot, so the
/// death half of the capture stands whether or not the shooter ever finds a
/// line of sight.
pub(super) const TARGET_KILL_AT: Duration = Duration::from_secs(10);

/// And again every so often: each death is another death in the capture, and
/// another roll of where the two stand, since retail's deathmatch spawn picker
/// puts a respawning player at the point farthest from the other client. Long
/// enough that the shooter's walk across the map is not chasing a target that
/// has already moved again.
const TARGET_KILL_PERIOD: Duration = Duration::from_secs(45);

/// The round-restart pair's first `kill`: a dead team ends an S&D round, and
/// the capture wants the restart early enough that the round timer's own end
/// still fits in the same run.
pub(super) const ROUNDRESTART_KILL_AT: Duration = Duration::from_secs(20);

pub(super) const TARGET_USE_AFTER_DEATH: Duration = Duration::from_secs(3);

pub(super) const TARGET_USE_RETRY: Duration = Duration::from_secs(1);

pub(super) const TARGET_SCORE_PERIOD: Duration = Duration::from_secs(2);

/// The cap on the whole run: past the shooter's approach limit and its firing
/// phases, so the target is still tracing when the shots land.
const TARGET_RUN_LIMIT: Duration = Duration::from_secs(200);

/// `--probe-kill-cmds`: a kill this long after the last, once alive again.
const KILL_CMDS_PERIOD: Duration = Duration::from_secs(8);

/// `--probe-kill-cmds`: no cmd this long after a kill's burst, so the next
/// one reaches the server past the death frame's `ClientEndFrame`.
pub(super) const KILL_CMDS_QUIET: Duration = Duration::from_millis(150);

/// Retail's body queue: the corpse clone lands in one of the eight entities
/// straight past the 64 client slots (docs/research/cod11-combat.md, 5.2).
const BODY_QUEUE: std::ops::Range<u32> = 64..72;

/// One snapshot of everything a hit or a death moves on the victim's side.
struct HitSample {
    elapsed_ms: u128,
    server_time: i32,
    health: i32,
    damage_event: i32,
    damage_count: i32,
    damage_yaw: i32,
    damage_pitch: i32,
    pm_type: i32,
    e_flags: i32,
    dead_view_height: i32,
    /// A float on the wire, unlike `deadViewHeight` next to it.
    view_height_current: f32,
    event_sequence: i32,
    events: [i32; 4],
    event_parms: [i32; 4],
    legs_anim: i32,
    torso_anim: i32,
    weaponstate: i32,
    /// `ps.stats[1]`, the yaw `player_die` writes toward the killer.
    stats1: i32,
    /// Non-zero entries only; both arrays are 64 wide and nearly all zero.
    ammo: Vec<(usize, i16)>,
    clip: Vec<(usize, i16)>,
    velocity: [f32; 3],
    origin: [f32; 3],
    /// `commandTime` and the two anim timers, which run down by each cmd's
    /// msec from the event that set them.
    command_time: i32,
    legs_timer: i32,
    torso_timer: i32,
}

impl HitSample {
    fn take(elapsed_ms: u128, snap: &net::snapshot::Snapshot) -> HitSample {
        let p = wire();
        let f = |n: &str| snap.ps.field_i32(p, n);
        HitSample {
            elapsed_ms,
            server_time: snap.server_time.raw(),
            health: snap.ps.health(),
            damage_event: f("damageEvent"),
            damage_count: f("damageCount"),
            damage_yaw: f("damageYaw"),
            damage_pitch: f("damagePitch"),
            pm_type: f("pm_type"),
            e_flags: f("eFlags"),
            dead_view_height: f("deadViewHeight"),
            view_height_current: snap.ps.field_f32(p, "viewHeightCurrent"),
            event_sequence: f("eventSequence"),
            events: [
                ev_v1(f("events[0]")),
                ev_v1(f("events[1]")),
                ev_v1(f("events[2]")),
                ev_v1(f("events[3]")),
            ],
            event_parms: [
                f("eventParms[0]"),
                f("eventParms[1]"),
                f("eventParms[2]"),
                f("eventParms[3]"),
            ],
            legs_anim: f("legsAnim"),
            torso_anim: f("torsoAnim"),
            weaponstate: f("weaponstate"),
            stats1: snap.ps.arrays.stats[1],
            ammo: nonzero_pairs(&snap.ps.arrays.ammo),
            clip: nonzero_pairs(&snap.ps.arrays.ammoclip),
            velocity: [
                snap.ps.field_f32(p, "velocity[0]"),
                snap.ps.field_f32(p, "velocity[1]"),
                snap.ps.field_f32(p, "velocity[2]"),
            ],
            origin: snap.ps.origin(p),
            command_time: f("commandTime"),
            legs_timer: f("legsTimer"),
            torso_timer: f("torsoTimer"),
        }
    }

    /// Everything but the two clocks and the origin. A run is 20 snapshots a
    /// second for minutes and a standing player moves none of these, so the
    /// trace keeps a sample when this changes and one a second otherwise.
    fn watched(&self) -> Vec<i32> {
        let mut v = vec![
            self.health,
            self.damage_event,
            self.damage_count,
            self.damage_yaw,
            self.damage_pitch,
            self.pm_type,
            self.e_flags,
            self.dead_view_height,
            self.view_height_current.to_bits() as i32,
            self.event_sequence,
            self.legs_anim,
            self.torso_anim,
            self.weaponstate,
            self.stats1,
            self.legs_timer,
            self.torso_timer,
        ];
        v.extend(self.events);
        v.extend(self.event_parms);
        v.extend(self.velocity.iter().map(|f| f.to_bits() as i32));
        v.extend(
            self.ammo
                .iter()
                .flat_map(|&(i, x)| [i as i32, i32::from(x)]),
        );
        v.extend(
            self.clip
                .iter()
                .flat_map(|&(i, x)| [i as i32, i32::from(x)]),
        );
        v
    }

    fn line(&self) -> String {
        format!(
            "!trace ms={} serverTime={} health={} damageEvent={} damageCount={} damageYaw={} \
damagePitch={} pm_type={} eFlags={} deadViewHeight={} viewHeightCurrent={:.1} eventSequence={} \
events={},{},{},{} eventParms={},{},{},{} legsAnim={} torsoAnim={} weaponstate={} stats1={} \
clip={} ammo={} velocity={} origin={} ct={} legsTimer={} torsoTimer={}\n",
            self.elapsed_ms,
            self.server_time,
            self.health,
            self.damage_event,
            self.damage_count,
            self.damage_yaw,
            self.damage_pitch,
            self.pm_type,
            self.e_flags,
            self.dead_view_height,
            self.view_height_current,
            self.event_sequence,
            self.events[0],
            self.events[1],
            self.events[2],
            self.events[3],
            self.event_parms[0],
            self.event_parms[1],
            self.event_parms[2],
            self.event_parms[3],
            self.legs_anim,
            self.torso_anim,
            self.weaponstate,
            self.stats1,
            pairs_str(&self.clip),
            pairs_str(&self.ammo),
            vec_str(self.velocity),
            vec_str(self.origin),
            self.command_time,
            self.legs_timer,
            self.torso_timer,
        )
    }
}

/// How long a settled stretch goes without a trace line.
const TRACE_HEARTBEAT: u128 = 1000;

/// Keeps a trace down to its transitions: a sample survives when its watched
/// fields differ from the last kept one, or when the heartbeat is due.
pub(super) fn keep_sample(last: &mut Option<(Vec<i32>, u128)>, key: Vec<i32>, ms: u128) -> bool {
    match last {
        Some((k, t)) if *k == key && ms.saturating_sub(*t) < TRACE_HEARTBEAT => false,
        _ => {
            *last = Some((key, ms));
            true
        }
    }
}

/// Corpse edges in the retail body queue, as `(ms, entity, clientNum, appeared)`.
/// `live` keeps each corpse's clientNum and eFlags; an eFlags change is
/// printed with the snapshot's serverTime, which is how the 250 ms think
/// that clears 0x800 is timed (combat doc 5.3).
fn corpse_edges(
    snap: &net::snapshot::Snapshot,
    live: &mut BTreeMap<u32, (i32, i32)>,
    elapsed_ms: u128,
    out: &mut Vec<(u128, u32, i32, bool)>,
) {
    let p = wire();
    let now: BTreeMap<u32, (i32, i32)> = BODY_QUEUE
        .filter_map(|n| {
            let e = snap.entities.get(&n)?;
            (e.field_i32(p, "eType") == crate::entities::ET_CORPSE)
                .then_some((n, (e.field_i32(p, "clientNum"), e.field_i32(p, "eFlags"))))
        })
        .collect();
    for (&n, &(cn, flags)) in &now {
        match live.get(&n) {
            None => {
                println!(
                    "  corpse {n} appeared (clientNum {cn}) at +{elapsed_ms}ms st={} eFlags=0x{flags:x}",
                    snap.server_time
                );
                out.push((elapsed_ms, n, cn, true));
            }
            Some(&(_, was)) if was != flags => println!(
                "  corpse {n} eFlags 0x{was:x} -> 0x{flags:x} at +{elapsed_ms}ms st={}",
                snap.server_time
            ),
            Some(_) => {}
        }
    }
    for (&n, &(cn, _)) in live.iter() {
        if !now.contains_key(&n) {
            println!("  corpse {n} vanished (clientNum {cn}) at +{elapsed_ms}ms");
            out.push((elapsed_ms, n, cn, false));
        }
    }
    *live = now;
}

/// Stands still and records what the server does to it: `kill` at 10 s, the
/// respawn press 3 s after each death, `score` every 2 s.
#[derive(Default)]
pub(super) struct TargetProbe {
    trace: Vec<HitSample>,
    last_kept: Option<(Vec<i32>, u128)>,
    /// `(ms, event, parm, entity)` for every death, pain and obituary drained.
    events: Vec<(u128, i32, i32, u32)>,
    /// `(ms, victim, attacker, parm)` for every EV_OBITUARY drained.
    obituaries: Vec<(u128, u32, i32, i32)>,
    /// `(ms, entity, clientNum, appeared)` per corpse edge.
    corpses: Vec<(u128, u32, i32, bool)>,
    live_corpses: BTreeMap<u32, (i32, i32)>,
    /// Every `b` scoreboard line, with its time.
    pub(super) scoreboards: Vec<(u128, String)>,
    /// Reliable commands the loop sends once its snapshot borrow is done.
    pub(super) pending: Vec<String>,
    started: Option<Instant>,
    killed_at: Option<Instant>,
    died_at: Option<Instant>,
    used_at: Option<Instant>,
    last_score_at: Option<Instant>,
    tracker: vcod_common::net::events::EventTracker,
    traced: Option<u32>,
    /// Held for the one frame the respawn press falls on.
    press_use: bool,
    deaths: u32,
    done: bool,
    /// How long after the spawn the first `kill` goes out.
    kill_at: Duration,
    /// `--probe-kill-cmds`: the cmd counts each kill takes in turn.
    pub(super) bursts: Vec<usize>,
    /// The count the loop sends behind the `kill` it is about to send.
    pub(super) burst: Option<usize>,
    /// `(ms, cmds)` per kill in burst mode.
    kills: Vec<(u128, usize)>,
}

impl TargetProbe {
    /// The first `kill`'s delay is the parameter: the hit pair waits out the
    /// shooter's approach, the round-restart pair ends the round sooner so two
    /// rounds fit in one run.
    pub(super) fn new(kill_at: Duration) -> Self {
        TargetProbe {
            kill_at,
            ..Default::default()
        }
    }

    pub(super) fn running(&self) -> bool {
        !self.done
    }

    /// Standing still, with the use bit down on the frame the press falls on.
    pub(super) fn cmd(&self) -> net::msg::UserCmd {
        net::msg::UserCmd {
            buttons: if self.press_use { BUTTON_USE } else { 0 },
            ..net::msg::NULL_USERCMD
        }
    }

    pub(super) fn elapsed_ms(&self, now: Instant) -> u128 {
        self.started
            .map_or(0, |t| now.duration_since(t).as_millis())
    }

    /// Feeds the newest snapshot in. Returns true once the run is over and the
    /// fixture is ready to write.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        use crate::fx::registry::{EV_CROUCH_PAIN, EV_DEATH, EV_OBITUARY, EV_PAIN};
        let p = wire();
        let started = *self.started.get_or_insert(now);
        let elapsed = now.duration_since(started);
        let ms = elapsed.as_millis();
        let pm_type = snap.ps.field_i32(p, "pm_type");

        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            let s = HitSample::take(ms, snap);
            if keep_sample(&mut self.last_kept, s.watched(), ms) {
                println!("  {}", s.line().trim_end());
                self.trace.push(s);
            }
            for ev in drain_v1(&mut self.tracker, snap, p) {
                print_bullet_hit(ms, &ev);
                if ev.event == EV_OBITUARY {
                    println!(
                        "  obituary +{ms}ms victim {} attacker {} parm {}",
                        ev.other_entity_num, ev.attacker_entity_num, ev.parm
                    );
                    self.obituaries.push((
                        ms,
                        ev.other_entity_num,
                        ev.attacker_entity_num,
                        ev.parm,
                    ));
                }
                if matches!(ev.event, EV_DEATH | EV_PAIN | EV_CROUCH_PAIN | EV_OBITUARY) {
                    println!(
                        "  event +{ms}ms ev {} parm {} entity {}",
                        ev.event, ev.parm, ev.entity_num
                    );
                    self.events.push((ms, ev.event, ev.parm, ev.entity_num));
                }
            }
            corpse_edges(snap, &mut self.live_corpses, ms, &mut self.corpses);
        }

        // The suicide: it is what makes the death half of the capture
        // independent of whether the shooter ever finds a line of sight.
        let kill_due = match self.killed_at {
            None => elapsed >= self.kill_at,
            Some(t) if !self.bursts.is_empty() => {
                self.died_at.is_none()
                    && pm_type == PM_NORMAL
                    && now.duration_since(t) >= KILL_CMDS_PERIOD
            }
            Some(t) => self.died_at.is_none() && now.duration_since(t) >= TARGET_KILL_PERIOD,
        };
        if kill_due {
            println!("TARGET: sending kill at +{ms}ms");
            self.pending.push("kill".to_string());
            self.killed_at = Some(now);
            if !self.bursts.is_empty() {
                let n = self.bursts[self.kills.len() % self.bursts.len()];
                println!("TARGET: {n} cmds behind the kill");
                self.burst = Some(n);
                self.kills.push((ms, n));
            }
        }
        if pm_type != PM_NORMAL {
            if self.died_at.is_none() {
                self.deaths += 1;
                println!(
                    "TARGET: pm_type {pm_type} at +{ms}ms, death {}",
                    self.deaths
                );
                self.died_at = Some(now);
                self.used_at = None;
            }
        } else if let Some(t) = self.died_at.take() {
            println!(
                "TARGET: alive again at +{ms}ms, {} ms dead, health {}",
                now.duration_since(t).as_millis(),
                snap.ps.health()
            );
            self.used_at = None;
        }

        // The press, retried once a second: a press the script does not take
        // leaves the target dead, and a dead target measures nothing.
        self.press_use = false;
        if let Some(dead) = self.died_at {
            let due = now.duration_since(dead) >= TARGET_USE_AFTER_DEATH;
            let again = self
                .used_at
                .is_none_or(|t| now.duration_since(t) >= TARGET_USE_RETRY);
            if due && again {
                println!("TARGET: use press at +{ms}ms");
                self.used_at = Some(now);
                self.press_use = true;
            }
        }

        // A burst run sends no `score`: a bare one opens retail's 800 ms
        // flood window, which drops a `kill` that falls inside it.
        if self.bursts.is_empty()
            && self
                .last_score_at
                .is_none_or(|t| now.duration_since(t) >= TARGET_SCORE_PERIOD)
        {
            self.last_score_at = Some(now);
            self.pending.push("score".to_string());
        }

        self.done = elapsed >= TARGET_RUN_LIMIT;
        self.done
    }
}

/// How long the shooter walks toward the target before it gives up and shoots
/// from where it stands.
const APPROACH_LIMIT: Duration = Duration::from_secs(150);

const SINGLE_SHOT_HOLD: Duration = Duration::from_millis(2000);

const BURST_HOLD: Duration = Duration::from_millis(3000);

/// The caps on `Finish`: taps, and the wall clock behind them.
const FINISH_TAPS: u32 = 30;

const FINISH_LIMIT: Duration = Duration::from_secs(15);

const WATCH_HOLD: Duration = Duration::from_secs(25);

/// The trigger held before the first throw, and the shorter hold of the second
/// one, which goes out barely cooked at the ground.
const COOK_HOLDS: [Duration; 2] = [Duration::from_millis(1000), Duration::from_millis(150)];

/// How long a cook waits for the switch to the frag before it throws anyway,
/// so a refused switch costs the throw and not the run.
const COOK_LIMIT: Duration = Duration::from_secs(8);

/// Long enough for the release and the frames the missile spawns on.
const THROW_HOLD: Duration = Duration::from_millis(600);

/// A grenade's fuse is about 4 s from the pull, so the watch has to outlast
/// the flight, the bounces and the explode.
const GRENADE_WATCH_HOLD: Duration = Duration::from_secs(8);

const GRENADE_THROWS: u32 = 2;

/// How far into the cook `--probe-grenade-death` kills itself, which is what
/// puts the death drop's missile on the wire.
const GRENADE_DEATH_AT: Duration = Duration::from_millis(500);

/// A trace that gets this far is clear enough to shoot along.
const LOS_FRACTION: f32 = 0.99;

/// The events the shooter records with their entity, which is what says
/// whether retail put one on a temp entity or on the missile's own ring: the
/// pullback, the melee swing and its hit or miss, the grenade's bounces and
/// every explode (docs/research/cod11-combat.md, sections 2.5, 12 and 13).
const TRACKED_EVENTS: [i32; 9] = [
    crate::fx::registry::EV_PULLBACK_WEAPON,
    crate::fx::registry::EV_MELEE_SWIPE,
    crate::fx::registry::EV_FIRE_MELEE,
    crate::fx::registry::EV_MELEE_HIT,
    crate::fx::registry::EV_MELEE_MISS,
    crate::fx::registry::EV_GRENADE_BOUNCE,
    crate::fx::registry::EV_GRENADE_EXPLODE,
    crate::fx::registry::EV_ROCKET_EXPLODE,
    crate::fx::registry::EV_ROCKET_EXPLODE_NOMARKS,
];

/// Prints a bullet impact (173 to 176) with the fields that say who it was
/// for: which of the pair a client is sent is the single-client rule
/// (docs/research/cod11-events-and-fx.md, section 2). Stdout only; no
/// fixture carries these.
fn print_bullet_hit(ms: u128, ev: &vcod_common::net::events::GameEvent) {
    use crate::fx::registry::{EV_BULLET_HIT_CLIENT_LARGE, EV_BULLET_HIT_SMALL};
    if (EV_BULLET_HIT_SMALL..=EV_BULLET_HIT_CLIENT_LARGE).contains(&ev.event) {
        println!(
            "  bullethit +{ms}ms ev {} parm {} surfType {} other {} clientNum {} entity {} pos {:?}",
            ev.event,
            ev.parm,
            ev.surf_type,
            ev.other_entity_num,
            ev.client_num,
            ev.entity_num,
            ev.pos
        );
    }
}

/// How close the approach walks before it shoots. A clear trace across a whole
/// map is still a shot the spread throws off a player-sized target: one
/// capture spent its taps that way from 3200 units out and another from 1950,
/// and neither moved the target's `damageEvent`. The approach limit is what
/// gets the trigger pulled when the walk cannot close this.
pub(super) const ENGAGE_RANGE: f32 = 700.0;

/// How close a `--probe-sweep` walks first. Its taps are a height ruler on
/// the target, and the hip cone at 350 units is a nine-unit blur that reads
/// nothing; the retail sweep that pinned the head line stood at 36 to 118.
pub(super) const SWEEP_RANGE: f32 = 120.0;

pub(super) use vcod_common::net::event_ids::{EV_BULLET_HIT_LARGE, EV_BULLET_HIT_SMALL};

/// Heights up the target's body the line-of-sight trace tries. Only the eye is
/// aimed at, but a target behind a low wall is one a shot can still reach.
const LOS_HEIGHTS: [f32; 3] = [16.0, 40.0, EYE_HEIGHT];

/// How far ahead the approach looks for a walk it can actually take, and the
/// headings it tries either side of the bearing to the target.
pub(super) const STEER_AHEAD: f32 = 200.0;

const STEER_OFFSETS: [f32; 11] = [
    0.0, 25.0, -25.0, 50.0, -50.0, 80.0, -80.0, 115.0, -115.0, 150.0, -150.0,
];

/// How long a chosen heading is held. Re-picking every frame made the walk
/// oscillate between two headings and stand still between them.
const STEER_HOLD: Duration = Duration::from_millis(1200);

/// Ground the walk has to cover inside [`STEER_HOLD`] to count as moving.
const STEER_PROGRESS: f32 = 40.0;

/// A local navigator for the approach: the heading nearest the bearing to the
/// next waypoint that a player-sized box can sweep along, held for a while so
/// the walk commits to it, and swapped for a random one when the walk wedges.
/// Steering only on the bearing, and turning only once already stuck,
/// wandered mp_carentan for the whole approach limit without closing.
#[derive(Default)]
pub(super) struct Steer {
    pub(super) heading: Option<f32>,
    chosen_at: Option<Instant>,
    /// Where the current heading was chosen, so a wedged walk is detectable.
    from: [f32; 3],
    /// LCG state for the unwedging turn; a fixed 45 degrees walked the
    /// same loop over and over.
    rng: u32,
}

impl Steer {
    /// Whether a player-sized box can sweep from `a` to `b`, both given as
    /// floor positions. The sweep starts a step up, so a curb is not a wall.
    fn sweep(w: &vcod_common::collision::CollisionWorld, a: [f32; 3], b: [f32; 3]) -> bool {
        Self::sweep_box(w, a, b, vcod_common::pmove::HEIGHT_STAND)
    }

    /// The same with the box height given, so a climb can be tested with the
    /// clearance a stairwell has rather than a standing player's.
    fn sweep_box(
        w: &vcod_common::collision::CollisionWorld,
        a: [f32; 3],
        b: [f32; 3],
        height: f32,
    ) -> bool {
        use vcod_common::pmove::{HALF_WIDTH, STEPSIZE};
        let mins = glam::Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
        let maxs = glam::Vec3::new(HALF_WIDTH, HALF_WIDTH, height);
        let from = glam::Vec3::new(a[0], a[1], a[2] + STEPSIZE);
        let to = glam::Vec3::new(b[0], b[1], b[2] + STEPSIZE);
        w.box_trace(from, to, mins, maxs).fraction >= 1.0
    }

    /// Whether that sweep is clear for `reach` units along `deg`.
    fn walkable(
        w: &vcod_common::collision::CollisionWorld,
        origin: [f32; 3],
        deg: f32,
        reach: f32,
    ) -> bool {
        let a = deg.to_radians();
        let to = [
            origin[0] + a.cos() * reach,
            origin[1] + a.sin() * reach,
            origin[2],
        ];
        Self::sweep(w, origin, to)
    }

    fn next_random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 16) as f32 * 360.0 / 65536.0
    }

    /// The heading to walk this frame.
    pub(super) fn heading(
        &mut self,
        now: Instant,
        world: Option<&vcod_common::collision::CollisionWorld>,
        origin: [f32; 3],
        bearing_deg: f32,
        reach: f32,
    ) -> f32 {
        let Some(w) = world else {
            return bearing_deg;
        };
        let moved = dist(
            [origin[0], origin[1], 0.0],
            [self.from[0], self.from[1], 0.0],
        );
        if let Some(h) = self.heading {
            // The hold runs whether or not the heading still looks walkable:
            // re-picking inside it resets the progress window every frame, and
            // then a wedged walk never reads as wedged.
            if self
                .chosen_at
                .is_some_and(|t| now.duration_since(t) < STEER_HOLD)
            {
                return h;
            }
            if moved >= STEER_PROGRESS && Self::walkable(w, origin, h, reach) {
                self.chosen_at = Some(now);
                self.from = origin;
                return h;
            }
        }
        // A walk that covered no ground took a heading the sweep called clear,
        // so the sweep is not what will get it off this spot: turn at random.
        let pick = if self.heading.is_some() && moved < STEER_PROGRESS {
            let h = self.next_random();
            println!("HIT: wedged after {moved:.0}u, turning to {h:.0} deg");
            h
        } else {
            STEER_OFFSETS
                .iter()
                .map(|off| bearing_deg + off)
                .find(|&deg| Self::walkable(w, origin, deg, reach))
                .unwrap_or_else(|| self.next_random())
        };
        self.heading = Some(pick);
        self.chosen_at = Some(now);
        self.from = origin;
        pick
    }
}

/// The pitch offsets a `--probe-sweep` run walks, degrees, positive down, one
/// tap each, applied to the aim at the target's eye. Fine around the eye where
/// the head, neck and torso boundaries sit and coarse toward the feet. Pair
/// each `!trace` line's `pitchOffset` and `range` with the `sHitLoc` the
/// server's own `games_mp.log` `D;` record carries for that shot, and the
/// height the bullet crossed the target at is trigonometry.
pub(super) const SWEEP_PITCH_OFFSETS: &[f32] = &[
    -12.0, -10.0, -8.0, -6.0, -4.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0,
    12.0, 15.0, 18.0, 21.0, 24.0, 28.0, 32.0, 36.0,
];

/// A sweep that cannot spend its table in this long gives up and watches.
const SWEEP_LIMIT: Duration = Duration::from_secs(240);

/// The shooter's phases, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HitPhase {
    /// Walk toward the other player, steered by [`Steer`], until the
    /// eye-to-eye trace is clear or [`APPROACH_LIMIT`] passes.
    Approach,
    /// One tap, aimed at the target's eye.
    SingleShot,
    /// `--probe-sweep` only: one tap per entry of [`SWEEP_PITCH_OFFSETS`],
    /// each offset spent only on a tap that had a live target to hit.
    Sweep,
    /// Five taps at the weapon's period.
    Burst,
    /// Tap until the target's entity stops being a player, or a cap hits.
    Finish,
    /// Grenade only: switch to the frag and hold the trigger down.
    Cook,
    /// Grenade only: the release, and the frames the missile spawns on.
    Throw,
    /// Idle, watching the corpse and the respawn.
    Watch,
    Done,
}

impl HitPhase {
    fn label(self) -> &'static str {
        match self {
            HitPhase::Approach => "approach",
            HitPhase::SingleShot => "single_shot",
            HitPhase::Sweep => "sweep",
            HitPhase::Burst => "burst",
            HitPhase::Finish => "finish",
            HitPhase::Cook => "cook",
            HitPhase::Throw => "throw",
            HitPhase::Watch => "watch",
            HitPhase::Done => "done",
        }
    }

    /// Taps the phase spends.
    fn taps(self) -> u32 {
        match self {
            HitPhase::SingleShot => 1,
            HitPhase::Sweep => SWEEP_PITCH_OFFSETS.len() as u32,
            HitPhase::Burst => 5,
            HitPhase::Finish => FINISH_TAPS,
            _ => 0,
        }
    }

    /// The held input the phase runs on.
    fn base(self) -> net::msg::UserCmd {
        match self {
            HitPhase::Approach => net::msg::UserCmd {
                forward: 127,
                ..net::msg::NULL_USERCMD
            },
            _ => net::msg::NULL_USERCMD,
        }
    }

    /// What the phase is after `in_phase` spent in it.
    fn advance(self, in_phase: Duration, cx: &PhaseCtx) -> HitPhase {
        use crate::entities::ET_PLAYER;
        let engaged = cx.ready || in_phase >= APPROACH_LIMIT;
        match self {
            // The round-restart shooter walks up and then only records: it
            // fires nothing, and its run ends on the probe's clock.
            HitPhase::Approach if engaged && cx.script == ShooterScript::RoundRestart => {
                HitPhase::Watch
            }
            HitPhase::Approach if engaged && cx.script == ShooterScript::Sweep => HitPhase::Sweep,
            HitPhase::Approach if engaged && cx.script.throws_grenades() => HitPhase::Cook,
            HitPhase::Approach if engaged => HitPhase::SingleShot,
            HitPhase::Sweep
                if cx.taps >= SWEEP_PITCH_OFFSETS.len() as u32 || in_phase >= SWEEP_LIMIT =>
            {
                HitPhase::Watch
            }
            HitPhase::SingleShot if in_phase >= SINGLE_SHOT_HOLD => HitPhase::Burst,
            HitPhase::Burst if in_phase >= BURST_HOLD => HitPhase::Finish,
            HitPhase::Finish
                if matches!(cx.target_etype, Some(t) if t != ET_PLAYER)
                    || cx.taps >= FINISH_TAPS
                    || in_phase >= FINISH_LIMIT =>
            {
                HitPhase::Watch
            }
            // The cook gives up rather than hanging when the switch is refused.
            HitPhase::Cook if cx.cook_done || in_phase >= COOK_LIMIT => HitPhase::Throw,
            HitPhase::Throw if in_phase >= THROW_HOLD => HitPhase::Watch,
            // A grenade run watches its missile out and then throws a second,
            // uncooked one at the ground; a bullet run just watches.
            HitPhase::Watch if cx.script.throws_grenades() && in_phase >= GRENADE_WATCH_HOLD => {
                if cx.throws >= GRENADE_THROWS {
                    HitPhase::Done
                } else {
                    HitPhase::Cook
                }
            }
            HitPhase::Watch
                if !cx.script.throws_grenades()
                    && cx.script != ShooterScript::RoundRestart
                    && in_phase >= WATCH_HOLD =>
            {
                HitPhase::Done
            }
            p => p,
        }
    }
}

/// What [`HitPhase::advance`] needs beyond the phase's own clock.
struct PhaseCtx {
    /// The target is in the PVS, the trace to it is clear and it is inside the
    /// script's engagement range.
    ready: bool,
    /// The aimed-at entity's eType; `None` when the server sent none, which is
    /// not a reason to stop shooting.
    target_etype: Option<i32>,
    taps: u32,
    script: ShooterScript,
    /// The trigger has been held down long enough for this throw.
    cook_done: bool,
    /// Throws released so far, which is what ends a grenade run.
    throws: u32,
}

/// One snapshot's worth of the shooter's side, with what it could see of the
/// target at the time.
struct ShooterSample {
    phase: &'static str,
    elapsed_ms: u128,
    server_time: i32,
    buttons: u8,
    wbuttons: u8,
    weaponstate: i32,
    weap_anim: i32,
    legs_anim: i32,
    torso_anim: i32,
    event_sequence: i32,
    events: [i32; 4],
    event_parms: [i32; 4],
    clip: Vec<(usize, i16)>,
    ammo: Vec<(usize, i16)>,
    los: bool,
    range: f32,
    /// The target was in this snapshot; when it was not, the three target
    /// columns are the last place it was seen.
    target_seen: bool,
    /// The server's own view angles, degrees: what the shot was aimed along.
    viewangles: [f32; 3],
    /// The sweep's pitch offset on this cmd, degrees, positive down; 0 outside
    /// a `--probe-sweep` run.
    pitch_offset: f32,
    target_num: i32,
    target_etype: i32,
    target_origin: [f32; 3],
    origin: [f32; 3],
    /// What is left of the cook, ms, and the weapon machine's own delay.
    grenade_time_left: i32,
    weapon_delay: i32,
    /// Every missile on the wire in this snapshot.
    missiles: Vec<MissileSample>,
}

impl ShooterSample {
    fn watched(&self) -> Vec<i32> {
        let mut v = vec![
            i32::from(self.buttons),
            i32::from(self.wbuttons),
            self.weaponstate,
            self.weap_anim,
            self.legs_anim,
            self.torso_anim,
            self.event_sequence,
            self.los as i32,
            self.target_seen as i32,
            self.target_num,
            self.target_etype,
        ];
        v.extend(self.events);
        v.extend(self.event_parms);
        v.extend(
            self.clip
                .iter()
                .flat_map(|&(i, x)| [i as i32, i32::from(x)]),
        );
        v.extend(
            self.ammo
                .iter()
                .flat_map(|&(i, x)| [i as i32, i32::from(x)]),
        );
        v
    }

    fn line(&self) -> String {
        format!(
            "!trace phase={} ms={} serverTime={} buttons={} wbuttons={} weaponstate={} weapAnim={} \
legsAnim={} torsoAnim={} eventSequence={} events={},{},{},{} eventParms={},{},{},{} clip={} \
ammo={} los={} range={:.0} seen={} viewangles={} pitchOffset={:.1} target={} \
target_etype={} target_origin={} origin={} grenadeTimeLeft={} weaponDelay={}\n",
            self.phase,
            self.elapsed_ms,
            self.server_time,
            self.buttons,
            self.wbuttons,
            self.weaponstate,
            self.weap_anim,
            self.legs_anim,
            self.torso_anim,
            self.event_sequence,
            self.events[0],
            self.events[1],
            self.events[2],
            self.events[3],
            self.event_parms[0],
            self.event_parms[1],
            self.event_parms[2],
            self.event_parms[3],
            pairs_str(&self.clip),
            pairs_str(&self.ammo),
            self.los as i32,
            self.range,
            self.target_seen as i32,
            vec_str(self.viewangles),
            self.pitch_offset,
            self.target_num,
            self.target_etype,
            vec_str(self.target_origin),
            vec_str(self.origin),
            self.grenade_time_left,
            self.weapon_delay,
        )
    }
}

/// Walks up to the other player, shoots it and watches what comes back. The
/// death half of the capture is the target's own `kill`, so a run that never
/// finds a line of sight costs the hits and nothing else.
pub(super) struct HitProbe {
    phase: HitPhase,
    started: Option<Instant>,
    /// When the current phase started, so its taps and its clock are its own.
    phase_started: Option<Instant>,
    trace: Vec<ShooterSample>,
    last_kept: Option<(Vec<i32>, u128)>,
    obituaries: Vec<(u128, u32, i32, i32)>,
    corpses: Vec<(u128, u32, i32, bool)>,
    live_corpses: BTreeMap<u32, (i32, i32)>,
    tracker: vcod_common::net::events::EventTracker,
    traced: Option<u32>,
    steer: Steer,
    /// The map's collision, for the eye-to-eye trace. `None` without game data
    /// or when the map did not resolve; the approach then runs to its limit.
    pub(super) world: Option<Box<vcod_common::collision::CollisionWorld>>,
    /// The two wire words `cmd` puts in the usercmd. Absolute, not relative to
    /// the spawn heading: what the retail server's `viewangles` reads back is
    /// what the usercmd carried, measured against the bearing to the target.
    aim: Option<(i32, i32)>,
    period: Duration,
    weapon_read: bool,
    los: bool,
    los_ever: bool,
    /// Distance to the target, and whether it is inside [`ENGAGE_RANGE`].
    range: f32,
    in_range: bool,
    /// The target was in this snapshot, rather than remembered from an older one.
    target_seen: bool,
    /// The last target seen: `(entity, eType, origin)`.
    target: Option<(u32, i32, [f32; 3])>,
    /// This probe's own clientNum, for reading its kills out of the obituaries.
    me: i32,
    /// The range the first shot went out at, and whether the trace to the
    /// target was clear then: the approach limit fires the trigger either way.
    engaged_at: Option<f32>,
    engaged_los: bool,
    /// An obituary named this probe as the attacker.
    killed: bool,
    /// How many offsets a `--probe-sweep` run has spent, which is also the
    /// index of the one it is firing, and the tap period it was spent in.
    sweep_index: usize,
    sweep_period: Option<u128>,
    /// Which phases the shooter runs and what its fixture is named.
    script: ShooterScript,
    /// The frag's configstring 7 index, the byte the cook's switch asks for.
    frag_index: u8,
    /// Set once this Cook's `ps.weapon` first read the frag; the trigger goes
    /// down from then, so the cook is not spent on the switch animation.
    cook_armed: Option<Instant>,
    switch: WeaponSwitch,
    /// Throws released, which is what ends a grenade run.
    throws: u32,
    /// `--probe-grenade-death` has sent its `kill`.
    killed_self: bool,
    /// Reliable commands the loop sends once its snapshot borrow is done.
    pub(super) pending: Vec<String>,
    /// Held for the one frame a respawn press falls on.
    press_use: bool,
    used_at: Option<Instant>,
    /// `(ms, event, parm, entity)` for every melee, grenade and death event.
    events: Vec<(u128, i32, i32, u32)>,
    missiles: MissileWatch,
}

impl Default for HitProbe {
    fn default() -> Self {
        HitProbe {
            phase: HitPhase::Approach,
            started: None,
            phase_started: None,
            trace: Vec::new(),
            last_kept: None,
            obituaries: Vec::new(),
            corpses: Vec::new(),
            live_corpses: BTreeMap::new(),
            tracker: vcod_common::net::events::EventTracker::new(),
            traced: None,
            steer: Steer::default(),
            world: None,
            aim: None,
            period: PULSE_PERIOD,
            weapon_read: false,
            los: false,
            los_ever: false,
            range: f32::INFINITY,
            in_range: false,
            target_seen: false,
            target: None,
            me: -1,
            engaged_at: None,
            engaged_los: false,
            killed: false,
            sweep_index: 0,
            sweep_period: None,
            script: ShooterScript::Hit,
            frag_index: 0,
            cook_armed: None,
            switch: WeaponSwitch::default(),
            throws: 0,
            killed_self: false,
            pending: Vec::new(),
            press_use: false,
            used_at: None,
            events: Vec::new(),
            missiles: MissileWatch::default(),
        }
    }
}

impl HitProbe {
    pub(super) fn new(script: ShooterScript) -> Self {
        HitProbe {
            script,
            ..Self::default()
        }
    }

    pub(super) fn running(&self) -> bool {
        self.phase != HitPhase::Done
    }

    /// The frag's `cmd.weapon` byte, out of configstring 7 at gamestate.
    pub(super) fn use_configstrings(&mut self, configstrings: &[String]) {
        self.frag_index = weapon_cs_index(configstrings, FRAG_WEAPON).map_or(0, |(i, _)| i);
        if self.script.throws_grenades() {
            println!(
                "HIT: configstring 7 index {} for {FRAG_WEAPON}",
                self.frag_index
            );
        }
    }

    /// The `cmd.weapon` this cmd carries, `None` to follow `ps.weapon`. The
    /// cook is the one phase that switches; see [`WeaponSwitch`] for why the
    /// byte is held rather than sent once.
    pub(super) fn weapon_override(&mut self, ps_weapon: u8) -> Option<u8> {
        if self.phase == HitPhase::Cook && self.switch.ask(self.frag_index) {
            println!("HIT: cook, asking for weapon {}", self.frag_index);
        }
        self.switch.byte(ps_weapon)
    }

    /// How long this throw's cook holds the trigger; the second one is short,
    /// so it goes out with most of its fuse left.
    fn cook_hold(&self) -> Duration {
        COOK_HOLDS[(self.throws as usize).min(COOK_HOLDS.len() - 1)]
    }

    fn elapsed_ms(&self, now: Instant) -> u128 {
        self.started
            .map_or(0, |t| now.duration_since(t).as_millis())
    }

    /// Loads the map's collision once the gamestate names the map; without it
    /// there is no line-of-sight test and the approach runs to its limit.
    pub(super) fn load_map(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, map: &str) {
        let Some(fs) = fs else {
            println!("HIT: no game data, the approach cannot test line of sight");
            return;
        };
        let Some(path) = fs.resolve_map(map) else {
            println!("HIT: map {map:?} did not resolve, the approach runs blind");
            return;
        };
        let Some(data) = fs.read(&path) else {
            println!("HIT: cannot read {path}, the approach runs blind");
            return;
        };
        match vcod_common::bsp::parse(&data) {
            Ok(bsp) => {
                let tris = vcod_common::props::collision_tris(fs, &bsp.entities);
                let world = vcod_common::collision::CollisionWorld::build(&bsp, &tris);
                println!(
                    "HIT: {path} loaded, {} brushes and {} tris for the line-of-sight trace",
                    world.brushes.len(),
                    world.tris.len(),
                );
                self.world = Some(Box::new(world));
            }
            Err(e) => println!("HIT: cannot parse {path} ({e:#}), the approach runs blind"),
        }
    }

    /// Taps at the weapon file's `fireTime`, the way the combat capture does.
    pub(super) fn use_weapon(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, name: &str) {
        if self.weapon_read || name.is_empty() {
            return;
        }
        self.weapon_read = true;
        let Some(def) = fs.and_then(|fs| vcod_common::weapon::load(fs, name).ok()) else {
            return;
        };
        let Some(fire) = data_secs(def.fire_time) else {
            return;
        };
        self.period = (fire + PULSE_MARGIN).max(PULSE_PERIOD);
        println!(
            "HIT: {name} fireTime {:.3}s, taps every {:?}",
            def.fire_time, self.period
        );
    }

    /// The offset the sweep's current tap carries, degrees, positive down.
    fn pitch_offset(&self) -> f32 {
        if self.phase != HitPhase::Sweep {
            return 0.0;
        }
        SWEEP_PITCH_OFFSETS
            .get(self.sweep_index)
            .copied()
            .unwrap_or(0.0)
    }

    /// The target is in the PVS, the trace to it is clear and it is close
    /// enough that the spread does not decide where the bullet lands.
    fn shootable(&self) -> bool {
        self.los && self.in_range && self.target_seen
    }

    /// The phase's held input with the aim on it, and the trigger tapped for
    /// [`PULSE_HOLD`] out of every period until the phase's taps are spent.
    /// A sweep taps only when it has something to hit, so an offset is not
    /// spent on a dead or hidden target, and it ends on its table rather than
    /// on the clock.
    pub(super) fn cmd(&self, now: Instant) -> net::msg::UserCmd {
        use net::msg::BUTTON_MELEE;
        let mut cmd = self.phase.base();
        if let Some((yaw, pitch)) = self.aim {
            cmd.angles[1] = yaw;
            cmd.angles[0] = (pitch + deg_to_short(self.pitch_offset())) & 0xffff;
        }
        // A dead shooter presses use to respawn: `--probe-grenade-death` kills
        // itself, and its second throw needs a body to throw from.
        if self.press_use {
            cmd.buttons |= BUTTON_USE;
        }
        // The cook is a held trigger, not a tap, and it starts once the switch
        // to the frag has actually landed.
        if self.phase == HitPhase::Cook {
            if self.cook_armed.is_some() {
                cmd.buttons |= BUTTON_ATTACK;
            }
            return cmd;
        }
        let Some(t) = self.phase_started else {
            return cmd;
        };
        let ms = now.duration_since(t).as_millis();
        let period = self.period.as_millis();
        let sweeping = self.phase == HitPhase::Sweep;
        let within = sweeping || ms / period < u128::from(self.phase.taps());
        let allowed = !sweeping || self.shootable();
        if within && allowed && ms % period < PULSE_HOLD.as_millis() {
            cmd.buttons |= if self.script == ShooterScript::Melee {
                BUTTON_MELEE
            } else {
                BUTTON_ATTACK
            };
        }
        cmd
    }

    /// Feeds the newest snapshot in. Returns true once the last phase is done.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        use crate::fx::registry::EV_OBITUARY;
        let p = wire();
        let started = *self.started.get_or_insert(now);
        let phase_started = *self.phase_started.get_or_insert(now);
        let ms = now.duration_since(started).as_millis();
        let in_phase = now.duration_since(phase_started);

        let me = snap.ps.field_i32(p, "clientNum") as u32;
        self.me = me as i32;
        let eye = {
            let o = snap.ps.origin(p);
            // A float field: read as an i32 it is the bit pattern, and an eye
            // 1.1e9 units up aims every shot at the floor.
            let h = snap.ps.field_f32(p, "viewHeightCurrent");
            [o[0], o[1], o[2] + if h > 0.0 { h } else { EYE_HEIGHT }]
        };
        // The nearest other player; when it stops being one, the same entity
        // slot, so the phase machine can see what it turned into.
        let prev = self.target;
        let seen = snap
            .entities
            .iter()
            .filter(|&(&n, e)| n != me && e.field_i32(p, "eType") == crate::entities::ET_PLAYER)
            .map(|(&n, e)| (n, e.field_i32(p, "eType"), e.origin(p)))
            .min_by(|a, b| {
                dist(a.2, eye)
                    .partial_cmp(&dist(b.2, eye))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .or_else(|| {
                let (n, _, last) = prev?;
                let e = snap.entities.get(&n)?;
                let o = e.origin(p);
                // A slot that stopped being a player can arrive with a zeroed
                // origin; the aim then keeps where it last saw one.
                Some((
                    n,
                    e.field_i32(p, "eType"),
                    if o == [0.0; 3] { last } else { o },
                ))
            });
        // A target out of the PVS is one the walk still heads for and the
        // trigger does not: the last place it stood is a heading, not a shot.
        self.target_seen = seen.is_some();
        self.target = seen.or(prev);

        if let Some((_, _, o)) = self.target {
            let target_eye = [o[0], o[1], o[2] + EYE_HEIGHT];
            let throwing = matches!(self.phase, HitPhase::Cook | HitPhase::Throw);
            // A grenade is thrown at the target's feet, not its eye; the
            // second one goes at the ground off to the side instead, which is
            // the wall-and-floor bounce the first throw does not show.
            let (yaw, pitch) = if throwing && self.throws == 0 {
                aim_at(eye, o)
            } else if throwing {
                let (yaw, _) = aim_at(eye, o);
                (yaw + deg_to_short(90.0), deg_to_short(45.0))
            } else {
                aim_at(eye, target_eye)
            };
            // The approach walks where it looks, so its heading is the steered
            // one; the firing phases look straight at the target.
            let yaw = if self.phase == HitPhase::Approach {
                let me_origin = snap.ps.origin(p);
                let bearing = yaw as f32 * 360.0 / 65536.0;
                let world = self.world.as_deref();
                deg_to_short(
                    self.steer
                        .heading(now, world, me_origin, bearing, STEER_AHEAD),
                )
            } else {
                yaw
            };
            self.aim = Some((yaw & 0xffff, pitch & 0xffff));
            self.los = match &self.world {
                Some(w) => LOS_HEIGHTS.iter().any(|h| {
                    let at = glam::Vec3::new(o[0], o[1], o[2] + h);
                    // Glass is left out: a round goes on through a pane.
                    let mask =
                        vcod_common::collision::MASK_SHOT & !vcod_common::collision::CONTENTS_GLASS;
                    w.point_trace(eye.into(), at, mask, true).fraction >= LOS_FRACTION
                }),
                None => false,
            };
            self.los_ever |= self.los;
            self.range = dist(eye, target_eye);
            self.in_range = self.range <= ENGAGE_RANGE;
        } else {
            self.aim = None;
            self.los = false;
            self.range = f32::INFINITY;
            self.in_range = false;
        }

        // The cook's clock starts when the switch has landed, not when the
        // phase did: the frag's raise is dead time no fuse counts down in.
        if self.phase == HitPhase::Cook
            && self.cook_armed.is_none()
            && snap.ps.field_i32(p, "weapon") == i32::from(self.frag_index)
        {
            println!("HIT: frag in hand at +{ms}ms, cooking");
            self.cook_armed = Some(now);
        }
        // The suicide of `--probe-grenade-death`, into a cook that has started:
        // what it captures is the grenade the death drops.
        if self.script == ShooterScript::GrenadeDeath
            && !self.killed_self
            && self
                .cook_armed
                .is_some_and(|t| now.duration_since(t) >= GRENADE_DEATH_AT)
        {
            println!("HIT: sending kill at +{ms}ms, {GRENADE_DEATH_AT:?} into the cook");
            self.pending.push("kill".to_string());
            self.killed_self = true;
        }
        // The respawn press, retried once a second while dead.
        self.press_use = false;
        if snap.ps.field_i32(p, "pm_type") != PM_NORMAL
            && self
                .used_at
                .is_none_or(|t| now.duration_since(t) >= TARGET_USE_RETRY)
        {
            self.used_at = Some(now);
            self.press_use = true;
        }

        // One offset per tap the sweep actually took, so a period spent
        // watching the target respawn costs nothing. Done before the trace
        // line, so the line and the cmd sent this frame carry the same offset.
        if self.phase == HitPhase::Sweep
            && self.cmd(now).buttons & BUTTON_ATTACK != 0
            && self
                .sweep_period
                .replace(in_phase.as_millis() / self.period.as_millis())
                .is_some_and(|p| p != in_phase.as_millis() / self.period.as_millis())
        {
            self.sweep_index += 1;
        }
        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            let cmd = self.cmd(now);
            let f = |n: &str| snap.ps.field_i32(p, n);
            let (target_num, target_etype, target_origin) = match self.target {
                Some((n, t, o)) => (n as i32, t, o),
                None => (-1, -1, [0.0; 3]),
            };
            let s = ShooterSample {
                phase: self.phase.label(),
                elapsed_ms: ms,
                server_time: snap.server_time.raw(),
                buttons: cmd.buttons,
                wbuttons: cmd.wbuttons,
                weaponstate: f("weaponstate"),
                weap_anim: f("weapAnim"),
                legs_anim: f("legsAnim"),
                torso_anim: f("torsoAnim"),
                event_sequence: f("eventSequence"),
                events: [
                    ev_v1(f("events[0]")),
                    ev_v1(f("events[1]")),
                    ev_v1(f("events[2]")),
                    ev_v1(f("events[3]")),
                ],
                event_parms: [
                    f("eventParms[0]"),
                    f("eventParms[1]"),
                    f("eventParms[2]"),
                    f("eventParms[3]"),
                ],
                clip: nonzero_pairs(&snap.ps.arrays.ammoclip),
                ammo: nonzero_pairs(&snap.ps.arrays.ammo),
                los: self.los,
                range: self.range,
                target_seen: self.target_seen,
                viewangles: [
                    snap.ps.field_f32(p, "viewangles[0]"),
                    snap.ps.field_f32(p, "viewangles[1]"),
                    snap.ps.field_f32(p, "viewangles[2]"),
                ],
                pitch_offset: self.pitch_offset(),
                target_num,
                target_etype,
                target_origin,
                origin: snap.ps.origin(p),
                grenade_time_left: f("grenadeTimeLeft"),
                weapon_delay: f("weaponDelay"),
                missiles: self.missiles.sample(snap),
            };
            // Every snapshot while shooting or while a grenade is in the air;
            // the walk and the watch collapse to their transitions and a
            // heartbeat.
            let firing = matches!(
                self.phase,
                HitPhase::SingleShot
                    | HitPhase::Burst
                    | HitPhase::Finish
                    | HitPhase::Sweep
                    | HitPhase::Cook
                    | HitPhase::Throw
            ) || !s.missiles.is_empty();
            if firing || keep_sample(&mut self.last_kept, s.watched(), ms) {
                println!("  {}", s.line().trim_end());
                for m in &s.missiles {
                    println!("  {}", m.line(ms).trim_end());
                }
                self.trace.push(s);
            }
            for ev in drain_v1(&mut self.tracker, snap, p) {
                print_bullet_hit(ms, &ev);
                if TRACKED_EVENTS.contains(&ev.event) {
                    println!(
                        "  event +{ms}ms ev {} parm {} entity {}",
                        ev.event, ev.parm, ev.entity_num
                    );
                    self.events.push((ms, ev.event, ev.parm, ev.entity_num));
                }
                if ev.event == EV_OBITUARY {
                    println!(
                        "  obituary +{ms}ms victim {} attacker {} parm {}",
                        ev.other_entity_num, ev.attacker_entity_num, ev.parm
                    );
                    // A kill of the other player is what says the shots
                    // landed: the shooter cannot see the target's health.
                    self.killed |=
                        ev.attacker_entity_num == self.me && ev.other_entity_num != self.me as u32;
                    self.obituaries.push((
                        ms,
                        ev.other_entity_num,
                        ev.attacker_entity_num,
                        ev.parm,
                    ));
                }
            }
            corpse_edges(snap, &mut self.live_corpses, ms, &mut self.corpses);
        }

        // A shot needs all three: the target in the PVS, a trace nothing
        // blocks, and a range the spread does not throw it off at.
        let shootable = self.shootable();
        let taps = if self.phase == HitPhase::Sweep {
            self.sweep_index as u32
        } else {
            (in_phase.as_millis() / self.period.as_millis()) as u32
        };
        // The approach keeps walking until it is close; the stop overshoots by
        // a stride or two, which is why the taps themselves are not gated on it.
        let ready = shootable && self.range <= self.script.engage_range();
        let cx = PhaseCtx {
            ready,
            target_etype: self.target.filter(|_| self.target_seen).map(|t| t.1),
            taps,
            script: self.script,
            cook_done: self
                .cook_armed
                .is_some_and(|t| now.duration_since(t) >= self.cook_hold()),
            throws: self.throws,
        };
        let next = self.phase.advance(in_phase, &cx);
        if next != self.phase {
            println!(
                "HIT: {} -> {} at +{ms}ms (los {}, range {:.0}, target {:?})",
                self.phase.label(),
                next.label(),
                self.los as i32,
                self.range,
                self.target.map(|t| (t.0, t.1)),
            );
            if self.phase == HitPhase::Approach {
                self.engaged_at = Some(self.range);
                self.engaged_los = self.los;
            }
            if self.phase == HitPhase::Throw {
                self.throws += 1;
            }
            if next == HitPhase::Cook {
                self.cook_armed = None;
                self.switch.reset();
            }
            self.phase = next;
            self.phase_started = Some(now);
        }
        self.phase == HitPhase::Done
    }
}

/// Writes one role's half of the hit capture to `<map>-<gametype>-<role>.txt`,
/// where `role` is the script's own prefix and the half: `hit-shooter`,
/// `grenade-target` and so on. Both halves carry the same header keys, so a
/// gate reads either the same way; `body` is the role's own section.
#[expect(clippy::too_many_arguments)]
fn write_hit_fixture(
    role: &str,
    configstrings: &[String],
    join: &JoinProbe,
    taken: &str,
    // The target half's own first-kill delay, which is a parameter of the run.
    first_kill: Duration,
    notes: &[String],
    body: &str,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let map = key("mapname");
    let gametype = key("g_gametype");

    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server: what a shot and a death do to a player.\n",
        retail_build()
    ));
    out.push_str(&format!(
        "# map {map}, gametype {gametype}, joined {}, weapon {}, role {role}, taken {taken}\n",
        join.team, join.weapon
    ));
    out.push_str(
        "# dedicated 1, sv_maxclients 8, sv_pure 0, stock scr_* defaults. Two probes on\n",
    );
    out.push_str("# the server: a target that stands still and a shooter that walks up to it.\n");
    out.push_str(&format!(
        "# The target kills itself with the `kill` client command {} s in and every {} s\n",
        first_kill.as_secs(),
        TARGET_KILL_PERIOD.as_secs()
    ));
    out.push_str("# after, so the death, the corpse, the obituary and the respawn are captured\n");
    out.push_str("# whether or not the shooter ever finds a line of sight. It presses use 3 s\n");
    out.push_str("# after each death, and again once a second until the script takes one.\n");
    out.push_str(
        "# Captured with tools/run_server.sh and --net-probe --probe-target / --save-hit.\n",
    );
    out.push_str(
        "# One !trace line per snapshot whose watched fields moved, plus one a second so\n",
    );
    out.push_str("# a settled stretch still carries a timeline: a run is 20 snapshots a second\n");
    out.push_str(
        "# for minutes and a standing player moves none of them. The shooter keeps every\n",
    );
    out.push_str("# snapshot of its three firing phases whether or not anything moved.\n");
    out.push_str(
        "# ms is since the role's script started; serverTime is the server's own clock.\n",
    );
    out.push_str("# clip and ammo are the non-zero entries of the two arrays, as index:value.\n");
    out.push_str("# ct is commandTime; legsTimer and torsoTimer are the anim timers, run down by each cmd.\n");
    for n in notes {
        out.push_str(n);
        out.push('\n');
    }
    out.push_str(body);

    let path = write_fixture_file(
        PLAYERSTATE_FIXTURE_DIR,
        &tagged_name(&format!("{map}-{gametype}-{role}"), tag),
        &out,
        overwrite,
    )?;
    println!("hit: {role} -> {path}");
    Ok(())
}

/// The target's half: the trace, the events, the corpse edges and the
/// scoreboards.
pub(super) fn write_target_fixture(
    script: ShooterScript,
    configstrings: &[String],
    join: &JoinProbe,
    target: &TargetProbe,
    now: Instant,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let mut notes = Vec::new();
    if target.trace.iter().all(|s| s.damage_event == 0) {
        notes.push(
            "# BROKEN no hit landed: damageEvent never moved, so this file measures deaths only \
and carries nothing about what a shot does to a player."
                .to_string(),
        );
    }
    if target.deaths == 0 {
        notes.push("# BROKEN the target never died: the `kill` command did nothing.".to_string());
    }
    if target.corpses.is_empty() {
        notes.push("# BROKEN no corpse ever appeared in the body queue.".to_string());
    }
    if !target.kills.is_empty() {
        notes.push(format!(
            "# --probe-kill-cmds: each `kill` (a !kill line) went out with exactly `cmds`\n\
# usercmds behind it, one message each, back to back, then no cmd for {} ms;\n\
# a kill every {} s once alive again, and no `score` (its flood window drops a kill).",
            KILL_CMDS_QUIET.as_millis(),
            KILL_CMDS_PERIOD.as_secs()
        ));
    }
    let mut body = String::from("[section target]\n");
    body.push_str(&format!(
        "!observed deaths={} traced={} obituaries={} corpse_edges={} scoreboards={}\n",
        target.deaths,
        target.trace.len(),
        target.obituaries.len(),
        target.corpses.len(),
        target.scoreboards.len(),
    ));
    for (ms, cmds) in &target.kills {
        body.push_str(&format!("!kill ms={ms} cmds={cmds}\n"));
    }
    for s in &target.trace {
        body.push_str(&s.line());
    }
    for (ms, ev, parm, ent) in &target.events {
        body.push_str(&format!(
            "!event ms={ms} event={ev} parm={parm} entity={ent}\n"
        ));
    }
    for (ms, victim, attacker, parm) in &target.obituaries {
        body.push_str(&format!(
            "!obituary ms={ms} victim={victim} attacker={attacker} parm={parm}\n"
        ));
    }
    for (ms, ent, cn, appeared) in &target.corpses {
        body.push_str(&format!(
            "!corpse ms={ms} entity={ent} clientNum={cn} appeared={}\n",
            *appeared as i32
        ));
    }
    for (ms, line) in &target.scoreboards {
        body.push_str(&format!("!scoreboard ms={ms} {line}\n"));
    }
    let taken = format!(
        "{} traced snapshots over {} s",
        target.trace.len(),
        target.elapsed_ms(now) / 1000
    );
    let role = format!("{}-target", script.prefix());
    write_hit_fixture(
        &role,
        configstrings,
        join,
        &taken,
        target.kill_at,
        &notes,
        &body,
        tag,
        overwrite,
    )
}

/// The shooter's half: the trace with what it could see of the target, the
/// obituaries and the corpse edges.
pub(super) fn write_shooter_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    hit: &HitProbe,
    now: Instant,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let mut notes = Vec::new();
    let missiles: usize = hit.trace.iter().map(|s| s.missiles.len()).sum();
    if !hit.los_ever {
        notes.push(format!(
            "# BROKEN no line of sight after {} s: the shots in this file hit the map, not a player.",
            APPROACH_LIMIT.as_secs()
        ));
    } else if hit.script.throws_grenades() {
        // A grenade at the target's feet is not meant to kill it; what says
        // the throw worked is a missile on the wire, and what says it landed
        // is the target fixture's health.
        if missiles == 0 {
            notes.push(
                "# BROKEN no missile ever reached the wire: the frag switch or the throw \
failed, so this file measures nothing about a grenade."
                    .to_string(),
            );
        }
    } else if !hit.killed {
        notes.push(format!(
            "# BROKEN no kill: the shots went out at {:.0} units with los {} and the target never \
died. Whether any of them landed is not visible from this side; the target fixture's damageEvent \
is what says so.",
            hit.engaged_at.unwrap_or(hit.range),
            hit.engaged_los as i32,
        ));
    }
    let mut body = String::from("[section shooter]\n");
    body.push_str(&format!(
        "!observed script={:?} reached={} los_found={} engaged_los={} killed={} engaged_at={:.0} \
traced={} throws={} missile_samples={} events={} obituaries={} corpse_edges={}\n",
        hit.script,
        hit.phase.label(),
        hit.los_ever as i32,
        hit.engaged_los as i32,
        hit.killed as i32,
        hit.engaged_at.unwrap_or(hit.range),
        hit.trace.len(),
        hit.throws,
        missiles,
        hit.events.len(),
        hit.obituaries.len(),
        hit.corpses.len(),
    ));
    for s in &hit.trace {
        body.push_str(&s.line());
        for m in &s.missiles {
            body.push_str(&m.line(s.elapsed_ms));
        }
    }
    for (ms, ev, parm, ent) in &hit.events {
        body.push_str(&format!(
            "!event ms={ms} event={ev} parm={parm} entity={ent}\n"
        ));
    }
    for (ms, victim, attacker, parm) in &hit.obituaries {
        body.push_str(&format!(
            "!obituary ms={ms} victim={victim} attacker={attacker} parm={parm}\n"
        ));
    }
    for (ms, ent, cn, appeared) in &hit.corpses {
        body.push_str(&format!(
            "!corpse ms={ms} entity={ent} clientNum={cn} appeared={}\n",
            *appeared as i32
        ));
    }
    let taken = format!(
        "{} traced snapshots over {} s",
        hit.trace.len(),
        hit.elapsed_ms(now) / 1000
    );
    let role = format!("{}-shooter", hit.script.prefix());
    // The shooter never runs beside a target on a shortened delay: the mode
    // that shortens it writes the netchan fixture instead of this one.
    write_hit_fixture(
        &role,
        configstrings,
        join,
        &taken,
        TARGET_KILL_AT,
        &notes,
        &body,
        tag,
        overwrite,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The phase context a test varies one field of at a time.
    fn ctx(ready: bool, script: ShooterScript) -> PhaseCtx {
        PhaseCtx {
            ready,
            target_etype: Some(crate::entities::ET_PLAYER),
            taps: 0,
            script,
            cook_done: false,
            throws: 0,
        }
    }

    /// The two transitions the capture depends on: the approach ends on a
    /// clear trace, and it ends anyway at the limit so a run behind a wall
    /// still records its shots.
    #[test]
    fn hit_approach_ends_on_a_clear_trace_or_at_the_limit() {
        let hit = ShooterScript::Hit;
        let blocked = HitPhase::Approach.advance(Duration::from_secs(30), &ctx(false, hit));
        assert_eq!(blocked, HitPhase::Approach);
        let clear = HitPhase::Approach.advance(Duration::from_secs(30), &ctx(true, hit));
        assert_eq!(clear, HitPhase::SingleShot);
        let out_of_time = HitPhase::Approach.advance(APPROACH_LIMIT, &ctx(false, hit));
        assert_eq!(out_of_time, HitPhase::SingleShot);
    }

    /// A target that stopped being a player is a target that died, which is
    /// what `Finish` is waiting for; a target the server stopped sending is
    /// not, or the shooter would stop firing every time PVS blinked.
    #[test]
    fn hit_finish_ends_when_the_target_stops_being_a_player() {
        let hit = ShooterScript::Hit;
        let one = Duration::from_secs(1);
        let mut cx = ctx(true, hit);
        cx.taps = 3;
        assert_eq!(HitPhase::Finish.advance(one, &cx), HitPhase::Finish);
        cx.target_etype = None;
        cx.ready = false;
        assert_eq!(HitPhase::Finish.advance(one, &cx), HitPhase::Finish);
        cx.target_etype = Some(crate::entities::ET_CORPSE);
        assert_eq!(HitPhase::Finish.advance(one, &cx), HitPhase::Watch);
        let mut spent = ctx(true, hit);
        spent.taps = FINISH_TAPS;
        assert_eq!(HitPhase::Finish.advance(one, &spent), HitPhase::Watch);
    }

    /// The sweep replaces the three scripted firing phases and ends on its
    /// table, not on the clock.
    #[test]
    fn hit_sweep_takes_the_approach_and_ends_on_its_table() {
        let t = Duration::from_secs(30);
        let plain = HitPhase::Approach.advance(t, &ctx(true, ShooterScript::Hit));
        assert_eq!(plain, HitPhase::SingleShot);
        let mut cx = ctx(true, ShooterScript::Sweep);
        assert_eq!(HitPhase::Approach.advance(t, &cx), HitPhase::Sweep);
        cx.taps = 3;
        assert_eq!(HitPhase::Sweep.advance(t, &cx), HitPhase::Sweep);
        cx.taps = SWEEP_PITCH_OFFSETS.len() as u32;
        assert_eq!(HitPhase::Sweep.advance(t, &cx), HitPhase::Watch);
    }

    /// A grenade run cooks instead of shooting, and ends after its second
    /// throw rather than on the watch clock the bullet run ends on.
    #[test]
    fn grenade_run_cooks_throws_twice_and_stops() {
        let t = Duration::from_secs(30);
        let mut cx = ctx(true, ShooterScript::Grenade);
        assert_eq!(HitPhase::Approach.advance(t, &cx), HitPhase::Cook);
        // The cook holds the trigger until the hold is spent, not on a clock.
        assert_eq!(
            HitPhase::Cook.advance(Duration::from_secs(1), &cx),
            HitPhase::Cook
        );
        cx.cook_done = true;
        assert_eq!(
            HitPhase::Cook.advance(Duration::from_secs(1), &cx),
            HitPhase::Throw
        );
        assert_eq!(HitPhase::Throw.advance(THROW_HOLD, &cx), HitPhase::Watch);
        cx.throws = 1;
        assert_eq!(
            HitPhase::Watch.advance(GRENADE_WATCH_HOLD, &cx),
            HitPhase::Cook
        );
        cx.throws = GRENADE_THROWS;
        assert_eq!(
            HitPhase::Watch.advance(GRENADE_WATCH_HOLD, &cx),
            HitPhase::Done
        );
    }

    /// The melee script is the bullet script with another bit: it takes the
    /// same phases, and every tap of them swings instead of shooting. A tap
    /// that still carried `BUTTON_ATTACK` would fire the rifle and the
    /// capture would measure a bullet.
    #[test]
    fn melee_run_takes_the_bullet_phases_and_swings_on_every_tap() {
        let t = Duration::from_secs(30);
        let cx = ctx(true, ShooterScript::Melee);
        assert_eq!(HitPhase::Approach.advance(t, &cx), HitPhase::SingleShot);
        assert_eq!(
            HitPhase::SingleShot.advance(SINGLE_SHOT_HOLD, &cx),
            HitPhase::Burst
        );
        // The head of a tap period, where the bit is down.
        let now = Instant::now();
        for phase in [HitPhase::SingleShot, HitPhase::Burst, HitPhase::Finish] {
            let mut melee = HitProbe::new(ShooterScript::Melee);
            melee.phase = phase;
            melee.phase_started = Some(now);
            let bits = melee.cmd(now).buttons;
            assert_eq!(bits & net::msg::BUTTON_MELEE, net::msg::BUTTON_MELEE);
            assert_eq!(bits & BUTTON_ATTACK, 0, "{phase:?} fired the rifle");

            let mut bullet = HitProbe::new(ShooterScript::Hit);
            bullet.phase = phase;
            bullet.phase_started = Some(now);
            let bits = bullet.cmd(now).buttons;
            assert_eq!(bits & BUTTON_ATTACK, BUTTON_ATTACK);
            assert_eq!(bits & net::msg::BUTTON_MELEE, 0);
        }
    }

    /// A trace 20 samples a second for minutes is unreadable and huge, and a
    /// collapse that drops a transition is worse than either.
    #[test]
    fn hit_trace_keeps_every_transition_and_one_sample_a_second() {
        let mut last = None;
        assert!(keep_sample(&mut last, vec![100], 0));
        assert!(!keep_sample(&mut last, vec![100], 500));
        assert!(keep_sample(&mut last, vec![55], 550), "a change is kept");
        assert!(!keep_sample(&mut last, vec![55], 900));
        assert!(
            keep_sample(&mut last, vec![55], 1550),
            "the heartbeat is due"
        );
    }
}
