//! `--probe-plant` and `--probe-defuse`: the S&D pair and its fixture.

use super::fixture::{
    PLAYERSTATE_FIXTURE_DIR, cmd_fields, serverinfo_key, tagged_name, vec_str, write_fixture_file,
};
use super::hit::{STEER_AHEAD, Steer};
use super::pvs::{entity_origin, offset_by};
use super::triggers::TriggerBox;
use super::*;

/// Which half of the S&D pair this probe is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SdRole {
    Attacker,
    Defender,
}

impl SdRole {
    /// The fixture name this half writes: `<map>-sd-<role>.txt`.
    fn label(self) -> &'static str {
        match self {
            SdRole::Attacker => "plant-attacker",
            SdRole::Defender => "defuse-defender",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SdPhase {
    /// Both halves: wait past the match-start restart the second join causes.
    Wait,
    /// Walk to the station (the zone centre, or the planted bomb).
    Approach,
    /// Attacker: use held 2 s, then released 1 s (the abort).
    Hold1,
    Release,
    /// Attacker: use held until the plant lands; forward from 2 s to 3.5 s.
    Hold2,
    /// Defender: the view sweep, aim only, one entry per station. Use is
    /// not pressed: sd.gsc's `bomb_think` starts the defuse on the lookat's
    /// first fire and its progress loop reads only `useButtonPressed`, so a
    /// held use would finish the defuse mid-sweep.
    Sweep,
    /// Defender: aimed true, use held until the defuse lands.
    Defuse,
    Watch,
    Done,
}

impl SdPhase {
    pub(super) fn label(self) -> &'static str {
        match self {
            SdPhase::Wait => "wait",
            SdPhase::Approach => "approach",
            SdPhase::Hold1 => "hold1",
            SdPhase::Release => "release",
            SdPhase::Hold2 => "hold2",
            SdPhase::Sweep => "sweep",
            SdPhase::Defuse => "defuse",
            SdPhase::Watch => "watch",
            SdPhase::Done => "done",
        }
    }
}

const SD_WAIT: Duration = Duration::from_secs(12);

const SD_HOLD1: Duration = Duration::from_secs(2);

const SD_RELEASE: Duration = Duration::from_secs(1);

const SD_HOLD2: Duration = Duration::from_secs(9);

const SD_WALK_FROM: Duration = Duration::from_secs(2);

const SD_WALK_TO: Duration = Duration::from_millis(3500);

const SD_DEFUSE_HOLD: Duration = Duration::from_secs(13);

const SD_STATION: Duration = Duration::from_millis(1500);

/// Where the defender's walk stops and the sweep starts. Inside sd.gsc's
/// `check_bomb` 32-unit test, so the defuse icon `bomb_think` creates on a
/// lookat fire survives the frame and every fire reads as `icon=1`; further
/// out `check_bomb` destroys it the same frame and the wire never shows it.
const SD_BOMB_RANGE: f32 = 28.0;

/// An origin jump past this between snapshots is the gsc teleport.
const SD_TELEPORT_JUMP: f32 = 200.0;

/// The sweep starts only once the horizontal speed is under this: `setOrigin`
/// keeps whatever velocity the player had, so a station taken on the landing
/// frame is taken from a spot the next frame has already left.
const SD_SETTLED_SPEED: f32 = 5.0;

const SD_WATCH: Duration = Duration::from_secs(20);

/// How far short of the bombzone the defender waits for the plant: inside the
/// zone it would be standing on the attacker, and its own use press would
/// contest the plant it is here to watch. 200 keeps it on the road the
/// teleport put it on.
const SD_STANDOFF: f32 = 200.0;

/// (yaw, pitch) degrees off the aim at the bomb, one station each.
#[rustfmt::skip]
const SD_SWEEP: [(f32, f32); 15] = [
    (0.0, 0.0), (0.0, -30.0), (0.0, -15.0), (0.0, -8.0), (0.0, 8.0), (0.0, 15.0), (0.0, 30.0),
    (-30.0, 0.0), (-15.0, 0.0), (-8.0, 0.0), (8.0, 0.0), (15.0, 0.0), (30.0, 0.0),
    (0.0, 45.0), (0.0, -45.0),
];

/// One snapshot's line: what the wire says about the link, the bar, the
/// objectives and the icon, beside the offset the sweep was at.
struct SdSample {
    phase: SdPhase,
    elapsed_ms: u128,
    server_time: i32,
    buttons: u8,
    forward: i8,
    pm_type: i32,
    pm_flags: i32,
    eflags: i32,
    ground: i32,
    origin: [f32; 3],
    velocity: [f32; 3],
    viewangles: [f32; 3],
    /// The sweep's (yaw, pitch) offset in degrees; 0,0 outside the sweep.
    offset: (f32, f32),
    /// The pitch the view took off the aim at the bomb, degrees, read out of
    /// the snapshot: the requested offset saturates at the 87.9 degree clamp
    /// when the base aim is steep.
    pitch_applied: f32,
    /// Both HUD arrays, archived then current with a `|` between, as
    /// `type:shader:w:h:fromW:fromH:scaleStart:scaleTime:x:y` per element.
    hud: String,
    /// Block 4 as `i:state:icon:ent:team:x,y,z` per non-empty slot.
    objectives: String,
    /// Whether the defuse or plant icon (a 64x64 shader element) is present.
    icon: bool,
    /// Whether a progress bar (a shader element with `scaleTime` non-zero) is present.
    bar: bool,
}

impl SdSample {
    fn line(&self) -> String {
        format!(
            "!trace ms={} serverTime={} buttons={} forward={} pm_type={} pm_flags={} eFlags={} \
groundEntityNum={} origin={} velocity={} viewangles={} yawOffset={:.1} pitchOffset={:.1} \
pitchApplied={:.1} icon={} bar={} hud={} objectives={}\n",
            self.elapsed_ms,
            self.server_time,
            self.buttons,
            self.forward,
            self.pm_type,
            self.pm_flags,
            self.eflags,
            self.ground,
            vec_str(self.origin),
            vec_str(self.velocity),
            vec_str(self.viewangles),
            self.offset.0,
            self.offset.1,
            self.pitch_applied,
            self.icon as i32,
            self.bar as i32,
            if self.hud.is_empty() { "-" } else { &self.hud },
            if self.objectives.is_empty() {
                "-"
            } else {
                &self.objectives
            },
        )
    }
}

/// Both HUD arrays, archived first then current with a `|` between them (an
/// empty half reads `-`), as the ten fields a plant or a defuse moves. All
/// ten are integer netfields (`HUD_FIELD_BITS`), so the raw word is the value.
fn sd_hud_str(archived: &[net::msg::HudElem], current: &[net::msg::HudElem]) -> String {
    let half = |elems: &[net::msg::HudElem]| {
        let s = sd_hud_elems_str(elems);
        if s.is_empty() { "-".to_string() } else { s }
    };
    format!("{}|{}", half(archived), half(current))
}

fn sd_hud_elems_str(elems: &[net::msg::HudElem]) -> String {
    use net::msg::hud_field as h;
    elems
        .iter()
        .map(|e| {
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
                e.get(h::TYPE),
                e.get(h::SHADER),
                e.get(h::WIDTH),
                e.get(h::HEIGHT),
                e.get(h::FROM_WIDTH),
                e.get(h::FROM_HEIGHT),
                e.get(h::SCALE_START_TIME),
                e.get(h::SCALE_TIME),
                e.get(h::X),
                e.get(h::Y),
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A deleted slot still differs from the default one (`objective_delete`
/// leaves `entNum` at 0x3ff), which is what makes the delete visible.
fn sd_objectives_str(objs: &[net::msg::Objective]) -> String {
    objs.iter()
        .enumerate()
        .filter(|(_, o)| **o != net::msg::Objective::default())
        .map(|(i, o)| {
            format!(
                "{i}:{}:{}:{}:{}:{}",
                o.state,
                o.icon,
                o.ent_num,
                o.team_num,
                vec_str(o.origin_f32()),
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A 64x64 shader element is the plant/defuse icon; a shader element with a
/// running `scaleTime` is the progress bar the tween drives.
fn sd_icon_and_bar(elems: &[net::msg::HudElem]) -> (bool, bool) {
    use net::msg::hud_field as h;
    let icon = elems
        .iter()
        .any(|e| e.get(h::SHADER) != 0 && e.get(h::WIDTH) == 64 && e.get(h::HEIGHT) == 64);
    let bar = elems
        .iter()
        .any(|e| e.get(h::SHADER) != 0 && e.get(h::SCALE_TIME) != 0);
    (icon, bar)
}

/// `--probe-plant` / `--probe-defuse`: one half each of the S&D pair. The
/// attacker walks into `bombzone_A` and holds use through an aborted plant and
/// a real one; the defender waits the plant out, walks to the bomb, sweeps its
/// view across it without pressing use (the `trigger_lookat`'s shape) and then
/// defuses. Both write their own fixture.
pub(super) struct SdProbe {
    role: SdRole,
    pub(super) phase: SdPhase,
    phase_started: Option<Instant>,
    /// The run's clock: when the first snapshot past the join reached `step`.
    settled_at: Option<Instant>,
    /// `bombzone_A`'s submodel bounds, read out of the BSP by `targetname`.
    zone: Option<TriggerBox>,
    world: Option<Box<vcod_common::collision::CollisionWorld>>,
    steer: Steer,
    /// The last snapshot's origin, so a teleport reads as a jump.
    last_origin: Option<[f32; 3]>,
    target: Option<[f32; 3]>,
    bomb: Option<[f32; 3]>,
    aim: Option<(i32, i32)>,
    /// The un-offset pitch of the aim at the bomb, degrees, for `pitchApplied`.
    aim_pitch_deg: Option<f32>,
    /// Whether the approach still pushes forward; the defender stops short.
    advance: bool,
    sweep_index: usize,
    /// When the current sweep station started.
    station_at: Option<Instant>,
    station_pose: Option<([f32; 3], [f32; 3])>,
    /// Every cmd sent from `Hold1`/`Sweep` on, with the server time it carried.
    cmds: Vec<(i32, net::msg::UserCmd)>,
    trace: Vec<SdSample>,
    traced: Option<u32>,
    pub(super) notes: Vec<String>,
    /// Whether objective slot 0 (the bomb) and slot 1 (the zone) have ever
    /// read state 4. A slot reads 0 both before the gametype fills it and
    /// after the plant deletes it, so the edge needs the latch.
    saw_current: [bool; 2],
    /// Slot 0's origin the first time it was seen, which the plant moves.
    slot0_origin: Option<[f32; 3]>,
}

impl SdProbe {
    pub(super) fn new(role: SdRole) -> Self {
        SdProbe {
            role,
            phase: SdPhase::Wait,
            phase_started: None,
            settled_at: None,
            zone: None,
            world: None,
            steer: Steer::default(),
            last_origin: None,
            target: None,
            bomb: None,
            aim: None,
            aim_pitch_deg: None,
            advance: true,
            sweep_index: 0,
            station_at: None,
            station_pose: None,
            cmds: Vec::new(),
            trace: Vec::new(),
            traced: None,
            notes: Vec::new(),
            saw_current: [false; 2],
            slot0_origin: None,
        }
    }

    pub(super) fn running(&self) -> bool {
        self.phase != SdPhase::Done
    }

    /// The sweep's (yaw, pitch) stations at which a trace saw the defuse
    /// icon: the lookat's shape as the wire shows it. A station's first two
    /// traces are skipped, since the previous station's icon is still on the
    /// wire there.
    fn stations_with_icon(&self) -> Vec<(f32, f32)> {
        SD_SWEEP
            .iter()
            .copied()
            .filter(|&st| {
                self.trace
                    .iter()
                    .filter(|t| t.phase == SdPhase::Sweep && t.offset == st)
                    .skip(2)
                    .any(|t| t.icon)
            })
            .collect()
    }

    /// Reads `bombzone_A`'s submodel bounds and the map's collision, the same
    /// way [`TriggerProbe::load_map`] reads the whole trigger belt.
    pub(super) fn load_map(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, map: &str) {
        let Some(fs) = fs else {
            println!("SD: no game data, the walk has nothing to steer at");
            return;
        };
        let Some(bsp) = vcod_common::mapworld::find(fs, map).ok() else {
            println!("SD: cannot load {map}, the walk has nothing to steer at");
            return;
        };
        for block in bsp.entities.split('}') {
            let key = |k: &str| {
                block
                    .split_once(&format!("\"{k}\""))
                    .and_then(|(_, rest)| rest.split('"').nth(1))
            };
            if key("targetname") != Some("bombzone_A") {
                continue;
            }
            let Some(model) = key("model")
                .and_then(|m| m.strip_prefix('*'))
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            let Some(m) = bsp.models.get(model) else {
                continue;
            };
            let at = entity_origin(key("origin"));
            self.zone = Some(TriggerBox {
                model,
                mins: offset_by(m.mins, at),
                maxs: offset_by(m.maxs, at),
            });
        }
        let statics = vcod_common::props::static_models(fs, &bsp.entities);
        self.world = Some(Box::new(
            vcod_common::collision::CollisionWorld::with_statics(&bsp, &statics),
        ));
        match &self.zone {
            Some(z) => println!(
                "SD: bombzone_A is *{} at [{:.0},{:.0},{:.0}] (world)",
                z.model,
                z.centre()[0],
                z.centre()[1],
                z.centre()[2],
            ),
            None => println!("SD: {map} has no bombzone_A, the walk has nothing to steer at"),
        }
    }

    /// The sweep's offset off the aim at the bomb, degrees.
    fn offset(&self) -> (f32, f32) {
        if self.phase != SdPhase::Sweep {
            return (0.0, 0.0);
        }
        SD_SWEEP
            .get(self.sweep_index)
            .copied()
            .unwrap_or((0.0, 0.0))
    }

    pub(super) fn cmd(&self, now: Instant) -> net::msg::UserCmd {
        let mut cmd = net::msg::NULL_USERCMD;
        if let Some((yaw, pitch)) = self.aim {
            cmd.angles[1] = yaw;
            cmd.angles[0] = pitch;
        }
        match self.phase {
            SdPhase::Wait | SdPhase::Release | SdPhase::Watch | SdPhase::Done => {}
            SdPhase::Approach => {
                if self.advance {
                    cmd.forward = 127;
                }
            }
            SdPhase::Hold1 => cmd.buttons |= BUTTON_USE,
            // The sweep can end outside the script's 64-unit defuse test
            // (the slide after the teleport); the hold closes on the bomb
            // under the approach's own stop rule, with the aim already on it.
            SdPhase::Defuse => {
                cmd.buttons |= BUTTON_USE;
                if self.advance {
                    cmd.forward = 127;
                }
            }
            // The walk inside the hold is what a link does to pmove: retail
            // pins a planting player to the bombzone entity.
            SdPhase::Hold2 => {
                cmd.buttons |= BUTTON_USE;
                if let Some(t) = self.phase_started {
                    let d = now.duration_since(t);
                    if d >= SD_WALK_FROM && d < SD_WALK_TO {
                        cmd.forward = 127;
                    }
                }
            }
            SdPhase::Sweep => {
                let (yaw, pitch) = self.offset();
                cmd.angles[1] = (cmd.angles[1] + deg_to_short(yaw)) & 0xffff;
                cmd.angles[0] = (cmd.angles[0] + deg_to_short(pitch)) & 0xffff;
            }
        }
        cmd
    }

    /// Keeps the cmds of the replayable part: a gate stands a client at
    /// `station_pose` and feeds it these.
    pub(super) fn record(&mut self, cmd: net::msg::UserCmd) {
        if matches!(self.phase, SdPhase::Wait | SdPhase::Approach) {
            return;
        }
        self.cmds.push((cmd.server_time, cmd));
    }

    fn enter(&mut self, now: Instant, next: SdPhase, ms: u128) {
        if next == self.phase {
            return;
        }
        println!("SD: {} -> {} at +{ms}ms", self.phase.label(), next.label());
        self.phase = next;
        self.phase_started = Some(now);
        self.station_at = Some(now);
    }

    /// Feeds the newest snapshot in. Returns true once the run is done.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let p = wire();
        let started = *self.settled_at.get_or_insert(now);
        let phase_started = *self.phase_started.get_or_insert(now);
        let ms = now.duration_since(started).as_millis();
        let in_phase = now.duration_since(phase_started);
        let origin = snap.ps.origin(p);
        let velocity = [
            snap.ps.field_f32(p, "velocity[0]"),
            snap.ps.field_f32(p, "velocity[1]"),
            snap.ps.field_f32(p, "velocity[2]"),
        ];
        let h = snap.ps.field_f32(p, "viewHeightCurrent");
        let eye = [
            origin[0],
            origin[1],
            origin[2] + if h > 0.0 { h } else { EYE_HEIGHT },
        ];
        let objs = &snap.ps.arrays.objectives;
        for (i, o) in objs.iter().enumerate().take(2) {
            self.saw_current[i] |= o.state == SD_STATE_CURRENT;
        }
        let slot0 = objs[0].origin_f32();
        // Block 4 reads empty for the first snapshots after a join, so a base
        // latched on the first frame seen is [0,0,0] and the first real
        // update reads as the plant. Only an added slot is a base.
        if self.slot0_origin.is_none() && objs[0].state != 0 {
            self.slot0_origin = Some(slot0);
        }

        // A death ends the run: the script the fixture replays cannot be
        // resumed from a respawn somewhere else.
        if snap.ps.field_i32(p, "pm_type") == PM_DEAD {
            self.notes
                .push(format!("# BROKEN died in {}", self.phase.label()));
            println!("SD: died in {}, ending the run", self.phase.label());
            self.enter(now, SdPhase::Done, ms);
            return true;
        }

        // Two signals, and the move is the one both paths rest on.
        // `saw_current` is never reset, so a restart's momentary delete would
        // read as a plant without the move under it; and a team-scoped
        // objective reaches the other team as state 0 whatever its real
        // state, so an axis defender may never see the 4 the latch needs and
        // slot 0 relocating onto the charge is the whole signal there.
        let zone_deleted = self.saw_current[1] && objs[1].state == 0;
        // The restart's own frames carry a blank block 4, whose slot 0 origin
        // reads 0,0,0 against the latched base; only an added slot is a move.
        let slot0_moved =
            objs[0].state != 0 && self.slot0_origin.is_some_and(|b| dist(slot0, b) > 1.0);
        let planted = zone_deleted && slot0_moved;
        if self.role == SdRole::Defender && self.bomb.is_none() && (planted || slot0_moved) {
            println!(
                "SD: plant seen at +{ms}ms, bomb at [{:.0},{:.0},{:.0}]",
                slot0[0], slot0[1], slot0[2]
            );
            self.bomb = Some(slot0);
            self.target = Some(slot0);
        }

        // The gsc teleport lands mid-walk: a heading chosen and held before
        // it walks the defender straight off the charge, so the steer is
        // dropped here, before this frame's heading, stop test and transition
        // all read the new origin.
        if self.phase == SdPhase::Approach
            && self
                .last_origin
                .is_some_and(|o| dist(origin, o) > SD_TELEPORT_JUMP)
        {
            println!("SD: origin jumped at +{ms}ms, dropping the steer");
            self.steer = Steer::default();
        }
        self.last_origin = Some(origin);

        // The attacker looks level at the zone and the defender at the bomb;
        // a trigger brush's centre is the midpoint of a brush that spans the
        // map's height, so aiming at it would point the view at the sky.
        let aim_point = match (self.role, self.phase) {
            (SdRole::Defender, SdPhase::Sweep | SdPhase::Defuse) => self.bomb,
            _ => self.target.map(|t| [t[0], t[1], eye[2]]),
        };
        if let Some(t) = aim_point {
            let (yaw, pitch) = aim_at(eye, t);
            let yaw = if self.phase == SdPhase::Approach {
                let bearing = yaw as f32 * 360.0 / 65536.0;
                deg_to_short(self.steer.heading(
                    now,
                    self.world.as_deref(),
                    origin,
                    bearing,
                    STEER_AHEAD,
                ))
            } else {
                yaw
            };
            self.aim = Some((yaw & 0xffff, pitch & 0xffff));
            self.aim_pitch_deg = Some(pitch as f32 * 360.0 / 65536.0);
        }
        // The defender stops short of the zone so its own use press does not
        // contest the plant, and stops at arm's length from the bomb.
        self.advance = match (self.role, self.target) {
            (SdRole::Defender, Some(t)) => {
                let stop = if self.bomb.is_some() {
                    SD_BOMB_RANGE
                } else {
                    SD_STANDOFF
                };
                horiz_dist(origin, t) > stop
            }
            _ => true,
        };

        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            let cmd = self.cmd(now);
            // A `newClientHudElem` lands in the archived array, so the bar
            // and the icon are invisible to a reader of `hud_current` alone.
            let hud: Vec<net::msg::HudElem> = snap
                .ps
                .arrays
                .hud_archived
                .iter()
                .chain(snap.ps.arrays.hud_current.iter())
                .copied()
                .collect();
            let (icon, bar) = sd_icon_and_bar(&hud);
            let viewangles = snap.ps.viewangles(p);
            let pitch_applied = match (self.phase, self.aim_pitch_deg) {
                (SdPhase::Sweep, Some(base)) => {
                    (viewangles[0] - base + 180.0).rem_euclid(360.0) - 180.0
                }
                _ => 0.0,
            };
            self.trace.push(SdSample {
                phase: self.phase,
                elapsed_ms: ms,
                server_time: snap.server_time.raw(),
                buttons: cmd.buttons,
                forward: cmd.forward,
                pm_type: snap.ps.field_i32(p, "pm_type"),
                pm_flags: snap.ps.field_i32(p, "pm_flags"),
                eflags: snap.ps.field_i32(p, "eFlags"),
                ground: snap.ps.field_i32(p, "groundEntityNum"),
                origin,
                velocity,
                viewangles,
                offset: self.offset(),
                pitch_applied,
                hud: sd_hud_str(&snap.ps.arrays.hud_archived, &snap.ps.arrays.hud_current),
                objectives: sd_objectives_str(objs),
                icon,
                bar,
            });
        }

        let next = match self.phase {
            SdPhase::Wait if in_phase >= SD_WAIT => {
                self.target = self.zone.as_ref().map(TriggerBox::centre);
                if self.target.is_none() {
                    self.notes
                        .push("# BROKEN no bombzone_A: the walk never had a station".to_string());
                }
                SdPhase::Approach
            }
            SdPhase::Approach => match self.role {
                SdRole::Attacker
                    if self.zone.as_ref().is_some_and(|z| z.holds(origin))
                        || self.target.is_none() =>
                {
                    self.station_pose = Some((origin, snap.ps.viewangles(p)));
                    SdPhase::Hold1
                }
                SdRole::Defender
                    if self
                        .bomb
                        .is_some_and(|b| horiz_dist(origin, b) <= SD_BOMB_RANGE)
                        && velocity[0].hypot(velocity[1]) < SD_SETTLED_SPEED =>
                {
                    self.station_pose = Some((origin, snap.ps.viewangles(p)));
                    self.sweep_index = 0;
                    SdPhase::Sweep
                }
                _ => SdPhase::Approach,
            },
            SdPhase::Hold1 if in_phase >= SD_HOLD1 => SdPhase::Release,
            SdPhase::Release if in_phase >= SD_RELEASE => SdPhase::Hold2,
            SdPhase::Hold2 if in_phase >= SD_HOLD2 || planted => SdPhase::Watch,
            SdPhase::Sweep => {
                if self
                    .station_at
                    .is_some_and(|t| now.duration_since(t) >= SD_STATION)
                {
                    self.sweep_index += 1;
                    self.station_at = Some(now);
                }
                if self.sweep_index >= SD_SWEEP.len() {
                    println!(
                        "SD: sweep stations with icon: {:?}",
                        self.stations_with_icon()
                    );
                    SdPhase::Defuse
                } else {
                    SdPhase::Sweep
                }
            }
            // The defuse deletes the bomb's own slot, which is the edge the
            // hold waits for when it lands before the clock runs out.
            SdPhase::Defuse
                if in_phase >= SD_DEFUSE_HOLD || (self.saw_current[0] && objs[0].state == 0) =>
            {
                SdPhase::Watch
            }
            SdPhase::Watch if in_phase >= SD_WATCH => SdPhase::Done,
            p => p,
        };
        self.enter(now, next, ms);
        self.phase == SdPhase::Done
    }
}

/// `ObjectiveStateIndexFromString`'s `"current"`
/// (`docs/research/cod11-gsc-object-model.md`, the objective record).
const SD_STATE_CURRENT: i32 = 4;

/// The pair's fixture: the header, the notes and one `[phase <name>]` block
/// per phase, each holding its cmds interleaved with its traces by server time.
pub(super) fn write_sd_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    sd: &SdProbe,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let map = key("mapname");
    let gametype = key("g_gametype");
    let role = sd.role.label();

    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server: what a bomb plant and a bomb defuse do to a player.\n",
        retail_build()
    ));
    out.push_str(&format!(
        "# map {map}, gametype sd (served as {gametype}), joined {}, weapon {}, role {}\n",
        join.team,
        join.weapon,
        match sd.role {
            SdRole::Attacker => "attacker",
            SdRole::Defender => "defender",
        },
    ));
    out.push_str("# Three shells, the gsc probe first, then the defender, then the attacker:\n");
    out.push_str("#   COD_LNXDED_HOME=<absolute, no '+'> SECS=420 \\\n");
    out.push_str("#       tools/run_probe.sh client-probes/probe_lookat mp_carentan \\\n");
    out.push_str("#           +set probe_teleport 1\n");
    out.push_str(
        "#   cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-defuse --probe-secs 400\n",
    );
    out.push_str(
        "#   cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-plant --probe-secs 380\n",
    );
    out.push_str(
        "# The server's own games_mp.log carries the lookat fires and isLookingAt's answer;\n",
    );
    out.push_str("# this file is one client's side of the same run.\n");
    out.push_str(
        "# probe_teleport 1 puts both probes on a courtyard spawn beside the zone; without\n",
    );
    out.push_str(
        "# it the walk starts a town away and never arrives on mp_carentan. It also puts\n",
    );
    out.push_str("# the defender 20 units from the charge once it is down, inside the fuse,\n");
    out.push_str("# and the planter back on its spawn, off the defender's sightline; the probe\n");
    out.push_str(
        "# unlinks it first (stock sd.gsc's success branch never does), so the attacker's\n",
    );
    out.push_str("# pm_type 1 -> 0 on the teleport frame is the probe's doing, not the plant's.\n");
    out.push_str(&format!(
        "# Phases: wait {} s past the match-start restart; approach walks at bombzone_A's\n",
        SD_WAIT.as_secs()
    ));
    out.push_str(&format!(
        "# centre; the attacker then holds use {} s and releases {} s (the abort) and holds\n",
        SD_HOLD1.as_secs(),
        SD_RELEASE.as_secs()
    ));
    out.push_str(&format!(
        "# use through a full plant with forward sent from {} ms to {} ms; the defender\n",
        SD_WALK_FROM.as_millis(),
        SD_WALK_TO.as_millis()
    ));
    out.push_str(&format!(
        "# waits the plant out, walks to the bomb and sweeps {} view stations of {} ms with\n",
        SD_SWEEP.len(),
        SD_STATION.as_millis()
    ));
    out.push_str("# use not pressed, then aims true and holds use through the defuse.\n");
    out.push_str(&format!(
        "# station origin={} viewangles={}\n",
        sd.station_pose.map_or("?".to_string(), |(o, _)| vec_str(o)),
        sd.station_pose.map_or("?".to_string(), |(_, a)| vec_str(a)),
    ));
    out.push_str(
        "# ms is since the first snapshot past the join; serverTime is the server's own\n",
    );
    out.push_str("# clock, and both !cmd and !trace carry it so a block reads in order.\n");
    out.push_str("# !cmd is every usercmd sent from the station on: st, the input bits and the\n");
    out.push_str("# three angle words, ANGLE2SHORT.\n");
    out.push_str(
        "# !trace is one line per snapshot: the movement fields, the sweep's offset off\n",
    );
    out.push_str(
        "# the aim at the bomb in degrees (pitchOffset is what was requested, pitchApplied\n",
    );
    out.push_str(
        "# what the 87.9 degree pitch clamp left of it, read off the snapshot's viewangles),\n",
    );
    out.push_str(
        "# whether a 64x64 shader element (the icon) and a shader element with a running\n",
    );
    out.push_str("# scaleTime (the bar) are on the wire, both HUD arrays, archived then current\n");
    out.push_str(
        "# with a | between, as type:shader:w:h:fromW:fromH:scaleStart:scaleTime:x:y per\n",
    );
    out.push_str("# element, and block 4 as i:state:icon:ent:team:x,y,z per non-empty slot.\n");
    for n in &sd.notes {
        out.push_str(n);
        out.push('\n');
    }
    out.push_str(&format!(
        "!observed role={role} reached={} traced={} cmds={} sweep_stations={}\n",
        sd.phase.label(),
        sd.trace.len(),
        sd.cmds.len(),
        sd.sweep_index.min(SD_SWEEP.len()),
    ));

    // A cmd carries no phase of its own; the trace line it follows is what
    // says which one it was sent in.
    let mut phases: Vec<SdPhase> = Vec::new();
    for s in &sd.trace {
        if phases.last() != Some(&s.phase) {
            phases.push(s.phase);
        }
    }
    for phase in &phases {
        out.push_str(&format!("[phase {}]\n", phase.label()));
        let mut lines: Vec<(i32, usize, String)> = sd
            .trace
            .iter()
            .filter(|s| s.phase == *phase)
            .map(|s| (s.server_time, 0, s.line()))
            .collect();
        for (st, c) in &sd.cmds {
            let at = sd
                .trace
                .iter()
                .rev()
                .find(|s| s.server_time <= *st)
                .map(|s| s.phase);
            if at != Some(*phase) {
                continue;
            }
            lines.push((
                *st,
                1,
                // `st` is the cmd's own server_time (see `cmds`), which cmd_fields prints.
                format!("!cmd {}\n", cmd_fields(c)),
            ));
        }
        lines.sort_by_key(|(st, tag, _)| (*st, *tag));
        for (_, _, l) in lines {
            out.push_str(&l);
        }
    }

    let path = write_fixture_file(
        PLAYERSTATE_FIXTURE_DIR,
        &tagged_name(&format!("{map}-sd-{role}"), tag),
        &out,
        overwrite,
    )?;
    println!("sd: {role} -> {path}");
    Ok(())
}
