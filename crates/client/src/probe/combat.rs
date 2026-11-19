//! `--save-combat`, `--save-ads`, `--probe-sway`, `--save-lean`, `--save-grenade`: the combat
//! scripts and their fixture.

use super::fixture::{
    PLAYERSTATE_FIXTURE_DIR, serverinfo_key, tagged_name, vec_exact, vec_str, write_fixture_file,
};
use super::hit::EV_BULLET_HIT_LARGE;
use super::*;

/// The stock frag, the only grenade in a spawn loadout.
pub(super) const FRAG_WEAPON: &str = "fraggrenade_mp";

/// A weapon's `cmd.weapon` byte: its slot in configstring 7, which is 1-based
/// (docs/protocol-1.1.md, "Configstring 7"), and the name that slot holds.
/// `None` when the list has no such weapon.
pub(super) fn weapon_cs_index(configstrings: &[String], name: &str) -> Option<(u8, String)> {
    let list = crate::entities::split_weapon_list(configstrings.get(7)?);
    let i = list.iter().position(|w| w == name)?;
    Some((i as u8 + 1, list[i].clone()))
}

/// `weaponstate` the capture keys off: the weapon is ready to take an input,
/// and it is firing (`docs/research/cod11-combat.md` 1.1).
pub(super) const WEAPONSTATE_READY: i32 = vcod_common::pmove::weapon::WEAPON_READY as i32;

const WEAPONSTATE_FIRING: i32 = vcod_common::pmove::weapon::WEAPON_FIRING as i32;

/// A key a step taps rather than holds. The stock rifle is semi-automatic, so
/// a held fire bit fires once and then nothing: every shot needs its own
/// release edge, and reload is a press for the same reason.
#[derive(Clone, Copy, Default)]
struct Pulse {
    buttons: u8,
    wbuttons: u8,
    /// Taps the step sends; after the last one it holds its base input alone.
    count: u32,
}

/// How long a tap is held down, and the floor on how often one starts: two
/// frames down, six up at the probe's 60 Hz send rate, which is what fires
/// cleanly on retail. The gap is raised to clear the weapon file's `fireTime`
/// when that is longer, so a tap edge never races the shot before it;
/// m1carbine_mp's is 0.135 s, past the floor.
pub(super) const PULSE_HOLD: Duration = Duration::from_millis(32);

pub(super) const PULSE_PERIOD: Duration = Duration::from_millis(128);

pub(super) const PULSE_MARGIN: Duration = Duration::from_millis(48);

/// The weapon a step's switch asks for. Retail reads a `cmd.weapon` that
/// differs from `ps.weapon` as the request, and its pickup half reads the byte
/// again on the frame the putaway ends (docs/research/cod11-combat.md, 1.8),
/// so the index rides every cmd until `ps.weapon` carries it; see
/// [`WeaponSwitch`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum SwitchTo {
    /// The frag, by its configstring 7 index.
    Frag,
    /// Back to the rifle the join chose.
    Joined,
}

/// A weapon switch in flight, shared by both instruments. Retail's pickup half
/// reads `cmd.weapon` again on the frame the putaway ends
/// (docs/research/cod11-combat.md, 1.8), so the index has to ride every cmd
/// until `ps.weapon` carries it: a byte reverted before then leaves the old
/// weapon in hand, which is what a first retail capture measured.
#[derive(Default)]
pub(super) struct WeaponSwitch {
    /// The index every cmd carries until the playerstate agrees.
    pending: Option<u8>,
    /// The current step or phase has had its turn to ask.
    asked: bool,
}

impl WeaponSwitch {
    /// Asks for `idx`, at most once per step or phase. Index 0 means the
    /// weapon is not in configstring 7 and is refused. Returns whether the
    /// ask went out, for the caller's log line.
    pub(super) fn ask(&mut self, idx: u8) -> bool {
        if self.asked {
            return false;
        }
        self.asked = true;
        self.pending = (idx != 0).then_some(idx);
        self.pending.is_some()
    }

    /// Lets the next step ask. A hold already in flight survives, since a drop
    /// and a raise together outlast the step that asked for them.
    pub(super) fn rearm(&mut self) {
        self.asked = false;
    }

    /// Drops the hold as well, for a phase that starts its switch over.
    pub(super) fn reset(&mut self) {
        *self = WeaponSwitch::default();
    }

    /// The byte this cmd carries, or `None` to follow `ps.weapon`.
    pub(super) fn byte(&mut self, ps_weapon: u8) -> Option<u8> {
        if self.pending == Some(ps_weapon) {
            self.pending = None;
        }
        self.pending
    }
}

/// One eType-4 missile as a snapshot carries it. Retail's explode flips the
/// missile's own `eType` to 0 and adds event 178 on its ring before freeing it
/// (docs/research/cod11-combat.md, section 13), so an entity is recorded from
/// its first missile snapshot until it leaves the wire, not while it reads 4.
pub(super) struct MissileSample {
    num: u32,
    e_type: i32,
    e_flags: i32,
    pos: vcod_common::net::trajectory::Trajectory,
    apos: vcod_common::net::trajectory::Trajectory,
    weapon: i32,
    index: i32,
    events: [i32; 4],
    event_parms: [i32; 4],
    event_sequence: i32,
    /// `pos` evaluated at the snapshot's own `serverTime`.
    origin: [f32; 3],
}

pub(super) fn traj_str(t: &vcod_common::net::trajectory::Trajectory) -> String {
    format!(
        "{},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}",
        t.tr_type, t.tr_time, t.base.x, t.base.y, t.base.z, t.delta.x, t.delta.y, t.delta.z
    )
}

impl MissileSample {
    pub(super) fn line(&self, ms: u128) -> String {
        format!(
            "!missile ms={ms} num={} eType={} eFlags={} pos={} apos={} weapon={} index={} \
events={},{},{},{} eventParms={},{},{},{} eventSequence={} origin={}\n",
            self.num,
            self.e_type,
            self.e_flags,
            traj_str(&self.pos),
            traj_str(&self.apos),
            self.weapon,
            self.index,
            self.events[0],
            self.events[1],
            self.events[2],
            self.events[3],
            self.event_parms[0],
            self.event_parms[1],
            self.event_parms[2],
            self.event_parms[3],
            self.event_sequence,
            vec_str(self.origin),
        )
    }
}

/// Which entities of a run are missiles: every one that reads eType 4 now, or
/// read it in an earlier snapshot and has not left the wire since.
#[derive(Default)]
pub(super) struct MissileWatch {
    seen: std::collections::BTreeSet<u32>,
}

impl MissileWatch {
    pub(super) fn sample(&mut self, snap: &net::snapshot::Snapshot) -> Vec<MissileSample> {
        let p = wire();
        self.seen.retain(|n| snap.entities.contains_key(n));
        for (&n, e) in &snap.entities {
            if e.field_i32(p, "eType") == crate::entities::ET_MISSILE {
                self.seen.insert(n);
            }
        }
        self.seen
            .iter()
            .filter_map(|&num| {
                let e = snap.entities.get(&num)?;
                let f = |n: &str| e.field_i32(p, n);
                let pos = vcod_common::net::trajectory::Trajectory::read(e, p, "pos");
                Some(MissileSample {
                    num,
                    e_type: f("eType"),
                    e_flags: f("eFlags"),
                    origin: pos.evaluate(snap.server_time).into(),
                    pos,
                    apos: vcod_common::net::trajectory::Trajectory::read(e, p, "apos"),
                    weapon: f("weapon"),
                    index: f("index"),
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
                    event_sequence: f("eventSequence"),
                })
            })
            .collect()
    }
}

/// One labelled step of the combat script: `base` held for `dur`, with
/// `pulse`'s bits tapped on top of it.
pub(super) struct CombatStep {
    pub(super) label: &'static str,
    base: net::msg::UserCmd,
    pulse: Pulse,
    /// Bits held down for the first of the step rather than tapped: a grenade
    /// is cooked by holding the trigger, and the throw is the release.
    press: u8,
    press_for: Duration,
    /// A `cmd.weapon` the step asks for, and how far into it the ask goes
    /// out. The byte is then held by [`WeaponSwitch`], not sent once.
    switch: Option<(Duration, SwitchTo)>,
    dur: Duration,
    /// Whether the step walks. A walking step is steered by [`StallTurn`], so
    /// where it ends up is not reproducible and a gate cannot replay it.
    walks: bool,
    /// Whether the step's clock waits for `weaponstate` to read ready. A step
    /// that taps has to: the weapon it inherits may still be busy, and a
    /// reload that outlives its step recorded a reload under a fire label.
    wait_ready: bool,
    /// Set to the weapon file's `reloadTime` plus a margin once the join names
    /// the weapon, so the hold is not a guess about one rifle's timings.
    from_reload_time: bool,
    /// How often a tap starts; [`PULSE_PERIOD`] until the weapon file raises it.
    period: Duration,
}

impl Default for CombatStep {
    fn default() -> Self {
        CombatStep {
            label: "",
            base: net::msg::NULL_USERCMD,
            pulse: Pulse::default(),
            press: 0,
            press_for: Duration::ZERO,
            switch: None,
            dur: Duration::ZERO,
            walks: false,
            wait_ready: false,
            from_reload_time: false,
            period: PULSE_PERIOD,
        }
    }
}

