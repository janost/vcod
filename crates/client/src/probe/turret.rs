//! `--save-turret`: the mounted MG capture and its fixture.

use super::combat::PULSE_HOLD;
use super::fixture::{
    FixturePhase, cmd_fields, serverinfo_key, tagged_name, vec_exact, vec_str, write_fixture_file,
    write_phase_blocks,
};
use super::hit::EV_BULLET_HIT_LARGE;
use super::*;

/// Where the turret capture lives.
static TURRET_FIXTURE_DIR: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../server/tests/fixtures/turret");

/// The capture's gun: the carentan turret nearest this spot.
const TURRET_XY: [f32; 2] = [1712.0, 1830.0];

/// How long past the join the capture waits for the gsc probe's placement.
const TURRET_WAIT: Duration = Duration::from_secs(30);

const TURRET_SETTLE: Duration = Duration::from_millis(500);

const TURRET_SWEEP: Duration = Duration::from_secs(4);

const TURRET_FIRE: Duration = Duration::from_millis(1000);

const TURRET_COOL: Duration = Duration::from_millis(1500);

const TURRET_STRAFE: Duration = Duration::from_millis(1500);

/// The gsc probe puts the gunner 40 units behind the gun; standing this close
/// in xy is being placed.
const TURRET_PLACED: f32 = 60.0;

/// A wire `eType` 11 this close to the lump origin is the capture's gun.
const TURRET_NUM_RADIUS: f32 = 64.0;

/// Where the aim at the gun points: above its origin, near its bounds centre.
const TURRET_AIM_Z: f32 = 40.0;

const TURRET_FLICK: f32 = 60.0;

/// `fire` shoots off the target's line, inside the arc and down into the
/// world, so the target survives to `target` and the rounds leave impacts.
const TURRET_FIRE_YAW: f32 = 30.0;

const TURRET_FIRE_PITCH: f32 = 10.0;

const TURRET_PITCH_EDGE: f32 = 60.0;

const TURRET_YAW_EDGE: f32 = 90.0;

/// The strafe stops once the bearing off the gun's "behind" direction passes
/// this, clear of the stock gun's 45-degree arc...
const TURRET_REFUSE_BEARING: f32 = 50.0;

/// ...while still this close, inside the use scan's 128-unit reach, so the
/// refused tap tests the arc and not the range.
const TURRET_REFUSE_RANGE: f32 = 100.0;

/// A refused tap this far out is too near the scan's reach to prove the arc.
const TURRET_REFUSE_TAP_RANGE: f32 = 110.0;

/// `--save-turret`'s phases, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum TurretPhase {
    #[default]
    Wait, // until the gsc has placed us
    Aim,      // face the gun's tag area
    Mount,    // tap use
    Yaw,      // sweep yaw -90..+90 relative to the gun, 2 deg per cmd
    Pitch,    // sweep pitch -60..+60
    Flick,    // one cmd turning 60 deg: the 15-per-frame limit
    Fire,     // attack held TURRET_FIRE, straight along the gun
    Target,   // attack held at the axis client if it is in the snapshot
    Cool,     // released, until the cooldown alias or TURRET_COOL
    Dismount, // tap use
    Crouch,   // crouch held, tap use: mount from a crouch
    Uncrouch, // tap use again: EV_STANCE_FORCE_CROUCH expected
    Strafe,   // strafe right TURRET_STRAFE: out of the yaw arc
    Refused,  // face the gun, tap use: must not mount
    Done,
}

impl TurretPhase {
    pub(super) fn label(self) -> &'static str {
        match self {
            TurretPhase::Wait => "wait",
            TurretPhase::Aim => "aim",
            TurretPhase::Mount => "mount",
            TurretPhase::Yaw => "yaw",
            TurretPhase::Pitch => "pitch",
            TurretPhase::Flick => "flick",
            TurretPhase::Fire => "fire",
            TurretPhase::Target => "target",
            TurretPhase::Cool => "cool",
            TurretPhase::Dismount => "dismount",
            TurretPhase::Crouch => "crouch",
            TurretPhase::Uncrouch => "uncrouch",
            TurretPhase::Strafe => "strafe",
            TurretPhase::Refused => "refused",
            TurretPhase::Done => "done",
        }
    }
}

impl FixturePhase for TurretPhase {
    fn order(self) -> u8 {
        self as u8
    }
    fn label(self) -> &'static str {
        TurretPhase::label(self)
    }
}

pub(super) struct TurretGun {
    origin: [f32; 3],
    /// The lump's `angles`, for the header.
    angles: [f32; 3],
    yaw: f32,
    /// Learned from the first snapshot carrying `eType` 11 near `origin`.
    num: Option<u32>,
}

/// The gun in the entity lump nearest `TURRET_XY`.
fn turret_from_entities(entities: &str) -> Option<TurretGun> {
    vcod_common::bsp::entity_blocks(entities)
        .into_iter()
        .filter(|b| b.get("classname").map(String::as_str) == Some("misc_mg42"))
        .filter_map(|b| {
            let origin = vcod_common::bsp::parse_vec3(b.get("origin")?)?;
            let angles = b
                .get("angles")
                .and_then(|a| vcod_common::bsp::parse_vec3(a))
                .unwrap_or([0.0; 3]);
            Some(TurretGun {
                origin,
                angles,
                yaw: angles[1],
                num: None,
            })
        })
        .min_by(|a, b| {
            let d = |g: &TurretGun| (g.origin[0] - TURRET_XY[0]).hypot(g.origin[1] - TURRET_XY[1]);
            d(a).total_cmp(&d(b))
        })
}