impl CombatStep {
    /// What the step sends `elapsed` into itself: its held input, with the
    /// pulsed bits down for the first [`PULSE_HOLD`] of each `period` until
    /// `count` taps have gone out.
    fn cmd_at(&self, elapsed: Duration) -> net::msg::UserCmd {
        let mut cmd = self.base;
        let ms = elapsed.as_millis();
        if ms / self.period.as_millis() < self.pulse.count as u128
            && ms % self.period.as_millis() < PULSE_HOLD.as_millis()
        {
            cmd.buttons |= self.pulse.buttons;
            cmd.wbuttons |= self.pulse.wbuttons;
        }
        if elapsed < self.press_for {
            cmd.buttons |= self.press;
        }
        cmd
    }
}

/// The steps, each starting from the one before. The two idles bracket the
/// shooting so the capture shows what the channels return to, and the stances
/// are separate steps because the animscript's fire clauses are per stance.
fn combat_script() -> Vec<CombatStep> {
    use net::msg::{NULL_USERCMD, WBUTTON_CROUCH, WBUTTON_PRONE, WBUTTON_RELOAD};
    let hold = |label, base, ms| CombatStep {
        label,
        base,
        dur: Duration::from_millis(ms),
        ..CombatStep::default()
    };
    let fire = |label, base, shots, ms| CombatStep {
        label,
        base,
        pulse: Pulse {
            buttons: BUTTON_ATTACK,
            wbuttons: 0,
            count: shots,
        },
        dur: Duration::from_millis(ms),
        wait_ready: true,
        ..CombatStep::default()
    };
    vec![
        // First, so two probes spawned across a map close some of the distance
        // between them before anyone shoots. Not a gate case: the stall
        // response steers it.
        CombatStep {
            label: "advance",
            base: net::msg::UserCmd {
                forward: 127,
                ..NULL_USERCMD
            },
            dur: Duration::from_millis(6000),
            walks: true,
            ..CombatStep::default()
        },
        hold("idle", NULL_USERCMD, 1500),
        fire("single_shot", NULL_USERCMD, 1, 1500),
        // More shots than the ring has slots, which is what shows how it wraps
        // and what `eventSequence` does across the wrap.
        fire("sustained_fire", NULL_USERCMD, 6, 2500),
        CombatStep {
            label: "reload",
            pulse: Pulse {
                buttons: 0,
                wbuttons: WBUTTON_RELOAD,
                count: 1,
            },
            dur: RELOAD_HOLD_FALLBACK,
            wait_ready: true,
            from_reload_time: true,
            ..CombatStep::default()
        },
        fire(
            "crouch_fire",
            net::msg::UserCmd {
                wbuttons: WBUTTON_CROUCH,
                up: -127,
                ..NULL_USERCMD
            },
            3,
            2500,
        ),
        // Standing between the two lowered stances: a prone taken straight out
        // of a crouch was refused in one motion capture and taken in another.
        hold("stand_between", NULL_USERCMD, 1500),
        fire(
            "prone_fire",
            net::msg::UserCmd {
                wbuttons: WBUTTON_PRONE,
                up: -127,
                ..NULL_USERCMD
            },
            3,
            3000,
        ),
        hold("idle_after", NULL_USERCMD, 3000),
    ]
}

/// The `--save-ads` steps: what the sight fraction and the spread counter do
/// under each input, every one standing still so a gate can replay it. The
/// sight is held, released, held through a reload, held through a shot and
/// held while walking; the spread counter is kicked by a hip shot and by a
/// walk and then left to decay. What the two captures measured is in
/// docs/research/cod11-combat.md, 1.13 and 2.1.
fn ads_script() -> Vec<CombatStep> {
    use net::msg::{BUTTON_ADS, NULL_USERCMD, WBUTTON_RELOAD};
    let hold = |label, base, ms| CombatStep {
        label,
        base,
        dur: Duration::from_millis(ms),
        ..CombatStep::default()
    };
    let fire = |label, base, shots, ms| CombatStep {
        label,
        base,
        pulse: Pulse {
            buttons: BUTTON_ATTACK,
            wbuttons: 0,
            count: shots,
        },
        dur: Duration::from_millis(ms),
        wait_ready: true,
        ..CombatStep::default()
    };
    let sight = net::msg::UserCmd {
        buttons: BUTTON_ADS,
        ..NULL_USERCMD
    };
    let walk = net::msg::UserCmd {
        forward: 127,
        ..NULL_USERCMD
    };
    let sight_walk = net::msg::UserCmd {
        buttons: BUTTON_ADS,
        forward: 127,
        ..NULL_USERCMD
    };
    vec![
        hold("idle", NULL_USERCMD, 1500),
        // The ramp up over adsTransInTime, then the settled sight.
        hold("ads_in", sight, 1500),
        // The ramp down over adsTransOutTime.
        hold("ads_out", NULL_USERCMD, 1500),
        // One hip shot: hipSpreadFireAdd onto the counter, then the decay. It
        // is also what empties a round, since a full clip refuses the reload.
        fire("hip_shot", NULL_USERCMD, 1, 1500),
        // The sight asked for through a whole reload: the fraction is refused
        // until adsReloadTransTime is left of it (1.13).
        CombatStep {
            label: "ads_reload",
            base: sight,
            pulse: Pulse {
                buttons: 0,
                wbuttons: WBUTTON_RELOAD,
                count: 1,
            },
            dur: RELOAD_HOLD_FALLBACK,
            wait_ready: true,
            from_reload_time: true,
            ..CombatStep::default()
        },
        hold("ads_release", NULL_USERCMD, 1000),
        // One shot down the sight: the add is skipped at a fraction of 1.
        fire("ads_shot", sight, 1, 1500),
        hold("ads_release_2", NULL_USERCMD, 1000),
        // A walk saturates the counter; standing still lets it decay.
        hold("walk", walk, 1500),
        hold("stop", NULL_USERCMD, 1500),
        // Walking down the sight: the add is skipped, the 0x80 flag is on.
        hold("ads_walk", sight_walk, 1500),
        hold("idle_after", NULL_USERCMD, 1500),
    ]
}

/// The `--probe-sway` steps: the sight raised and held, then eight shots
/// tapped down it standing still, spaced so a bolt action has rechambered.
/// Where each lands against the raw view is the sway (combat doc, 15).
fn sway_script() -> Vec<CombatStep> {
    use net::msg::{BUTTON_ADS, NULL_USERCMD};
    let sight = net::msg::UserCmd {
        buttons: BUTTON_ADS,
        ..NULL_USERCMD
    };
    let mut steps = vec![
        CombatStep {
            label: "idle",
            base: NULL_USERCMD,
            dur: Duration::from_millis(1000),
            ..CombatStep::default()
        },
        CombatStep {
            label: "ads_in",
            base: sight,
            dur: Duration::from_millis(1500),
            ..CombatStep::default()
        },
    ];
    for label in [
        "scope_shot_1",
        "scope_shot_2",
        "scope_shot_3",
        "scope_shot_4",
        "scope_shot_5",
        "scope_shot_6",
        "scope_shot_7",
        "scope_shot_8",
    ] {
        steps.push(CombatStep {
            label,
            base: sight,
            pulse: Pulse {
                buttons: BUTTON_ATTACK,
                wbuttons: 0,
                count: 1,
            },
            dur: Duration::from_millis(2500),
            wait_ready: true,
            ..CombatStep::default()
        });
    }
    steps
}

/// The `--save-lean` steps: the sight raised, then scoped taps level,
/// leaning left, leaning right and crouched leaning left, each lean held
/// long enough for `leanf` to settle before its taps. Every step holds
/// still, so a gate replays it from the header's spot; where each round
/// lands against the swayed aim is the leaned muzzle (combat doc 2.1).
fn lean_script() -> Vec<CombatStep> {
    use net::msg::{
        BUTTON_ADS, NULL_USERCMD, WBUTTON_CROUCH, WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT,
    };
    let held = |wbuttons, up| net::msg::UserCmd {
        buttons: BUTTON_ADS,
        wbuttons,
        up,
        ..NULL_USERCMD
    };
    let hold = |label, base, ms| CombatStep {
        label,
        base,
        dur: Duration::from_millis(ms),
        ..CombatStep::default()
    };
    let tap = |label, base| CombatStep {
        label,
        base,
        pulse: Pulse {
            buttons: BUTTON_ATTACK,
            wbuttons: 0,
            count: 1,
        },
        dur: Duration::from_millis(2500),
        wait_ready: true,
        ..CombatStep::default()
    };
    let level = held(0, 0);
    let left = held(WBUTTON_LEAN_LEFT, 0);
    let right = held(WBUTTON_LEAN_RIGHT, 0);
    let crouch_left = held(WBUTTON_LEAN_LEFT | WBUTTON_CROUCH, -127);
    vec![
        hold("idle", NULL_USERCMD, 1000),
        hold("ads_in", level, 1500),
        tap("scope_level", level),
        hold("lean_left", left, 1500),
        tap("scope_left_1", left),
        tap("scope_left_2", left),
        // Straight through to the other side: the ramp crosses zero.
        hold("lean_right", right, 1500),
        tap("scope_right_1", right),
        tap("scope_right_2", right),
        hold("lean_off", level, 1500),
        tap("scope_unleaned", level),
        // Long enough for the idle sway's crouch factor to ease in (0.5 a
        // second from 1 to idleCrouchFactor).
        hold("crouch_lean_left", crouch_left, 2500),
        tap("crouch_scope_left", crouch_left),
        hold("idle_after", NULL_USERCMD, 1500),
    ]
}