/// Step `i` of a sweep from `-edge` to `edge` in 2-degree steps.
fn sweep_step(i: usize, edge: f32) -> Option<f32> {
    let o = -edge + 2.0 * i as f32;
    (o <= edge).then_some(o)
}

/// Yaw offset from the gun's own yaw for sweep step `i`, -90..=90 in 2s.
fn turret_sweep_offset(i: usize) -> Option<f32> {
    sweep_step(i, TURRET_YAW_EDGE)
}

/// `at`'s horizontal bearing in degrees from the gun, measured against the
/// direction behind it (`yaw + 180`), and its horizontal distance. The arc
/// test in `G_IsTurretUsable` reads the same angle (cod11-turrets.md 4.2).
fn turret_bearing(gun: [f32; 3], yaw: f32, at: [f32; 3]) -> (f32, f32) {
    let (dx, dy) = (at[0] - gun[0], at[1] - gun[1]);
    let d = dx.hypot(dy);
    let (s, c) = yaw.to_radians().sin_cos();
    let cos = if d > 0.0 { (-c * dx - s * dy) / d } else { 1.0 };
    (cos.clamp(-1.0, 1.0).acos().to_degrees(), d)
}

/// Where the strafe out of the arc stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StrafeStop {
    /// Still inside the refuse bearing: keep strafing.
    Inside,
    /// Past the bearing and inside the range: stop here.
    Outside,
    /// Past the range: a refusal here would test the distance.
    OutOfRange,
}

fn turret_strafe_stop(bearing: f32, dist: f32) -> StrafeStop {
    if dist >= TURRET_REFUSE_RANGE {
        StrafeStop::OutOfRange
    } else if bearing > TURRET_REFUSE_BEARING {
        StrafeStop::Outside
    } else {
        StrafeStop::Inside
    }
}

/// The gunner's playerstate as one `!trace` line; `ct=` is its
/// `commandTime`, which places each cmd on the frame that ran it, and
/// `exact=` and `velocity=` carry the origin and velocity at full precision.
fn turret_trace_line(ms: u128, snap: &net::snapshot::Snapshot) -> String {
    let p = wire();
    let f = |n: &str| snap.ps.field_i32(p, n);
    format!(
        "!trace ms={ms} serverTime={} ct={} pm_type={} pm_flags={} eFlags={} groundEntityNum={} \
origin={} viewangles={} delta_angles={},{},{} viewlocked={} viewlocked_entNum={} gunfx={} \
hint={}:{}:{} weapon={} legsAnim={} torsoAnim={} eventSequence={} events={},{},{},{} \
eventParms={},{},{},{} exact={} velocity={}",
        snap.server_time,
        f("commandTime"),
        f("pm_type"),
        f("pm_flags"),
        f("eFlags"),
        f("groundEntityNum"),
        vec_str(snap.ps.origin(p)),
        vec_str(snap.ps.viewangles(p)),
        f("delta_angles[0]"),
        f("delta_angles[1]"),
        f("delta_angles[2]"),
        f("viewlocked"),
        f("viewlocked_entNum"),
        f("gunfx"),
        f("serverCursorHint"),
        f("serverCursorHintVal"),
        f("serverCursorHintString"),
        f("weapon"),
        f("legsAnim"),
        f("torsoAnim"),
        f("eventSequence"),
        ev_v1(f("events[0]")),
        ev_v1(f("events[1]")),
        ev_v1(f("events[2]")),
        ev_v1(f("events[3]")),
        f("eventParms[0]"),
        f("eventParms[1]"),
        f("eventParms[2]"),
        f("eventParms[3]"),
        vec_exact(snap.ps.origin(p)),
        vec_exact(std::array::from_fn(|i| {
            snap.ps.field_f32(p, &format!("velocity[{i}]"))
        })),
    )
}

/// The turret entity's watched fields, without the `ms`, so an unchanged
/// snapshot can be told from a changed one.
fn turret_entity_fields(e: &net::msg::EntityState) -> String {
    let p = wire();
    let g = |n: &str| e.field_i32(p, n);
    format!(
        "eFlags={} angles2={} loopSound={} otherEntityNum={} eventSequence={} \
events={},{},{},{} eventParms={},{},{},{}",
        g("eFlags"),
        vec_str([
            e.field_f32(p, "angles2[0]"),
            e.field_f32(p, "angles2[1]"),
            e.field_f32(p, "angles2[2]"),
        ]),
        g("loopSound"),
        g("otherEntityNum"),
        g("eventSequence"),
        ev_v1(g("events[0]")),
        ev_v1(g("events[1]")),
        ev_v1(g("events[2]")),
        ev_v1(g("events[3]")),
        g("eventParms[0]"),
        g("eventParms[1]"),
        g("eventParms[2]"),
        g("eventParms[3]"),
    )
}