/// The `--save-grenade` steps, every one standing still so a gate can replay
/// it: one melee swing on the rifle, then the frag through a cook and a
/// release, a cook held past the pin, a cook cancelled by a weapon switch and
/// a throw aimed at the ground. What the capture measured is in
/// docs/research/cod11-combat.md, sections 6 and 11 to 14.
fn grenade_script() -> Vec<CombatStep> {
    use net::msg::{BUTTON_MELEE, NULL_USERCMD};
    let hold = |label, ms| CombatStep {
        label,
        dur: Duration::from_millis(ms),
        ..CombatStep::default()
    };
    // The trigger held for `press_ms` and released; the release is the throw.
    let cook = |label, press_ms, ms| CombatStep {
        label,
        press: BUTTON_ATTACK,
        press_for: Duration::from_millis(press_ms),
        dur: Duration::from_millis(ms),
        ..CombatStep::default()
    };
    vec![
        // The rifle's melee, before any switch: the swing, its anims and the
        // temp entity it spawns.
        CombatStep {
            label: "melee_tap",
            press: BUTTON_MELEE,
            press_for: Duration::from_millis(100),
            dur: Duration::from_millis(1500),
            ..CombatStep::default()
        },
        // Long enough for the rifle's drop and the frag's raise: the byte is
        // held until `ps.weapon` reads the frag, and the cook after this step
        // has to start with it already in hand.
        CombatStep {
            label: "to_frag",
            switch: Some((Duration::ZERO, SwitchTo::Frag)),
            dur: Duration::from_millis(3000),
            ..CombatStep::default()
        },
        // A one-second cook, then the release: the fuse the missile inherits
        // is what is left of `grenadeTimeLeft`.
        cook("cook_release", 1000, 6000),
        // Held past the pin: retail pins `grenadeTimeLeft` at 50 and throws
        // the grenade itself, with the trigger still down.
        cook("pin_out", 5000, 7000),
        // A switch away mid-cook, which is the one path that puts the pin back
        // in: no missile should come out of this step.
        CombatStep {
            label: "cancel",
            press: BUTTON_ATTACK,
            press_for: Duration::from_millis(3000),
            switch: Some((Duration::from_millis(300), SwitchTo::Joined)),
            dur: Duration::from_millis(3000),
            ..CombatStep::default()
        },
        CombatStep {
            label: "to_frag_2",
            switch: Some((Duration::ZERO, SwitchTo::Frag)),
            dur: Duration::from_millis(3000),
            ..CombatStep::default()
        },
        // Aimed down, so the missile hits the floor a few units out and its
        // bounces and its explode are all inside the step.
        CombatStep {
            label: "throw_down",
            base: net::msg::UserCmd {
                angles: [deg_to_short(45.0) & 0xffff, 0, 0],
                ..NULL_USERCMD
            },
            press: BUTTON_ATTACK,
            press_for: Duration::from_millis(200),
            dur: Duration::from_millis(8000),
            ..CombatStep::default()
        },
        hold("idle_after", 1500),
    ]
}

/// One snapshot's worth of the channels a shot moves.
struct CombatSample {
    label: &'static str,
    elapsed_ms: u128,
    server_time: i32,
    /// `ps.commandTime`: the last cmd the snapshot's frame ran, which says
    /// which of the logged cmds reached it.
    command_time: i32,
    /// The input in flight when the snapshot arrived, so a trace line can be
    /// lined up with the tap that produced it.
    buttons: u8,
    wbuttons: u8,
    weaponstate: i32,
    weap_anim: i32,
    legs_anim: i32,
    torso_anim: i32,
    event_sequence: i32,
    events: [i32; 4],
    /// `ps.weapon`, which is what the probe echoes into every `cmd.weapon`.
    /// Retail reads a byte that differs from it as a request to holster
    /// (docs/research/cod11-combat.md, section 1.8), so a replay of this
    /// capture has to send the same one.
    weapon: i32,
    /// The sight fraction and the spread counter (combat doc, 1.13 and 2.1).
    weapon_pos_frac: f32,
    aim_spread_scale: f32,
    /// What is left of the cook, ms, and the weapon machine's own delay
    /// (combat doc, 6 and 1.3).
    grenade_time_left: i32,
    weapon_delay: i32,
    /// `pm_flags` (0x20 is the gated ADS bit, 0x80 the ADS walk) and
    /// `groundEntityNum`: the two inputs the sight ramp is gated on, so a
    /// fraction that leaves 1.0 mid-hold can be read back to its cause.
    pm_flags: i32,
    ground_entity: i32,
    /// The view the snapshot carries, which a client predicts against.
    viewangles: [f32; 3],
    /// `ps.eFlags`, for 0x400, the bit `PmoveSingle` sets under a held
    /// trigger (combat doc 16.5).
    e_flags: i32,
    /// Every missile on the wire in this snapshot.
    missiles: Vec<MissileSample>,
    /// `ps.leanf`, the lean fraction the muzzle is offset by (combat doc 2.1).
    leanf: f32,
    /// One `!impact` line per bullet-impact temp entity in this snapshot.
    impacts: Vec<String>,
}

/// What a step turned out to be: kept beside the playerstate it ended at, so
/// the fixture can say on its face whether a step labelled fire ever fired.
struct StepResult {
    label: &'static str,
    /// The step ever observed `weaponstate` firing. False on a fire step is a
    /// broken capture, not a fact about retail.
    fired: bool,
    /// Snapshots where `weaponstate` rose into firing. A shot shorter than the
    /// gap between snapshots can slip through, which is what `seq_delta` is
    /// the check on.
    shots_seen: u32,
    /// `eventSequence` gained over the step. 8 bits on the wire, so it wraps.
    seq_delta: i32,
    /// How long the step waited for the weapon to read ready before its clock
    /// started.
    waited_ms: u128,
    /// The step was restarted because the weapon went busy under it.
    retried: bool,
    /// The playerstate the step ended at, for the fields a trace line omits.
    fields: Vec<i32>,
}

/// Walks [`combat_script`] and keeps every snapshot along the way. A settled
/// sample cannot hold a shot (the event ring has four slots and overwrites
/// as it goes), so this mode records the trace itself and the settled
/// playerstate only as the step's tail.
pub(super) struct CombatProbe {
    pub(super) steps: Vec<CombatStep>,
    /// Which script runs, which is also the fixture's suffix and the flag
    /// the header names: `combat` or `ads`.
    kind: &'static str,
    pub(super) idx: usize,
    /// Set when the current step's clock starts, which a `wait_ready` step
    /// defers until the weapon reads ready.
    started: Option<Instant>,
    /// When the current step began waiting for that, and since when the weapon
    /// has read ready without interruption.
    waiting_since: Option<Instant>,
    ready_since: Option<Instant>,
    /// Steps already restarted once, so a weapon that goes busy every time
    /// costs one retry and not the run.
    retried: Vec<&'static str>,
    trace: Vec<CombatSample>,
    results: Vec<StepResult>,
    done: bool,
    /// Newest snapshot already recorded, so the trace runs per snapshot rather
    /// than per loop iteration.
    traced: Option<u32>,
    /// The weapon file has been read once; a second look would reread it every
    /// frame.
    pub(super) weapon_read: bool,
    pub(super) spawn_yaw: ViewHold,
    pub(super) stall: StallTurn,
    /// Configstring 7 indexes, 1-based, of the frag and of the weapon the join
    /// chose; 0 until the list is read.
    frag_index: u8,
    joined_index: u8,
    /// The frag's name, for the fixture header.
    frag_name: String,
    switch: WeaponSwitch,
    missiles: MissileWatch,
    /// The player's origin and view at the first step, which is the spot a
    /// replay of this capture has to throw from.
    spawn_pose: Option<([f32; 3], [f32; 3])>,
    /// `--probe-sway` only: the wire yaw of the longest clear sightline from
    /// the spawn, so the impacts land far enough for the sway to be read off
    /// their whole-unit origins. `None` holds the spawn view.
    pub(super) aim_yaw: Option<i32>,
    /// Every cmd sent while a step was current, its wait for the weapon
    /// included, so a replay can run retail's cmds frame for frame, a
    /// stall turn's included. The first step's also holds the cmds sent
    /// before it started that its first snapshot's `commandTime` had not
    /// reached.
    sent: Vec<(&'static str, SentCmd)>,
    /// The last cmds sent before the first step started.
    before: Vec<SentCmd>,
    /// The newest snapshot's `serverTime` when each step became current.
    start_times: Vec<(&'static str, i32)>,
}

impl Default for CombatProbe {
    fn default() -> Self {
        Self {
            steps: combat_script(),
            kind: "combat",
            idx: 0,
            started: None,
            waiting_since: None,
            ready_since: None,
            retried: Vec::new(),
            trace: Vec::new(),
            results: Vec::new(),
            done: false,
            traced: None,
            weapon_read: false,
            spawn_yaw: ViewHold::default(),
            stall: StallTurn::default(),
            frag_index: 0,
            joined_index: 0,
            frag_name: String::new(),
            switch: WeaponSwitch::default(),
            missiles: MissileWatch::default(),
            spawn_pose: None,
            aim_yaw: None,
            sent: Vec::new(),
            before: Vec::new(),
            start_times: Vec::new(),
        }
    }
}

/// The yaw, as a wire word, of the longest sightline from `eye` that still
/// ends on something within 4000 units, scanned two degrees at a time at a
/// level pitch. A sway run points its shots down it: at 44 units, which is
/// what one carentan spawn faces, a whole-unit impact origin is 1.3 degrees
/// wide and the sway is 0.45.
pub(super) fn longest_sightline(
    world: &vcod_common::collision::CollisionWorld,
    eye: [f32; 3],
) -> i32 {
    let start = glam::Vec3::from(eye);
    let mut best = (0.0f32, 0.0f32);
    for step in 0..180 {
        let yaw = step as f32 * 2.0;
        let (s, c) = yaw.to_radians().sin_cos();
        let end = start + glam::Vec3::new(c, s, 0.0) * 4000.0;
        let t = world.shot_trace(start, end);
        if t.fraction < 1.0 && t.fraction > best.1 {
            best = (yaw, t.fraction);
        }
    }
    println!(
        "SWAY: aiming at yaw {} where the wall is {:.0} units off",
        best.0,
        best.1 * 4000.0
    );
    deg_to_short(best.0) & 0xffff
}

/// The yaw, as a wire word, `--save-lean` shoots along: a level sightline
/// that ends on something 250 to 700 units out, as near 400 as the spawn
/// has, with 24 units clear either side of the eye so neither lean is cut
/// short by a wall. Close enough that the sway's model error is a fraction
/// of a unit at the impact, far enough that the lean's 20 units are not
/// lost to the impact's whole-unit origin.
pub(super) fn lean_sightline(world: &vcod_common::collision::CollisionWorld, eye: [f32; 3]) -> i32 {
    let start = glam::Vec3::from(eye);
    let mut best: Option<(f32, f32)> = None;
    for step in 0..180 {
        let yaw = step as f32 * 2.0;
        let (s, c) = yaw.to_radians().sin_cos();
        let t = world.shot_trace(start, start + glam::Vec3::new(c, s, 0.0) * 4000.0);
        let dist = t.fraction * 4000.0;
        if !(250.0..=700.0).contains(&dist) {
            continue;
        }
        let side = glam::Vec3::new(s, -c, 0.0) * 24.0;
        if world.shot_trace(start, start + side).fraction < 1.0
            || world.shot_trace(start, start - side).fraction < 1.0
        {
            continue;
        }
        if best.is_none_or(|(_, d)| (dist - 400.0).abs() < (d - 400.0).abs()) {
            best = Some((yaw, dist));
        }
    }
    let (yaw, dist) = best.unwrap_or((0.0, 0.0));
    println!("LEAN: aiming at yaw {yaw} where the wall is {dist:.0} units off");
    deg_to_short(yaw) & 0xffff
}

impl CombatProbe {
    /// The `--save-ads` capture: the same machine running [`ads_script`].
    pub(super) fn ads() -> Self {
        Self {
            steps: ads_script(),
            kind: "ads",
            ..Self::default()
        }
    }

    /// The `--probe-sway` measurement: the same machine running
    /// [`sway_script`].
    pub(super) fn sway() -> Self {
        Self {
            steps: sway_script(),
            kind: "sway",
            ..Self::default()
        }
    }

    /// The `--save-lean` capture: the same machine running [`lean_script`].
    pub(super) fn lean() -> Self {
        Self {
            steps: lean_script(),
            kind: "lean",
            ..Self::default()
        }
    }

    /// The `--save-grenade` capture: the same machine running
    /// [`grenade_script`].
    pub(super) fn grenade() -> Self {
        Self {
            steps: grenade_script(),
            kind: "grenade",
            ..Self::default()
        }
    }

    /// Whether the step in flight declares itself a walk, which is what
    /// [`StallTurn`] is allowed to steer.
    pub(super) fn step_walks(&self) -> bool {
        self.steps.get(self.idx).is_some_and(|s| s.walks)
    }

    /// The `cmd.weapon` this cmd carries, `None` to follow `ps.weapon`. See
    /// [`WeaponSwitch`] for why the byte is held rather than sent once.
    pub(super) fn weapon_override(&mut self, ps_weapon: u8) -> Option<u8> {
        let due = self.steps.get(self.idx).and_then(|step| {
            let (at, to) = step.switch?;
            self.started
                .is_some_and(|t| t.elapsed() >= at)
                .then_some((step.label, to))
        });
        if let Some((label, to)) = due {
            let idx = match to {
                SwitchTo::Frag => self.frag_index,
                SwitchTo::Joined => self.joined_index,
            };
            if self.switch.ask(idx) {
                println!("COMBAT {label}: asking for weapon {idx}");
            }
        }
        self.switch.byte(ps_weapon)
    }

    pub(super) fn running(&self) -> bool {
        !self.done
    }

    /// Logs a cmd as it went on the wire, with the absolute view `angles`
    /// it was built from: under its step once the script has begun, else
    /// in [`Self::before`].
    pub(super) fn record(&mut self, c: &net::msg::UserCmd, angles: [i32; 3]) {
        let sent = SentCmd {
            server_time: c.server_time,
            buttons: c.buttons,
            wbuttons: c.wbuttons,
            weapon: c.weapon,
            view: [angles[0] & 0xffff, angles[1] & 0xffff],
            moves: [c.forward, c.right, c.up],
        };
        match self.steps.get(self.idx) {
            Some(step) if !self.start_times.is_empty() => self.sent.push((step.label, sent)),
            Some(_) => {
                self.before.push(sent);
                let excess = self.before.len().saturating_sub(16);
                self.before.drain(..excess);
            }
            None => {}
        }
    }

    /// Holds the step's base input, and taps only once its clock runs: a step
    /// waiting for the weapon to read ready must not be tapping while it waits.
    /// A walk stands still until its clock runs too: walking through the
    /// join's settle would take the `# spawn` pose (and the yaw a gate rebases
    /// `!input` on) after a stall turn the input never had.
    pub(super) fn cmd(&self) -> net::msg::UserCmd {
        match (self.steps.get(self.idx), self.started) {
            (Some(step), Some(t)) => step.cmd_at(t.elapsed()),
            (Some(step), None) if step.walks => net::msg::NULL_USERCMD,
            (Some(step), None) => step.base,
            (None, _) => net::msg::NULL_USERCMD,
        }
    }

    /// Sizes the reload step off the weapon the join was given, once. A hold
    /// shorter than `reloadTime` leaves the reload running into the steps after
    /// it, which recorded a reload under two fire labels before this existed.
    pub(super) fn use_weapon(
        &mut self,
        fs: Option<&vcod_common::pk3::Pk3Fs>,
        name: &str,
        configstrings: &[String],
    ) {
        if self.weapon_read || name.is_empty() {
            return;
        }
        self.weapon_read = true;
        if let Some((idx, frag)) = weapon_cs_index(configstrings, FRAG_WEAPON) {
            self.frag_index = idx;
            self.frag_name = frag;
        }
        self.joined_index = weapon_cs_index(configstrings, name).map_or(0, |(i, _)| i);
        println!(
            "COMBAT: configstring 7 index {} for {name}, {} for {FRAG_WEAPON}",
            self.joined_index, self.frag_index
        );
        let Some(fs) = fs else {
            println!("COMBAT: no game data, reload holds the {RELOAD_HOLD_FALLBACK:?} fallback");
            return;
        };
        let def = match vcod_common::weapon::load(fs, name) {
            Ok(def) => def,
            Err(e) => {
                println!("COMBAT: cannot read weapon {name} ({e:#}); reload holds the fallback");
                return;
            }
        };
        let (Some(reload), Some(fire)) = (data_secs(def.reload_time), data_secs(def.fire_time))
        else {
            println!(
                "COMBAT: {name} reloadTime {} fireTime {} are no times; reload holds the fallback",
                def.reload_time, def.fire_time
            );
            return;
        };
        let dur = reload + RELOAD_MARGIN;
        let period = (fire + PULSE_MARGIN).max(PULSE_PERIOD);
        for step in &mut self.steps {
            if step.from_reload_time {
                step.dur = dur;
            }
            step.period = period;
        }
        println!(
            "COMBAT: {name} reloadTime {:.2}s fireTime {:.3}s, reload step holds {dur:?}, taps every {period:?}",
            def.reload_time, def.fire_time
        );
    }

    /// Feeds the newest snapshot in. Returns true once the last step is done
    /// and the fixture is ready to write.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let Some(step) = self.steps.get(self.idx) else {
            self.done = true;
            return true;
        };
        let (label, dur, wait_ready, fires) =
            (step.label, step.dur, step.wait_ready, step_fires(step));
        if self.start_times.is_empty() {
            self.start_times.push((label, snap.server_time));
            // The cmds already in flight when the script began run in its
            // first frames: a walk's first logged cmd otherwise reads as
            // walking from the last `commandTime` traced.
            let ran = snap.ps.field_i32(wire(), "commandTime");
            self.sent.extend(
                self.before
                    .drain(..)
                    .filter(|c| c.server_time > ran)
                    .map(|c| (label, c)),
            );
        }
        let p = wire();
        let weaponstate = snap.ps.field_i32(p, "weaponstate");
        // A step that taps starts from a weapon that can take the input. The
        // wait is on the observed state, not a duration, so a weapon with other
        // timings than the stock rifle's does not silently break the capture.
        if wait_ready && self.started.is_none() {
            let since = *self.waiting_since.get_or_insert(now);
            let waited = now.duration_since(since);
            let steady = if weaponstate == WEAPONSTATE_READY {
                now.duration_since(*self.ready_since.get_or_insert(now))
            } else {
                self.ready_since = None;
                Duration::ZERO
            };
            if steady < READY_STABLE && waited < READY_TIMEOUT {
                return false;
            }
            if steady < READY_STABLE {
                println!(
                    "COMBAT {label}: weaponstate still {weaponstate} after {READY_TIMEOUT:?}, starting anyway"
                );
            } else if waited > READY_STABLE {
                println!(
                    "COMBAT {label}: weapon ready after {}ms",
                    waited.as_millis()
                );
            }
        }
        let started = *self.started.get_or_insert(now);
        // Where the script began, which is the spot a replay throws from.
        self.spawn_pose.get_or_insert_with(|| {
            (
                snap.ps.origin(p),
                [
                    snap.ps.field_f32(p, "viewangles[0]"),
                    snap.ps.field_f32(p, "viewangles[1]"),
                    snap.ps.field_f32(p, "viewangles[2]"),
                ],
            )
        });
        let elapsed = now.duration_since(started);
        let cmd = self.cmd();
        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            let s = CombatSample {
                label,
                elapsed_ms: elapsed.as_millis(),
                server_time: snap.server_time,
                command_time: snap.ps.field_i32(p, "commandTime"),
                buttons: cmd.buttons,
                wbuttons: cmd.wbuttons,
                weaponstate,
                weap_anim: snap.ps.field_i32(p, "weapAnim"),
                legs_anim: snap.ps.field_i32(p, "legsAnim"),
                torso_anim: snap.ps.field_i32(p, "torsoAnim"),
                event_sequence: snap.ps.field_i32(p, "eventSequence"),
                events: [
                    ev_v1(snap.ps.field_i32(p, "events[0]")),
                    ev_v1(snap.ps.field_i32(p, "events[1]")),
                    ev_v1(snap.ps.field_i32(p, "events[2]")),
                    ev_v1(snap.ps.field_i32(p, "events[3]")),
                ],
                weapon: snap.ps.field_i32(p, "weapon"),
                weapon_pos_frac: snap.ps.field_f32(p, "fWeaponPosFrac"),
                aim_spread_scale: snap.ps.field_f32(p, "aimSpreadScale"),
                grenade_time_left: snap.ps.field_i32(p, "grenadeTimeLeft"),
                weapon_delay: snap.ps.field_i32(p, "weaponDelay"),
                pm_flags: snap.ps.field_i32(p, "pm_flags"),
                ground_entity: snap.ps.field_i32(p, "groundEntityNum"),
                viewangles: [
                    snap.ps.field_f32(p, "viewangles[0]"),
                    snap.ps.field_f32(p, "viewangles[1]"),
                    snap.ps.field_f32(p, "viewangles[2]"),
                ],
                e_flags: snap.ps.field_i32(p, "eFlags"),
                missiles: self.missiles.sample(snap),
                leanf: snap.ps.field_f32(p, "leanf"),
                impacts: Vec::new(),
            };
            let mut s = s;
            // Every bullet-impact temp entity this snapshot carries, beside
            // the eye and view it was fired from, for `--probe-sway`: the
            // angle between the raw view and the eye-to-impact ray is the
            // sway retail put on the shot.
            for (&num, e) in &snap.entities {
                let Some(ev) = p.event_of(e.field_i32(p, "eType")).map(nearest_v1) else {
                    continue;
                };
                if ev != EV_BULLET_HIT_SMALL && ev != EV_BULLET_HIT_LARGE {
                    continue;
                }
                let pos = vcod_common::net::trajectory::Trajectory::read(e, p, "pos");
                let eye = [
                    snap.ps.field_f32(p, "origin[0]"),
                    snap.ps.field_f32(p, "origin[1]"),
                    snap.ps.field_f32(p, "origin[2]") + snap.ps.field_f32(p, "viewHeightCurrent"),
                ];
                println!(
                    "  impact st={} ent={num} event={ev} origin={} eye={} leanf={} view=[{:.3},{:.3},{:.3}]",
                    snap.server_time,
                    vec_str(pos.base.into()),
                    vec_str(eye),
                    snap.ps.field_f32(p, "leanf"),
                    s.viewangles[0],
                    s.viewangles[1],
                    s.viewangles[2],
                );
                // The fixture's copy: full precision, and the playerstate
                // the muzzle is built from, so a gate can take the eye apart.
                let o = snap.ps.origin(p);
                s.impacts.push(format!(
                    "!impact ms={} serverTime={} ent={num} event={ev} at={},{},{} ps_origin={},{},{} \
viewHeightCurrent={} leanf={} viewangles={},{},{} eFlags={}\n",
                    s.elapsed_ms,
                    snap.server_time,
                    pos.base[0],
                    pos.base[1],
                    pos.base[2],
                    o[0],
                    o[1],
                    o[2],
                    snap.ps.field_f32(p, "viewHeightCurrent"),
                    s.leanf,
                    s.viewangles[0],
                    s.viewangles[1],
                    s.viewangles[2],
                    s.e_flags,
                ));
            }
            for m in &s.missiles {
                println!("  {}", m.line(s.elapsed_ms).trim_end());
            }
            println!(
                "  trace {label} +{:>5}ms st={} in={:02x}/{:02x} weaponstate={} weapAnim={} legsAnim={} torsoAnim={} evSeq={} events=[{},{},{},{}] ads={:.3} spread={:.1} nade={} delay={} pm=0x{:x} ground={} view=[{:.2},{:.2}]",
                s.elapsed_ms,
                s.server_time,
                s.buttons,
                s.wbuttons,
                s.weaponstate,
                s.weap_anim,
                s.legs_anim,
                s.torso_anim,
                s.event_sequence,
                s.events[0],
                s.events[1],
                s.events[2],
                s.events[3],
                s.weapon_pos_frac,
                s.aim_spread_scale,
                s.grenade_time_left,
                s.weapon_delay,
                s.pm_flags,
                s.ground_entity,
                s.viewangles[0],
                s.viewangles[1],
            );
            self.trace.push(s);
        }
        // The weapon went busy under a fire step before it fired: the taps
        // spent so far bought nothing, so the step starts over rather than
        // recording a reload under a firing label.
        if fires
            && reloading(weaponstate)
            && !self.retried.contains(&label)
            && !self
                .trace
                .iter()
                .any(|s| s.label == label && s.weaponstate == WEAPONSTATE_FIRING)
        {
            println!(
                "COMBAT {label}: weaponstate {weaponstate} +{}ms into a fire step, restarting it once",
                elapsed.as_millis()
            );
            self.retried.push(label);
            self.trace.retain(|s| s.label != label);
            self.started = None;
            self.waiting_since = None;
            self.ready_since = None;
            self.switch.rearm();
            return false;
        }
        if elapsed < dur {
            return false;
        }
        let waited_ms = self
            .waiting_since
            .map_or(0, |t| started.duration_since(t).as_millis());
        let mine: Vec<&CombatSample> = self.trace.iter().filter(|s| s.label == label).collect();
        let set = |f: fn(&CombatSample) -> i32| {
            mine.iter()
                .map(|s| f(s))
                .collect::<std::collections::BTreeSet<i32>>()
        };
        let mut shots_seen = 0;
        let mut was_firing = false;
        for s in &mine {
            let firing = s.weaponstate == WEAPONSTATE_FIRING;
            shots_seen += u32::from(firing && !was_firing);
            was_firing = firing;
        }
        // `eventSequence` is 8 bits on the wire, so a run spanning the wrap
        // reads as a negative difference without this.
        let seq_delta = match (mine.first(), mine.last()) {
            (Some(a), Some(b)) => (b.event_sequence - a.event_sequence).rem_euclid(256),
            _ => 0,
        };
        println!(
            "COMBAT {label}: {} snapshots, {shots_seen} shot(s) seen, eventSequence +{seq_delta}, weaponstate {:?}, weapAnim {:?}, legsAnim {:?}, torsoAnim {:?}",
            mine.len(),
            set(|s| s.weaponstate),
            set(|s| s.weap_anim),
            set(|s| s.legs_anim),
            set(|s| s.torso_anim),
        );
        let fired = mine.iter().any(|s| s.weaponstate == WEAPONSTATE_FIRING);
        if fires && !fired {
            println!(
                "COMBAT WARNING {label}: a fire step that never saw weaponstate {WEAPONSTATE_FIRING}. \
                 The capture is broken, not the server: something left the weapon busy."
            );
        }
        self.results.push(StepResult {
            label,
            fired,
            shots_seen,
            seq_delta,
            waited_ms,
            retried: self.retried.contains(&label),
            fields: snap.ps.fields.clone(),
        });
        self.idx += 1;
        if let Some(next) = self.steps.get(self.idx) {
            self.start_times.push((next.label, snap.server_time));
        }
        self.started = None;
        self.waiting_since = None;
        self.ready_since = None;
        self.switch.rearm();
        if self.idx >= self.steps.len() {
            self.done = true;
            return true;
        }
        false
    }
}