/// The mounted MG capture: wait for the gsc probe to put us behind the gun,
/// mount it, sweep it past both arcs, fire it along its yaw and at the axis
/// client, dismount, remount from a crouch, then strafe out of the arc and
/// try once more.
#[derive(Default)]
pub(super) struct TurretProbe {
    pub(super) phase: TurretPhase,
    /// How long `target` holds the trigger; [`TURRET_FIRE`] when unset.
    pub(super) target_for: Option<Duration>,
    pub(super) phase_started: Option<Instant>,
    pub(super) settled_at: Option<Instant>,
    pub(super) gun: Option<TurretGun>,
    /// The absolute view at the gun, off this snapshot's eye, in wire shorts.
    pub(super) gun_aim: Option<(i32, i32)>,
    /// The same at the axis client's eye, while it is in the snapshot.
    pub(super) target_aim: Option<(i32, i32)>,
    /// The snapshot's own view, held while waiting.
    pub(super) held_view: Option<(i32, i32)>,
    /// Cmds sent in the current sweep phase.
    pub(super) sweep_i: usize,
    /// When the current phase's use tap goes down.
    pub(super) tap_at: Option<Instant>,
    /// The cooldown alias was seen on the gun during `cool`.
    pub(super) cooled: bool,
    pub(super) target_seen: bool,
    /// A live axis client was in some snapshot before `target`.
    pub(super) target_seen_before: bool,
    /// An obituary named another client as the victim, and in which phase.
    pub(super) target_killed_in: Option<TurretPhase>,
    /// The refused tap's bearing and distance have been noted.
    pub(super) refused_noted: bool,
    pub(super) viewlocked: i32,
    /// `(phase, ms, wire cmd, asked view, netchan sequence of its message)`
    /// per cmd sent from `aim` on.
    pub(super) cmds: Vec<(TurretPhase, u128, net::msg::UserCmd, [i32; 3], u32)>,
    /// Every trace, turret, event and server line, in arrival order.
    pub(super) lines: Vec<(TurretPhase, u128, String)>,
    pub(super) stations: Vec<(TurretPhase, [f32; 3], [f32; 3])>,
    pub(super) traced: Option<u32>,
    pub(super) last_turret: Option<String>,
    pub(super) notes: Vec<String>,
    pub(super) events: vcod_common::net::events::EventTracker,
}

impl TurretProbe {
    pub(super) fn running(&self) -> bool {
        self.phase != TurretPhase::Done
    }

    /// The gun out of the BSP the gamestate names.
    pub(super) fn load_map(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, map: &str) {
        let Some(fs) = fs else {
            println!("TURRET: no game data, no gun to find");
            return;
        };
        let Some(bsp) = vcod_common::mapworld::find(fs, map).ok() else {
            println!("TURRET: cannot load {map}, no gun to find");
            return;
        };
        self.gun = turret_from_entities(&bsp.entities);
        match &self.gun {
            Some(g) => println!(
                "TURRET: misc_mg42 at [{:.0},{:.0},{:.0}] yaw {:.0}",
                g.origin[0], g.origin[1], g.origin[2], g.yaw
            ),
            None => println!("TURRET: {map} has no misc_mg42"),
        }
    }

    fn ms(&self, now: Instant) -> u128 {
        self.settled_at
            .map_or(0, |t| now.saturating_duration_since(t).as_millis())
    }

    /// The absolute view this phase asks for, `(yaw, pitch)` in wire shorts.
    fn view(&self) -> Option<(i32, i32)> {
        let gun_yaw = self.gun.as_ref()?.yaw;
        let along = |yaw_off: f32, pitch: f32| {
            Some((
                deg_to_short(gun_yaw + yaw_off) & 0xffff,
                deg_to_short(pitch) & 0xffff,
            ))
        };
        match self.phase {
            TurretPhase::Wait | TurretPhase::Done => self.held_view,
            TurretPhase::Aim | TurretPhase::Mount | TurretPhase::Crouch | TurretPhase::Refused => {
                self.gun_aim
            }
            // A sweep holds its last step once it runs out.
            TurretPhase::Yaw => along(
                turret_sweep_offset(self.sweep_i).unwrap_or(TURRET_YAW_EDGE),
                0.0,
            ),
            TurretPhase::Pitch => along(
                0.0,
                sweep_step(self.sweep_i, TURRET_PITCH_EDGE).unwrap_or(TURRET_PITCH_EDGE),
            ),
            TurretPhase::Flick => along(TURRET_FLICK, 0.0),
            TurretPhase::Fire => along(TURRET_FIRE_YAW, TURRET_FIRE_PITCH),
            TurretPhase::Target => self.target_aim.or_else(|| along(0.0, 0.0)),
            TurretPhase::Cool
            | TurretPhase::Dismount
            | TurretPhase::Uncrouch
            | TurretPhase::Strafe => along(0.0, 0.0),
        }
    }

    pub(super) fn cmd(&self, now: Instant) -> net::msg::UserCmd {
        let mut cmd = net::msg::NULL_USERCMD;
        if let Some((yaw, pitch)) = self.view() {
            cmd.angles = [pitch, yaw, 0];
        }
        if self
            .tap_at
            .is_some_and(|t| now >= t && now.duration_since(t) < PULSE_HOLD)
        {
            cmd.buttons |= BUTTON_USE;
        }
        // Held, not tapped: the mounted frame reads the held bit
        // (docs/research/cod11-turrets.md, 6.3).
        let firing = match self.phase {
            TurretPhase::Fire => true,
            TurretPhase::Target => self.target_aim.is_some(),
            _ => false,
        };
        if firing {
            cmd.buttons |= net::msg::BUTTON_ATTACK;
        }
        if self.phase == TurretPhase::Crouch {
            cmd.wbuttons |= net::msg::WBUTTON_CROUCH;
        }
        if self.phase == TurretPhase::Strafe {
            cmd.right = 127;
        }
        cmd
    }

    pub(super) fn on_server_command(&mut self, now: Instant, tokens: &[String]) {
        let ms = self.ms(now);
        self.lines.push((
            self.phase,
            ms,
            format!("!server ms={ms} {}", escape_ctl(&tokens.join(" "))),
        ));
    }

    pub(super) fn record(
        &mut self,
        now: Instant,
        sent: net::msg::UserCmd,
        asked: [i32; 3],
        seq: u32,
    ) {
        if matches!(self.phase, TurretPhase::Wait | TurretPhase::Done) {
            return;
        }
        let ms = self.ms(now);
        self.cmds.push((self.phase, ms, sent, asked, seq));
        if matches!(self.phase, TurretPhase::Yaw | TurretPhase::Pitch) {
            self.sweep_i += 1;
        }
    }