/// Whether the step taps the trigger, which is what makes a step that never
/// observed a shot a defect rather than a fact.
fn step_fires(step: &CombatStep) -> bool {
    step.pulse.buttons & BUTTON_ATTACK != 0 && step.pulse.count > 0
}

/// Added to the weapon file's `reloadTime` so the reload has finished before
/// the next step starts, and used whole when no weapon file can be read.
/// m1carbine_mp's `reloadTime` is 2.65 s; a step shorter than that ran the
/// reload on into the two stance steps and recorded it under their labels.
const RELOAD_MARGIN: Duration = Duration::from_millis(1200);

const RELOAD_HOLD_FALLBACK: Duration = Duration::from_millis(6000);

/// How long `weaponstate` has to read ready before a step's clock starts. One
/// ready snapshot is not enough: in the capture that motivated this, a second
/// reload began 50 ms into `crouch_fire`, and `crouch_fire` had started on a
/// weapon that read ready.
const READY_STABLE: Duration = Duration::from_millis(500);

/// How long a step waits for that before it gives up and starts anyway, so a
/// weapon that never settles costs one step's worth of capture rather than the
/// whole run.
const READY_TIMEOUT: Duration = Duration::from_secs(8);

/// A weapon file's seconds as a `Duration`; `None` for a negative or
/// non-finite one, which a mod's file can hold.
pub(super) fn data_secs(s: f32) -> Option<Duration> {
    Duration::try_from_secs_f32(s).ok()
}

/// A `weaponstate` of the reload's (5 reloading to 9 the reload's end,
/// `docs/research/cod11-combat.md` 1.1). A fire step that meets one before
/// it has seen a shot restarts once: the weapon went busy under it, and the
/// taps it has already spent bought nothing.
fn reloading(weaponstate: i32) -> bool {
    use vcod_common::pmove::weapon::{WEAPON_RELOAD_END, WEAPON_RELOADING};
    (i32::from(WEAPON_RELOADING)..=i32::from(WEAPON_RELOAD_END)).contains(&weaponstate)
}

/// Ground covered below this inside [`STALL_WINDOW`] while walking forward
/// means the probe is standing against geometry. Retail's run speed is
/// ~190 u/s, so a clear walk covers ~280u per window.
const STALL_UNITS: f32 = 60.0;

const STALL_WINDOW: Duration = Duration::from_millis(1500);

const STALL_TURN_DEG: i32 = 45;

/// A wall follower, near enough: a probe walking into geometry stops there and
/// stays, ~135u from a spawn, so a stalled walk turns and keeps going. Opt-in,
/// because every capture that holds an exact input depends on not wandering.
#[derive(Default)]
pub(super) struct StallTurn {
    /// Added to the walk's heading; it accumulates until the walk moves again.
    detour_deg: i32,
    /// Time and position the current stall window opened at.
    last_progress: Option<(Instant, [f32; 3])>,
}

impl StallTurn {
    /// Turns `cmd` by the accumulated detour, and turns further when a forward
    /// walk has covered no ground. Only the horizontal distance counts: a
    /// probe sliding down a slope is not making progress.
    pub(super) fn apply(&mut self, cmd: &mut net::msg::UserCmd, now: Instant, origin: [f32; 3]) {
        if cmd.forward <= 0 {
            self.last_progress = None;
            return;
        }
        let (t0, o0) = *self.last_progress.get_or_insert((now, origin));
        if now.duration_since(t0) >= STALL_WINDOW {
            let moved = ((origin[0] - o0[0]).powi(2) + (origin[1] - o0[1]).powi(2)).sqrt();
            if moved < STALL_UNITS {
                self.detour_deg = (self.detour_deg + STALL_TURN_DEG).rem_euclid(360);
                println!(
                    "STALL: only {moved:.0}u in {}ms, turning to detour {} deg",
                    STALL_WINDOW.as_millis(),
                    self.detour_deg,
                );
            }
            self.last_progress = Some((now, origin));
        }
        cmd.angles[1] += self.detour_deg * 65536 / 360;
    }
}

/// One cmd [`CombatProbe::record`] logged, as the `!cmds` line writes it.
#[derive(Clone, Copy)]
struct SentCmd {
    server_time: i32,
    buttons: u8,
    wbuttons: u8,
    weapon: u8,
    /// Absolute pitch and yaw shorts, before `delta_angles` comes off.
    view: [i32; 2],
    /// `forward`, `right`, `up`.
    moves: [i8; 3],
}