    fn enter(&mut self, now: Instant, next: TurretPhase, snap: &net::snapshot::Snapshot) {
        if next == self.phase {
            return;
        }
        let p = wire();
        println!(
            "TURRET: {} -> {} at +{}ms",
            self.phase.label(),
            next.label(),
            self.ms(now)
        );
        match self.phase {
            TurretPhase::Mount | TurretPhase::Crouch if self.viewlocked == 0 => {
                self.notes.push(format!(
                    "# BROKEN {}: viewlocked 0 after the use tap",
                    self.phase.label()
                ))
            }
            TurretPhase::Dismount | TurretPhase::Uncrouch if self.viewlocked != 0 => {
                self.notes.push(format!(
                    "# BROKEN {}: still viewlocked after the use tap",
                    self.phase.label()
                ))
            }
            TurretPhase::Target if !self.target_seen => {
                self.notes.push(match self.target_killed_in {
                    Some(k) => format!(
                        "# BROKEN target: the axis client was dead through target (obituary in {})",
                        k.label()
                    ),
                    None if self.target_seen_before => {
                        "# BROKEN target: the axis client left the snapshot before target"
                            .to_string()
                    }
                    None => {
                        "# BROKEN target: the axis client was never in the snapshot".to_string()
                    }
                })
            }
            TurretPhase::Refused => self.notes.push(if self.viewlocked == 0 {
                "# REFUSED ok".to_string()
            } else {
                "# BROKEN mounted from outside the arc".to_string()
            }),
            _ => {}
        }
        self.phase = next;
        self.phase_started = Some(now);
        self.sweep_i = 0;
        self.tap_at = match next {
            TurretPhase::Mount | TurretPhase::Dismount | TurretPhase::Uncrouch => Some(now),
            TurretPhase::Crouch | TurretPhase::Refused => Some(now + TURRET_SETTLE),
            _ => None,
        };
        if next != TurretPhase::Done {
            self.stations
                .push((next, snap.ps.origin(p), snap.ps.viewangles(p)));
        }
    }

    /// The `!turret` line, when the gun is on the wire and a field moved.
    fn turret_line(&mut self, ms: u128, snap: &net::snapshot::Snapshot) {
        let Some(num) = self.gun.as_ref().and_then(|g| g.num) else {
            return;
        };
        let Some(e) = snap.entities.get(&num) else {
            return;
        };
        let fields = turret_entity_fields(e);
        if self.last_turret.as_deref() == Some(fields.as_str()) {
            return;
        }
        self.lines.push((
            self.phase,
            ms,
            format!("!turret ms={ms} num={num} {fields}"),
        ));
        self.last_turret = Some(fields);
    }

    /// Feeds the newest snapshot in. Returns true once the run is done.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let p = wire();
        self.settled_at.get_or_insert(now);
        let phase_started = *self.phase_started.get_or_insert(now);
        let in_phase = now.duration_since(phase_started);
        let ms = self.ms(now);

        let Some((gun_origin, gun_yaw, gun_num)) =
            self.gun.as_ref().map(|g| (g.origin, g.yaw, g.num))
        else {
            self.notes.push(format!(
                "# BROKEN no misc_mg42 near {} {} in the entity lump (no game data?)",
                TURRET_XY[0], TURRET_XY[1]
            ));
            self.enter(now, TurretPhase::Done, snap);
            return true;
        };
        if gun_num.is_none() {
            let num = snap
                .entities
                .iter()
                .filter(|(_, e)| e.field_i32(p, "eType") == ET_TURRET)
                .map(|(&n, e)| (n, dist(e.origin(p), gun_origin)))
                .filter(|&(_, d)| d <= TURRET_NUM_RADIUS)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(n, _)| n);
            if let (Some(n), Some(g)) = (num, self.gun.as_mut()) {
                println!("TURRET: the gun is entity {n}");
                g.num = Some(n);
            }
        }
        let gun_num = self.gun.as_ref().and_then(|g| g.num);

        if snap.ps.field_i32(p, "pm_type") == PM_DEAD {
            self.notes
                .push(format!("# BROKEN died in {}", self.phase.label()));
            self.enter(now, TurretPhase::Done, snap);
            return true;
        }

        let origin = snap.ps.origin(p);
        let h = snap.ps.field_f32(p, "viewHeightCurrent");
        let eye = [
            origin[0],
            origin[1],
            origin[2] + if h > 0.0 { h } else { EYE_HEIGHT },
        ];
        let wrap = |(yaw, pitch): (i32, i32)| (yaw & 0xffff, pitch & 0xffff);
        self.gun_aim = Some(wrap(aim_at(
            eye,
            [gun_origin[0], gun_origin[1], gun_origin[2] + TURRET_AIM_Z],
        )));
        let me = snap.ps.field_i32(p, "clientNum") as u32;
        self.target_aim = snap
            .entities
            .iter()
            .filter(|&(&n, e)| n != me && e.field_i32(p, "eType") == crate::entities::ET_PLAYER)
            .map(|(_, e)| e.origin(p))
            .min_by(|a, b| dist(*a, eye).total_cmp(&dist(*b, eye)))
            .map(|o| wrap(aim_at(eye, [o[0], o[1], o[2] + EYE_HEIGHT])));
        if self.target_aim.is_some() {
            if self.phase == TurretPhase::Target {
                self.target_seen = true;
            } else if (self.phase as u8) < TurretPhase::Target as u8 {
                self.target_seen_before = true;
            }
        }
        // The first snapshot at or past the refused tap is where it was made.
        if self.phase == TurretPhase::Refused
            && !self.refused_noted
            && self.tap_at.is_some_and(|t| now >= t)
        {
            self.refused_noted = true;
            let (bearing, d) = turret_bearing(gun_origin, gun_yaw, snap.ps.origin(p));
            self.notes
                .push(format!("# refused bearing={bearing:.1} dist={d:.1}"));
            if d >= TURRET_REFUSE_TAP_RANGE {
                self.notes
                    .push(format!("# BROKEN refused tap at dist={d:.1}"));
            }
        }
        if self.phase == TurretPhase::Wait && self.stations.is_empty() {
            self.stations
                .push((TurretPhase::Wait, snap.ps.origin(p), snap.ps.viewangles(p)));
        }
        let v = snap.ps.viewangles(p);
        self.held_view = Some(wrap((deg_to_short(v[1]), deg_to_short(v[0]))));
        self.viewlocked = snap.ps.field_i32(p, "viewlocked");

        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            self.lines
                .push((self.phase, ms, turret_trace_line(ms, snap)));
            self.turret_line(ms, snap);
            if matches!(self.phase, TurretPhase::Fire | TurretPhase::Target) {
                for (&n, e) in snap.entities.iter().filter(|&(&n, e)| {
                    n != me && e.field_i32(p, "eType") == crate::entities::ET_PLAYER
                }) {
                    let f = |name: &str| e.field_f32(p, name);
                    let i = |name: &str| e.field_i32(p, name);
                    self.lines.push((
                        self.phase,
                        ms,
                        format!(
                            "!victim ms={ms} num={n} trTime={} trBase={:.3},{:.3},{:.3} \
                             trDelta={:.3},{:.3},{:.3} yaw={:.3} legsAnim={} torsoAnim={} \
                             eFlags={} solid={} eventSequence={}",
                            i("pos.trTime"),
                            f("pos.trBase[0]"),
                            f("pos.trBase[1]"),
                            f("pos.trBase[2]"),
                            f("pos.trDelta[0]"),
                            f("pos.trDelta[1]"),
                            f("pos.trDelta[2]"),
                            f("apos.trBase[1]"),
                            i("legsAnim"),
                            i("torsoAnim"),
                            i("eFlags"),
                            i("solid"),
                            i("eventSequence"),
                        ),
                    ));
                }
            }
        }
        for ev in drain_v1(&mut self.events, snap, p) {
            self.lines.push((
                self.phase,
                ms,
                format!(
                    "!event ms={ms} event={} parm={} entity={}",
                    ev.event, ev.parm, ev.entity_num
                ),
            ));
            if ev.event == EV_BULLET_HIT_SMALL || ev.event == EV_BULLET_HIT_LARGE {
                self.lines.push((
                    self.phase,
                    ms,
                    format!(
                        "!impact ms={ms} event={} origin={}",
                        ev.event,
                        vec_str(ev.pos)
                    ),
                ));
            }
            // The cooldown alias rides the gun's own ring (cod11-turrets.md
            // 6.4); a temp entity at the gun counts too until a capture says.
            let at_gun = gun_num == Some(ev.entity_num)
                || (ev.entity_num != u32::MAX && dist(ev.pos, gun_origin) <= TURRET_NUM_RADIUS);
            if ev.event == crate::fx::registry::EV_OBITUARY
                && ev.other_entity_num != me
                && self.target_killed_in.is_none()
            {
                self.target_killed_in = Some(self.phase);
            }
            if self.phase == TurretPhase::Cool
                && ev.event == crate::fx::registry::EV_SOUND_ALIAS
                && at_gun
            {
                self.cooled = true;
            }
        }

        let placed = horiz_dist(origin, gun_origin) <= TURRET_PLACED;
        let settled = in_phase >= TURRET_SETTLE;
        let next = match self.phase {
            TurretPhase::Wait if placed => TurretPhase::Aim,
            TurretPhase::Wait if in_phase >= TURRET_WAIT => {
                self.notes.push(
                    "# BROKEN not placed: run client-probes/probe_turret with +set probe_teleport 1"
                        .to_string(),
                );
                TurretPhase::Done
            }
            TurretPhase::Aim if settled => TurretPhase::Mount,
            TurretPhase::Mount if settled => TurretPhase::Yaw,
            TurretPhase::Yaw
                if in_phase >= TURRET_SWEEP && turret_sweep_offset(self.sweep_i).is_none() =>
            {
                TurretPhase::Pitch
            }
            TurretPhase::Pitch
                if in_phase >= TURRET_SWEEP
                    && sweep_step(self.sweep_i, TURRET_PITCH_EDGE).is_none() =>
            {
                TurretPhase::Flick
            }
            TurretPhase::Flick if settled => TurretPhase::Fire,
            TurretPhase::Fire if in_phase >= TURRET_FIRE => TurretPhase::Target,
            TurretPhase::Target if in_phase >= self.target_for.unwrap_or(TURRET_FIRE) => {
                TurretPhase::Cool
            }
            TurretPhase::Cool if self.cooled || in_phase >= TURRET_COOL => TurretPhase::Dismount,
            TurretPhase::Dismount if settled => TurretPhase::Crouch,
            TurretPhase::Crouch if in_phase >= TURRET_SETTLE * 2 => TurretPhase::Uncrouch,
            TurretPhase::Uncrouch if settled => TurretPhase::Strafe,
            TurretPhase::Strafe => {
                let (bearing, d) = turret_bearing(gun_origin, gun_yaw, origin);
                match turret_strafe_stop(bearing, d) {
                    StrafeStop::Outside => TurretPhase::Refused,
                    StrafeStop::OutOfRange => {
                        self.notes.push(format!(
                            "# BROKEN strafe passed {TURRET_REFUSE_RANGE} units at bearing {bearing:.1} before leaving the arc"
                        ));
                        TurretPhase::Refused
                    }
                    StrafeStop::Inside if in_phase >= TURRET_STRAFE => {
                        self.notes
                            .push("# BROKEN strafe never left the arc".to_string());
                        TurretPhase::Refused
                    }
                    StrafeStop::Inside => TurretPhase::Strafe,
                }
            }
            TurretPhase::Refused if in_phase >= TURRET_SETTLE * 2 => TurretPhase::Done,
            p => p,
        };
        self.enter(now, next, snap);
        self.phase == TurretPhase::Done
    }
}