/// Writes one section per step to `<map>-<gametype>-combat.txt`. Each carries
/// the input that produced it on an `!input` line, so the gate replays the
/// capture's own script rather than a copy of it, one `!trace` line per
/// snapshot, and the playerstate the step ended at.
pub(super) fn write_combat_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    combat: &CombatProbe,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let p = wire();
    let key = |k: &str| serverinfo_key(configstrings, k);
    let map = key("mapname");
    let gametype = key("g_gametype");

    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server playerstate under weapon input.\n",
        retail_build()
    ));
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
    ));
    out.push_str("# sv_maxclients 8, sv_pure 0, stock scr_* defaults, one client on the server.\n");
    out.push_str(&format!(
        "# Captured with tools/run_server.sh and --net-probe --save-{}.\n",
        combat.kind
    ));
    out.push_str("# The fire bit is tapped, not held: the stock rifle is semi-automatic and a\n");
    out.push_str("# held bit fires one shot and then nothing. !input carries the held input,\n");
    out.push_str("# the tapped bits, how many taps and the tap timing, all in ms, and the\n");
    out.push_str("# usercmd weapon byte every cmd of the step carried.\n");
    out.push_str("# One !trace line per snapshot, because the event ring holds four slots and\n");
    out.push_str("# overwrites: a shot is a transient no settled sample can hold. The field\n");
    out.push_str("# lines after a step's traces are the playerstate it ended at.\n");
    out.push_str("# !observed carries what the step turned out to be: whether a fire step ever\n");
    out.push_str("# saw weaponstate 3, how many shots and how much eventSequence it gained, and\n");
    out.push_str("# the distinct values each channel took. A fire step with fired=0 is a broken\n");
    out.push_str("# capture and is flagged above its section as well.\n");
    out.push_str("# Each trace also carries fWeaponPosFrac and aimSpreadScale as decimals,\n");
    out.push_str("# the sight fraction and the spread counter (combat doc, 1.13 and 2.1),\n");
    out.push_str("# grenadeTimeLeft and weaponDelay as ms.\n");
    out.push_str("# !cmds lists every cmd sent while the step was current, including its wait\n");
    out.push_str("# for a ready rifle: start= the newest snapshot's serverTime when it became\n");
    out.push_str("# current, st= the first cmd's serverTime and d= the steps\n");
    out.push_str("# to each next one, in= <index>:<buttons>/<wbuttons> for each cmd whose bits\n");
    out.push_str("# differ from the !input base (the taps and presses), w= <index>:<byte> for\n");
    out.push_str("# each cmd whose weapon byte differs from the cmd before it (the first from\n");
    out.push_str("# the step's weapon=), a= <index>:<pitch>/<yaw> for each cmd whose absolute\n");
    out.push_str("# view shorts (before delta_angles) differ from the cmd before it (the\n");
    out.push_str(
        "# first always), which carries a stall turn, and m= <index>:<forward>/<right>/<up>\n",
    );
    out.push_str("# where the movement changes (the first always). The first step's cmds start\n");
    out.push_str("# past its first snapshot's commandTime. A trace's commandTime\n");
    out.push_str("# is the last cmd its frame ran.\n");
    out.push_str("# Values are the raw i32 wire words, floats as their bit patterns.\n");
    // Where the script stood and looked at its first step. A replay that
    // starts anywhere else is looking somewhere else too, and `viewangles` is
    // the sum of the cmd and the spawn's `delta_angles`, so a gate comparing
    // the view needs this to put its own client where the capture's was.
    let (spawn_origin, spawn_view) = combat.spawn_pose.unwrap_or_default();
    out.push_str(&format!(
        "# spawn origin={} viewangles={}\n",
        vec_exact(spawn_origin),
        vec_exact(spawn_view),
    ));
    if combat.kind == "lean" {
        out.push_str("# Every step is scoped and holds still. The probe aims every cmd at the\n");
        out.push_str(
            "# absolute wire yaw below (a wall 250-700 units out, 24 units clear either\n",
        );
        out.push_str("# side), so !input yaw is 0. An !impact line follows each trace for every\n");
        out.push_str(
            "# EV_BULLET_HIT_SMALL/LARGE temp entity on the wire, with the impact origin\n",
        );
        out.push_str("# (at) and the playerstate the muzzle is built from; one impact rides\n");
        out.push_str(
            "# several snapshots. Read in docs/research/cod11-combat.md 2.1 and gated by\n",
        );
        out.push_str("# crates/server/tests/lean_ab.rs.\n");
        out.push_str(&format!("# aim yaw={}\n", combat.aim_yaw.unwrap_or(0)));
    }
    if combat.kind == "grenade" {
        out.push_str(
            "# press_buttons is held down for the first press_ms of the step rather than\n",
        );
        out.push_str("# tapped: a grenade is cooked by holding the trigger and thrown by the\n");
        out.push_str("# release. switch_weapon is asked for at switch_ms into the step and then\n");
        out.push_str("# held on every cmd until ps.weapon reads it: the pickup half takes the\n");
        out.push_str("# byte off the cmd of the frame the putaway ends on, so a byte sent once\n");
        out.push_str(
            "# is reverted before the swap lands and the old weapon stays in hand (1.8).\n",
        );
        out.push_str("# A !missile line follows each trace that had a missile on the wire. It\n");
        out.push_str("# records every entity that reads eType 4 or read it earlier in the run\n");
        out.push_str("# and has not left the wire, because the explode flips the missile's own\n");
        out.push_str("# eType to 0 and adds its event there (combat doc, section 13). Each\n");
        out.push_str("# trajectory is trType,trTime,base xyz,delta xyz; origin is pos evaluated\n");
        out.push_str("# at the snapshot's serverTime.\n");
        let (origin, view) = combat.spawn_pose.unwrap_or_default();
        out.push_str(&format!(
            "# grenade map={map} gametype={gametype} joined={} weapon={} weapon_index={} \
frag={} frag_index={} origin={} viewangles={}\n",
            join.team,
            join.weapon,
            combat.joined_index,
            combat.frag_name,
            combat.frag_index,
            vec_exact(origin),
            vec_exact(view),
        ));
    }
    let broken: Vec<&str> = combat
        .results
        .iter()
        .filter(|r| {
            !r.fired
                && combat
                    .steps
                    .iter()
                    .any(|s| s.label == r.label && step_fires(s))
        })
        .map(|r| r.label)
        .collect();
    if !broken.is_empty() {
        out.push_str(&format!(
            "# BROKEN: fire step(s) {} never observed weaponstate {WEAPONSTATE_FIRING}; \
recapture before gating anything on this file.\n",
            broken.join(", ")
        ));
    }
    for r in &combat.results {
        let Some(step) = combat.steps.iter().find(|s| s.label == r.label) else {
            continue;
        };
        let mine: Vec<&CombatSample> = combat.trace.iter().filter(|s| s.label == r.label).collect();
        let set = |f: fn(&CombatSample) -> i32| {
            mine.iter()
                .map(|s| f(s))
                .map(|v| v.to_string())
                .collect::<std::collections::BTreeSet<String>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",")
        };
        if step_fires(step) && !r.fired {
            out.push_str(&format!(
                "# BROKEN: {} never observed weaponstate {WEAPONSTATE_FIRING}; it captured no shot.\n",
                r.label
            ));
        }
        out.push_str(&format!("[step {}]\n", r.label));
        out.push_str(&format!(
            "!input buttons={} wbuttons={} up={} forward={} right={} pitch={} yaw={} \
pulse_buttons={} pulse_wbuttons={} pulses={} pulse_hold_ms={} pulse_period_ms={} \
press_buttons={} press_ms={} switch_weapon={} switch_ms={} \
hold_ms={} walks={} wait_ready={} weapon={}\n",
            step.base.buttons,
            step.base.wbuttons,
            step.base.up,
            step.base.forward,
            step.base.right,
            step.base.angles[0],
            step.base.angles[1],
            step.pulse.buttons,
            step.pulse.wbuttons,
            step.pulse.count,
            PULSE_HOLD.as_millis(),
            step.period.as_millis(),
            step.press,
            step.press_for.as_millis(),
            match step.switch.map(|(_, to)| to) {
                Some(SwitchTo::Frag) => combat.frag_index,
                Some(SwitchTo::Joined) => combat.joined_index,
                None => 0,
            },
            step.switch.map_or(0, |(at, _)| at.as_millis()),
            step.dur.as_millis(),
            step.walks as i32,
            step.wait_ready as i32,
            // The weapon byte the step ran with, so a replay sends it too.
            mine.first().map_or(0, |s| s.weapon),
        ));
        let sent: Vec<_> = combat.sent.iter().filter(|c| c.0 == r.label).collect();
        if let (Some(first), Some((_, start))) = (
            sent.first(),
            combat.start_times.iter().find(|(l, _)| *l == r.label),
        ) {
            let d: Vec<String> = sent
                .windows(2)
                .map(|w| (w[1].1.server_time - w[0].1.server_time).to_string())
                .collect();
            let held: Vec<String> = sent
                .iter()
                .enumerate()
                .filter(|(_, (_, c))| {
                    (c.buttons, c.wbuttons) != (step.base.buttons, step.base.wbuttons)
                })
                .map(|(i, (_, c))| format!("{i}:{}/{}", c.buttons, c.wbuttons))
                .collect();
            // Where the byte changes, the first cmd against the step's
            // `weapon=`: a switch is held on the wire until `ps.weapon`
            // reads it (`WeaponSwitch`).
            let mut byte = mine.first().map_or(0, |s| s.weapon);
            let mut bytes: Vec<String> = Vec::new();
            for (i, (_, c)) in sent.iter().enumerate() {
                if i32::from(c.weapon) != byte {
                    byte = i32::from(c.weapon);
                    bytes.push(format!("{i}:{byte}"));
                }
            }
            let (mut view, mut moves) = (None, None);
            let (mut views, mut walks) = (Vec::new(), Vec::new());
            for (i, (_, c)) in sent.iter().enumerate() {
                if view != Some(c.view) {
                    view = Some(c.view);
                    views.push(format!("{i}:{}/{}", c.view[0], c.view[1]));
                }
                if moves != Some(c.moves) {
                    moves = Some(c.moves);
                    let [f, r, u] = c.moves;
                    walks.push(format!("{i}:{f}/{r}/{u}"));
                }
            }
            out.push_str(&format!(
                "!cmds start={start} st={} d={} in={} w={} a={} m={}\n",
                first.1.server_time,
                d.join(","),
                held.join(","),
                bytes.join(","),
                views.join(","),
                walks.join(",")
            ));
        }
        out.push_str(&format!(
            "!observed fire_step={} expected_shots={} fired={} shots_seen={} seq_delta={} \
waited_ready_ms={} retried={} snapshots={} weaponstate={} legsAnim={} torsoAnim={} weapAnim={}\n",
            step_fires(step) as i32,
            if step_fires(step) {
                step.pulse.count
            } else {
                0
            },
            r.fired as i32,
            r.shots_seen,
            r.seq_delta,
            r.waited_ms,
            r.retried as i32,
            mine.len(),
            set(|s| s.weaponstate),
            set(|s| s.legs_anim),
            set(|s| s.torso_anim),
            set(|s| s.weap_anim),
        ));
        for s in &mine {
            out.push_str(&format!(
                "!trace ms={} serverTime={} buttons={} wbuttons={} weaponstate={} weapAnim={} \
legsAnim={} torsoAnim={} eventSequence={} events[0]={} events[1]={} events[2]={} events[3]={} \
fWeaponPosFrac={:.4} aimSpreadScale={:.2} grenadeTimeLeft={} weaponDelay={} pm_flags={} \
groundEntityNum={} viewangles[0]={:.4} viewangles[1]={:.4} viewangles[2]={:.4} eFlags={} \
leanf={:.4} commandTime={}\n",
                s.elapsed_ms,
                s.server_time,
                s.buttons,
                s.wbuttons,
                s.weaponstate,
                s.weap_anim,
                s.legs_anim,
                s.torso_anim,
                s.event_sequence,
                s.events[0],
                s.events[1],
                s.events[2],
                s.events[3],
                s.weapon_pos_frac,
                s.aim_spread_scale,
                s.grenade_time_left,
                s.weapon_delay,
                s.pm_flags,
                s.ground_entity,
                s.viewangles[0],
                s.viewangles[1],
                s.viewangles[2],
                s.e_flags,
                s.leanf,
                s.command_time,
            ));
            for m in &s.missiles {
                out.push_str(&m.line(s.elapsed_ms));
            }
            for line in &s.impacts {
                out.push_str(line);
            }
        }
        for (f, v) in p.player_fields.iter().zip(&r.fields) {
            out.push_str(&format!("{} {v}\n", f.name));
        }
    }

    // `--capture-tag` keeps a capture taken with another weapon beside the
    // committed one rather than on top of it.
    let path = write_fixture_file(
        PLAYERSTATE_FIXTURE_DIR,
        &tagged_name(&format!("{map}-{gametype}-{}", combat.kind), tag),
        &out,
        overwrite,
    )?;
    println!(
        "{}: {} steps, {} traced snapshots -> {path}",
        combat.kind,
        combat.results.len(),
        combat.trace.len(),
    );
    if !broken.is_empty() {
        println!(
            "combat: BROKEN fire step(s) {}: no shot captured",
            broken.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::tests::pickup_snap;

    /// A weapon file's negative or non-finite time is no duration, not a
    /// panic (BUG-089).
    #[test]
    fn a_weapon_time_that_is_no_duration_is_refused() {
        for bad in [-1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(data_secs(bad), None);
        }
        assert_eq!(data_secs(0.5), Some(Duration::from_millis(500)));
    }

    /// The reload the fire step restarts on is weaponstate 5 to 9; 2 is
    /// the drop (combat doc 1.1; BUG-088).
    #[test]
    fn a_fire_step_restarts_on_the_reload_states() {
        assert!((5..=9).all(reloading));
        assert!(![0, 1, 2, 3, 4, 10].into_iter().any(reloading));
    }

    #[test]
    fn a_fire_step_taps_the_attack_bit_once_per_shot() {
        let script = combat_script();
        let step = script
            .iter()
            .find(|s| s.label == "sustained_fire")
            .expect("the script fires a burst");
        let (mut edges, mut frames_down, mut was_down) = (0, 0, false);
        for frame in 0..step.dur.as_millis() / 16 {
            let down = step
                .cmd_at(Duration::from_millis(frame as u64 * 16))
                .buttons
                & BUTTON_ATTACK
                != 0;
            edges += u32::from(down && !was_down);
            frames_down += u32::from(down);
            was_down = down;
        }
        assert_eq!(edges, step.pulse.count);
        assert!(
            frames_down <= 3 * step.pulse.count,
            "{frames_down} frames down for {} taps: the bit is being held",
            step.pulse.count
        );
    }

    /// A step that taps has to start from a weapon that can take the input.
    /// A reload that outlived its step ran on into both stance steps and was
    /// recorded under their labels, with no shot in either.
    #[test]
    fn a_tapping_step_starts_from_a_ready_weapon() {
        for step in combat_script().iter().filter(|s| s.pulse.count > 0) {
            assert!(
                step.wait_ready,
                "{} taps without waiting for the weapon",
                step.label
            );
        }
        let reload = combat_script()
            .into_iter()
            .find(|s| s.from_reload_time)
            .expect("the script reloads");
        // m1carbine_mp's `reloadTime`, the shortest hold the fallback has to
        // outlast when no weapon file can be read.
        assert!(reload.dur > Duration::from_millis(2650));
    }

    /// An entity is a missile from its first eType-4 snapshot until it leaves
    /// the wire, not while it reads 4: the explode flips the missile's own
    /// eType to 0 and adds event 178 there (combat doc, section 13), so an
    /// eType-4 filter drops the one frame the capture exists for.
    #[test]
    fn missile_watch_keeps_an_entity_past_its_last_etype_4_frame() {
        use crate::fx::registry::EV_GRENADE_EXPLODE;
        use net::msg::{EntityState, PlayerState};
        let p = &net::protocol::PROTOCOL_V1;
        let ent = |num: u32, etype: i32, event: i32| {
            let mut e = EntityState::null(p);
            e.number = num;
            e.fields[EntityState::field_index(p, "eType").unwrap()] = etype;
            e.fields[EntityState::field_index(p, "events[0]").unwrap()] = event;
            e
        };
        let snap = |ents: Vec<EntityState>| net::snapshot::Snapshot {
            server_time: 1000,
            message_num: 1,
            delta_num: -1,
            snap_flags: 0,
            ps: PlayerState::null(p),
            entities: ents.into_iter().map(|e| (e.number, e)).collect(),
            clients: BTreeMap::new(),
            valid: true,
        };
        let mut watch = MissileWatch::default();

        // A flying grenade and a player: only the grenade is a missile.
        let flying = watch.sample(&snap(vec![
            ent(5, crate::entities::ET_MISSILE, 0),
            ent(2, crate::entities::ET_PLAYER, 0),
        ]));
        assert_eq!(flying.iter().map(|m| m.num).collect::<Vec<_>>(), vec![5]);

        // The explode frame: the same entity now reads eType 0 with 178 on it.
        let exploding = watch.sample(&snap(vec![
            ent(5, crate::entities::ET_GENERAL, EV_GRENADE_EXPLODE),
            ent(2, crate::entities::ET_PLAYER, 0),
        ]));
        assert_eq!(exploding.len(), 1, "the explode frame was dropped");
        assert_eq!(exploding[0].num, 5);
        assert_eq!(exploding[0].e_type, crate::entities::ET_GENERAL);
        assert_eq!(exploding[0].events[0], EV_GRENADE_EXPLODE);

        // Freed: the slot is forgotten, so its reuse does not read as a missile.
        assert!(
            watch
                .sample(&snap(vec![ent(2, crate::entities::ET_PLAYER, 0)]))
                .is_empty()
        );
        assert!(
            watch
                .sample(&snap(vec![ent(5, crate::entities::ET_ITEM, 0)]))
                .is_empty()
        );
    }

    /// A capture off another protocol prints vcod's (1.1's) event ids and
    /// means of death, as it prints `pm_flags` and `eFlags`: UO's grenade
    /// explode (185) on a missile's ring and its suicide obituary (MOD 21)
    /// read as 1.1's.
    #[test]
    fn a_uo_capture_prints_1_1_ids() {
        use net::event_ids::{EV_GRENADE_EXPLODE, EV_OBITUARY};
        use net::msg::{EntityState, PlayerState};
        let p = &net::protocol::PROTOCOL_V22;
        set_wire(p);
        let set = |e: &mut EntityState, name: &str, v: i32| {
            e.fields[EntityState::field_index(p, name).unwrap()] = v;
        };
        let mut missile = EntityState::null(p);
        missile.number = 5;
        set(&mut missile, "eType", crate::entities::ET_GENERAL);
        set(&mut missile, "events[0]", 185);
        let mut obit = EntityState::null(p);
        obit.number = 70;
        set(&mut obit, "eType", p.et_events + 209);
        set(&mut obit, "eventParm", 0x80 | 21);
        let mut ps = PlayerState::null(p);
        ps.fields[PlayerState::field_index(p, "weapons[2]").unwrap()] = 4;
        let snap = net::snapshot::Snapshot {
            server_time: 1000,
            message_num: 1,
            delta_num: -1,
            snap_flags: 0,
            ps,
            entities: [(5, missile), (70, obit)].into_iter().collect(),
            clients: BTreeMap::new(),
            valid: true,
        };
        let mut watch = MissileWatch::default();
        watch.seen.insert(5);
        assert_eq!(watch.sample(&snap)[0].events[0], EV_GRENADE_EXPLODE);
        let mut tracker = vcod_common::net::events::EventTracker::new();
        let obits: Vec<i32> = drain_v1(&mut tracker, &snap, p)
            .iter()
            .filter(|e| e.event == EV_OBITUARY)
            .map(|e| e.parm)
            .collect();
        assert_eq!(obits, [0x80 | 22]);
        assert_eq!(weapon_bits(&snap.ps, p), "0,0,4,0");
        assert_eq!(retail_build(), "UO 1.51b");
    }

    /// One fire step of one tap, held `ms`.
    fn one_shot(wait_ready: bool, ms: u64) -> CombatProbe {
        CombatProbe {
            steps: vec![CombatStep {
                label: "shot",
                pulse: Pulse {
                    buttons: BUTTON_ATTACK,
                    wbuttons: 0,
                    count: 1,
                },
                dur: Duration::from_millis(ms),
                wait_ready,
                ..CombatStep::default()
            }],
            ..CombatProbe::default()
        }
    }

    /// A tapping step's clock starts once the weapon has read ready for
    /// 500 ms without a break, or after 8 s of waiting whatever it reads.
    #[test]
    fn a_combat_step_waits_for_a_steady_ready_weapon_or_its_timeout() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let ws = |num, state: u8| pickup_snap(num, &[("weaponstate", i32::from(state))], &[]);
        use vcod_common::pmove::weapon::{WEAPON_FIRING, WEAPON_RAISING, WEAPON_READY};
        let mut c = one_shot(true, 1000);
        assert!(!c.step(ms(0), &ws(1, WEAPON_RAISING)));
        assert!(!c.step(ms(300), &ws(2, WEAPON_READY)));
        assert!(!c.step(ms(400), &ws(3, WEAPON_RAISING)), "a break");
        assert!(!c.step(ms(500), &ws(4, WEAPON_READY)));
        assert!(!c.step(ms(999), &ws(5, WEAPON_READY)));
        assert_eq!(c.started, None, "499 ms steady");
        assert!(!c.step(ms(1000), &ws(6, WEAPON_READY)));
        assert_eq!(c.started, Some(ms(1000)));
        assert!(!c.step(ms(1100), &ws(7, WEAPON_FIRING)));
        assert!(c.step(ms(2000), &ws(8, WEAPON_READY)), "the last step ends");
        let r = &c.results[0];
        assert_eq!((r.fired, r.shots_seen, r.waited_ms), (true, 1, 1000));
        assert_eq!(c.trace.len(), 3, "a sample per snapshot from the start");

        let mut c = one_shot(true, 1000);
        assert!(!c.step(ms(0), &ws(1, WEAPON_RAISING)));
        assert!(!c.step(ms(7999), &ws(2, WEAPON_RAISING)));
        assert_eq!(c.started, None);
        assert!(!c.step(ms(8000), &ws(3, WEAPON_RAISING)));
        assert_eq!(c.started, Some(ms(8000)), "the timeout");
    }

    /// A reload under a fire step before its shot restarts the step once,
    /// dropping its samples; a second reload rides; `eventSequence`'s
    /// count runs across its 8-bit wrap.
    #[test]
    fn a_fire_step_restarts_once_on_a_reload_and_counts_events_across_the_wrap() {
        use vcod_common::pmove::weapon::{WEAPON_READY, WEAPON_RELOADING};
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let snap = |num, state: u8, seq| {
            pickup_snap(
                num,
                &[("weaponstate", i32::from(state)), ("eventSequence", seq)],
                &[],
            )
        };
        let mut c = one_shot(false, 1000);
        assert!(!c.step(ms(0), &snap(1, WEAPON_READY, 240)));
        assert!(!c.step(ms(100), &snap(2, WEAPON_RELOADING, 241)));
        assert_eq!(c.started, None, "restarted");
        assert_eq!(c.retried, ["shot"]);
        assert!(c.trace.is_empty());
        assert!(!c.step(ms(200), &snap(3, WEAPON_READY, 250)));
        assert!(!c.step(ms(300), &snap(4, WEAPON_RELOADING, 254)));
        assert_eq!(c.started, Some(ms(200)), "once only");
        assert!(c.step(ms(1200), &snap(5, WEAPON_READY, 4)));
        let r = &c.results[0];
        assert!(r.retried && !r.fired);
        assert_eq!(r.seq_delta, 10, "250 to 4 across the wrap");
        let kept: Vec<u32> = c.trace.iter().map(|s| s.server_time as u32).collect();
        assert_eq!(kept, [1150, 1200, 1250]);
    }

    /// A forward walk that covers under 60 units in a 1.5 s window turns
    /// 45 degrees more each window; the detour stays once it moves again;
    /// a cmd that does not walk forward closes the window.
    #[test]
    fn a_stalled_walk_turns_45_degrees_a_window_and_keeps_the_detour() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let walk = || net::msg::UserCmd {
            forward: 127,
            ..net::msg::NULL_USERCMD
        };
        let mut st = StallTurn::default();
        let yaw = |st: &mut StallTurn, at, origin| {
            let mut c = walk();
            st.apply(&mut c, ms(at), origin);
            c.angles[1]
        };
        assert_eq!(yaw(&mut st, 0, [0.0; 3]), 0);
        assert_eq!(yaw(&mut st, 1499, [10.0, 0.0, 0.0]), 0);
        assert_eq!(
            yaw(&mut st, 1500, [50.0, 0.0, 500.0]),
            8192,
            "a fall is no progress"
        );
        assert_eq!(yaw(&mut st, 3000, [50.0, 30.0, 0.0]), 16384);
        assert_eq!(
            yaw(&mut st, 4500, [200.0, 0.0, 0.0]),
            16384,
            "moved; the detour stays"
        );
        let mut stand = net::msg::NULL_USERCMD;
        st.apply(&mut stand, ms(5000), [200.0, 0.0, 0.0]);
        assert_eq!(stand.angles[1], 0, "a stand is not turned");
        assert_eq!(yaw(&mut st, 6000, [200.0, 0.0, 0.0]), 16384);
        assert_eq!(yaw(&mut st, 7499, [200.0, 0.0, 0.0]), 16384, "a new window");
        assert_eq!(yaw(&mut st, 7500, [200.0, 0.0, 0.0]), 24576);
    }
}