/// The turret fixture: header, notes, then one `[phase <label>]` block per
/// phase, its station first and its cmds and lines interleaved by ms. Named
/// `dm` whatever cs 0 says: retail runs the capture as gametype
/// `probe_turret`, which is `dm` underneath.
pub(super) fn write_turret_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    tp: &TurretProbe,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let (map, gametype) = (key("mapname"), key("g_gametype"));
    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server: a mount, sweep, fire and dismount of a misc_mg42.\n",
        retail_build()
    ));
    out.push_str(&format!("# map {map}, gametype {gametype}\n"));
    match &tp.gun {
        Some(g) => out.push_str(&format!(
            "# turret {} origin={} angles={}\n",
            g.num.map_or("?".to_string(), |n| n.to_string()),
            vec_str(g.origin),
            vec_str(g.angles)
        )),
        None => out.push_str("# turret ? origin=? angles=?\n"),
    }
    out.push_str(&format!(
        "# gunner team={} weapon={}\n",
        join.team, join.weapon
    ));
    out.push_str("# Three shells, the gsc probe first:\n");
    // The probe cannot see the server's own run length, so the header names none.
    out.push_str("#   COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=<past both clients> \\\n");
    out.push_str(
        "#       tools/run_probe.sh client-probes/probe_turret mp_carentan +set probe_teleport 1\n",
    );
    out.push_str(
        "#   cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team axis --probe-secs 180\n",
    );
    out.push_str(
        "#   cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-turret --probe-secs 170\n",
    );
    out.push_str(
        "# The gsc probe puts the gunner 40 units behind the gun facing along it and the axis\n",
    );
    out.push_str("# client 300 units in front facing back.\n");
    out.push_str(&format!(
        "# Phases: aim {s} ms at the gun; mount taps use; yaw sweeps -90..90 and pitch -60..60 off\n\
# the gun, 2 degrees a cmd, each at least {sw} ms; flick turns 60 in one cmd for {s} ms; fire\n\
# holds attack {f} ms at the gun's yaw +{fy}, {fp} down, off the target's line; target holds it\n\
# {f} ms at the axis client; cool waits for the\n\
# cooldown alias or {c} ms; dismount taps use; crouch holds crouch and taps use {s} ms in;\n\
# uncrouch taps use; strafe holds right until the bearing off the gun's back passes {rb}\n\
# degrees inside {rr} units ({st} ms at most); refused faces the gun and taps use {s} ms in.\n\
# refused bearing= and dist= are measured on the first snapshot at or past the refused tap.\n",
        fy = TURRET_FIRE_YAW,
        fp = TURRET_FIRE_PITCH,
        rb = TURRET_REFUSE_BEARING,
        rr = TURRET_REFUSE_RANGE,
        s = TURRET_SETTLE.as_millis(),
        sw = TURRET_SWEEP.as_millis(),
        f = TURRET_FIRE.as_millis(),
        c = TURRET_COOL.as_millis(),
        st = TURRET_STRAFE.as_millis(),
    ));
    out.push_str("# !station is where the phase started; one per phase.\n");
    out.push_str("# !cmd view= is the absolute view asked for (wire shorts, before delta_angles); angles= went on the wire; q= is its message's netchan sequence.\n");
    out.push_str(
        "# !trace is the gunner's playerstate per snapshot; hint is serverCursorHint:Val:String;\n",
    );
    out.push_str(
        "# ct= is its commandTime, the stamp of the last cmd the server ran before that frame.\n",
    );
    out.push_str(
        "# !turret is the gun's entity, on every snapshot where one of its fields moved.\n",
    );
    out.push_str("# !event is every drained event; entity 4294967295 is this client's own playerstate ring.\n");
    out.push_str("# !impact is every EV_BULLET_HIT_SMALL/LARGE temp entity with its origin.\n");
    out.push_str("# !victim is every other player's entity on each fire and target snapshot.\n");
    out.push_str("# !server is every server command, tokens joined, control bytes as \\xNN.\n");
    for n in &tp.notes {
        out.push_str(n);
        out.push('\n');
    }
    turret_phase_blocks(&mut out, tp);
    let path = write_fixture_file(
        &TURRET_FIXTURE_DIR,
        &tagged_name(&format!("{map}-dm-turret"), tag),
        &out,
        overwrite,
    )?;
    println!("turret: wrote {path}");
    Ok(())
}

/// [`write_turret_fixture`]'s `[phase]` blocks; a `!trace` keeps its `!turret` and `!event`
/// lines after it.
fn turret_phase_blocks(out: &mut String, tp: &TurretProbe) {
    let cmds: Vec<(TurretPhase, u128, String)> = tp
        .cmds
        .iter()
        .map(|(p, ms, c, v, q)| {
            let row = format!(
                "!cmd ms={ms} {} view={},{},{} q={q}",
                cmd_fields(c),
                v[0],
                v[1],
                v[2]
            );
            (*p, *ms, row)
        })
        .collect();
    write_phase_blocks(out, &tp.stations, &cmds, &tp.lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::tests::pickup_snap;

    /// The writer's old inline loop, kept as the reference [`turret_phase_blocks`] must match.
    fn old_turret_phase_blocks(tp: &TurretProbe) -> String {
        let mut out = String::new();
        let mut phases: Vec<TurretPhase> = Vec::new();
        for p in tp
            .cmds
            .iter()
            .map(|c| c.0)
            .chain(tp.lines.iter().map(|l| l.0))
        {
            if !phases.contains(&p) {
                phases.push(p);
            }
        }
        phases.sort_by_key(|p| *p as u8);
        for phase in phases {
            out.push_str(&format!("[phase {}]\n", phase.label()));
            for (_, o, v) in tp.stations.iter().filter(|s| s.0 == phase) {
                out.push_str(&format!(
                    "!station origin={} viewangles={}\n",
                    vec_str(*o),
                    vec_str(*v)
                ));
            }
            let mut rows: Vec<(u128, String)> = tp
            .cmds
            .iter()
            .filter(|c| c.0 == phase)
            .map(|(_, ms, c, v, q)| {
                (
                    *ms,
                    format!(
                        "!cmd ms={ms} st={} buttons={} wbuttons={} weapon={} up={} forward={} right={} angles={},{},{} view={},{},{} q={q}",
                        c.server_time, c.buttons, c.wbuttons, c.weapon, c.up, c.forward, c.right,
                        c.angles[0], c.angles[1], c.angles[2], v[0], v[1], v[2]
                    ),
                )
            })
            .chain(
                tp.lines
                    .iter()
                    .filter(|l| l.0 == phase)
                    .map(|(_, ms, l)| (*ms, l.clone())),
            )
            .collect();
            // Stable, so a `!trace` keeps its `!turret` and `!event` lines after it.
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
    fn turret_phase_blocks_match_the_old_loop() {
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
        let tp = TurretProbe {
            cmds: vec![
                (TurretPhase::Fire, 30, c(100, 1), [1, 2, 3], 7),
                (TurretPhase::Aim, 10, c(90, -1), [0, 0, 0], 5),
                (TurretPhase::Fire, 20, c(110, 127), [9, -9, 9], 6),
            ],
            lines: vec![
                (TurretPhase::Cool, 30, "!trace a".into()),
                (TurretPhase::Fire, 30, "!trace b".into()),
                (TurretPhase::Fire, 20, "!event c".into()),
                (TurretPhase::Wait, 5, "!trace d".into()),
                (TurretPhase::Fire, 30, "!event e".into()),
            ],
            stations: vec![
                (TurretPhase::Fire, [1.25, -2.0, 3.0], [0.0, 90.0, 0.0]),
                (TurretPhase::Done, [0.0; 3], [0.0; 3]),
            ],
            ..TurretProbe::default()
        };
        let mut out = String::new();
        turret_phase_blocks(&mut out, &tp);
        assert!(out.contains("[phase "), "{out}");
        assert_eq!(out, old_turret_phase_blocks(&tp));
    }

    #[test]
    fn turret_sweep_steps_two_degrees_a_cmd_and_ends_past_both_edges() {
        let offs: Vec<f32> = (0..)
            .map(turret_sweep_offset)
            .take_while(|o| o.is_some())
            .map(Option::unwrap)
            .collect();
        assert_eq!(offs.first(), Some(&-90.0));
        assert_eq!(offs.last(), Some(&90.0));
        assert!(offs.windows(2).all(|w| (w[1] - w[0] - 2.0).abs() < 1e-4));
    }

    #[test]
    fn turret_probe_finds_the_gun_nearest_the_capture_spot() {
        let ents = "{\n\"classname\" \"misc_mg42\"\n\"origin\" \"-500 1896 175\"\n\"angles\" \"0 90 0\"\n}\n{\n\"classname\" \"misc_mg42\"\n\"origin\" \"1712 1830 8\"\n\"angles\" \"0 180 0\"\n}\n";
        let gun = turret_from_entities(ents).unwrap();
        assert_eq!(gun.origin, [1712.0, 1830.0, 8.0]);
        assert_eq!(gun.yaw, 180.0);
    }

    #[test]
    fn a_turret_trace_line_carries_the_view_lock() {
        let snap = pickup_snap(
            1,
            &[
                ("viewlocked", 1),
                ("viewlocked_entNum", 298),
                ("eFlags", 0xC000),
            ],
            &[],
        );
        let line = turret_trace_line(10, &snap);
        assert!(
            line.contains("viewlocked=1 viewlocked_entNum=298"),
            "{line}"
        );
        assert!(line.contains(&format!("eFlags={}", 0xC000)), "{line}");
        assert!(line.contains(" ct="), "{line}");
    }

    /// The whole script on fake snapshots: use rises once per mount or
    /// dismount phase, attack is held on every `fire` cmd off the target's
    /// line, and a refused tap is noted where it was made, not where the
    /// strafe stopped.
    #[test]
    fn turret_script_taps_use_per_phase_and_holds_attack_through_fire() {
        let gun = [1712.0f32, 1830.0, 8.0];
        let mut tp = TurretProbe {
            gun: Some(TurretGun {
                origin: gun,
                angles: [0.0, 229.0, 0.0],
                yaw: 229.0,
                num: Some(298),
            }),
            ..TurretProbe::default()
        };
        let bits = |v: f32| v.to_bits() as i32;
        let t0 = Instant::now();
        let (mut mounted, mut down) = (false, false);
        let mut rises = Vec::new();
        let mut fire_cmds = (0, 0);
        let mut first_yaw = None;
        let mut fire_view = None;
        // 40 behind the gun; the strafe walks right at 200 units a second and
        // the gunner coasts 25 units a second through refused.
        let (s, c) = 229f32.to_radians().sin_cos();
        let mut lateral = 0.0f32;
        for k in 0..6000u32 {
            let now = t0 + Duration::from_millis(u64::from(k) * 4);
            match tp.phase {
                TurretPhase::Strafe => lateral += 0.8,
                TurretPhase::Refused => lateral += 0.1,
                _ => {}
            }
            let at = [
                gun[0] - 40.0 * c + lateral * s,
                gun[1] - 40.0 * s - lateral * c,
            ];
            let snap = pickup_snap(
                k + 1,
                &[
                    ("origin[0]", bits(at[0])),
                    ("origin[1]", bits(at[1])),
                    ("origin[2]", bits(gun[2])),
                    ("viewlocked", i32::from(mounted)),
                ],
                &[],
            );
            if tp.step(now, &snap) {
                break;
            }
            let cmd = tp.cmd(now);
            tp.record(now, cmd, cmd.angles, 0);
            let pressed = cmd.buttons & BUTTON_USE != 0;
            if pressed && !down {
                rises.push(tp.phase);
                if tp.phase != TurretPhase::Refused {
                    mounted = !mounted;
                }
            }
            down = pressed;
            if tp.phase == TurretPhase::Fire {
                fire_cmds.0 += 1;
                fire_cmds.1 += u32::from(cmd.buttons & net::msg::BUTTON_ATTACK != 0);
                fire_view.get_or_insert((cmd.angles[1], cmd.angles[0]));
            }
            if tp.phase == TurretPhase::Yaw && first_yaw.is_none() {
                first_yaw = Some(cmd.angles[1]);
            }
        }
        assert_eq!(tp.phase, TurretPhase::Done);
        assert_eq!(
            rises,
            [
                TurretPhase::Mount,
                TurretPhase::Dismount,
                TurretPhase::Crouch,
                TurretPhase::Uncrouch,
                TurretPhase::Refused,
            ]
        );
        assert!(
            fire_cmds.0 > 0 && fire_cmds.0 == fire_cmds.1,
            "{fire_cmds:?}"
        );
        assert_eq!(first_yaw, Some(deg_to_short(229.0 - 90.0) & 0xffff));
        assert_eq!(
            fire_view,
            Some((deg_to_short(229.0 + 30.0) & 0xffff, deg_to_short(10.0)))
        );
        assert_eq!(tp.notes.len(), 3, "{:?}", tp.notes);
        assert_eq!(
            tp.notes[0],
            "# BROKEN target: the axis client was never in the snapshot"
        );
        // The strafe stops just past 50 degrees; 500 ms of coasting later the
        // tap stands near 56.
        let bearing: f32 = tp.notes[1]
            .strip_prefix("# refused bearing=")
            .and_then(|r| r.split(' ').next())
            .and_then(|b| b.parse().ok())
            .unwrap_or_else(|| panic!("{:?}", tp.notes));
        assert!((55.0..58.0).contains(&bearing), "{:?}", tp.notes);
        assert_eq!(tp.notes[2], "# REFUSED ok");
    }

    /// The strafe stops outside the arc but inside the use range: 40 behind
    /// is inside, and so is 40 across at 45 degrees; 40 behind and 50 across
    /// (51 degrees) and 80 across (63 degrees, 89 units) are out of the arc,
    /// and 200 across is out of range.
    #[test]
    fn turret_strafe_stops_outside_the_arc_and_inside_the_range() {
        let gun = [1712.0f32, 1830.0, 8.0];
        let yaw = 229.0f32;
        let (s, c) = yaw.to_radians().sin_cos();
        let spot = |behind: f32, across: f32| {
            [
                gun[0] - behind * c + across * s,
                gun[1] - behind * s - across * c,
                gun[2],
            ]
        };
        let stop = |at| {
            let (b, d) = turret_bearing(gun, yaw, at);
            turret_strafe_stop(b, d)
        };
        let (b, d) = turret_bearing(gun, yaw, spot(40.0, 0.0));
        assert!(b.abs() < 1e-3 && (d - 40.0).abs() < 1e-3, "{b} {d}");
        assert_eq!(stop(spot(40.0, 0.0)), StrafeStop::Inside);
        assert_eq!(stop(spot(40.0, 40.0)), StrafeStop::Inside, "45 degrees");
        assert_eq!(stop(spot(40.0, 50.0)), StrafeStop::Outside);
        assert_eq!(stop(spot(40.0, 80.0)), StrafeStop::Outside);
        assert_eq!(stop(spot(40.0, 200.0)), StrafeStop::OutOfRange);
    }

    /// A target that died before its phase is not reported as never seen.
    #[test]
    fn turret_target_note_tells_a_dead_target_from_a_missing_one() {
        let snap = pickup_snap(1, &[], &[]);
        let note = |killed: Option<TurretPhase>, before: bool| {
            let mut tp = TurretProbe {
                phase: TurretPhase::Target,
                target_killed_in: killed,
                target_seen_before: before,
                ..TurretProbe::default()
            };
            tp.enter(Instant::now(), TurretPhase::Cool, &snap);
            tp.notes.join("|")
        };
        assert_eq!(
            note(Some(TurretPhase::Fire), true),
            "# BROKEN target: the axis client was dead through target (obituary in fire)"
        );
        assert_eq!(
            note(None, true),
            "# BROKEN target: the axis client left the snapshot before target"
        );
        assert_eq!(
            note(None, false),
            "# BROKEN target: the axis client was never in the snapshot"
        );
    }
}
