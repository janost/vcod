//! The scripted walks `--save-slope` runs (prone, moves, turns, sprint, turret, patch and the rest)
//! and their runner.

use super::combat::{WeaponSwitch, weapon_cs_index};
use super::*;

/// One step of an [`InputScript`] walk ([`prone_script`] and the rest): from
/// `at_ms` on, until the next step, hold these inputs, with the view moving
/// linearly from the previous step's angles to this one's over the step. Yaw
/// is degrees off the uphill heading, pitch positive down the way the wire
/// reads it.
pub(super) struct InputStep {
    at_ms: u64,
    buttons: u8,
    wbuttons: u8,
    forward: i8,
    right: i8,
    up: i8,
    yaw: f32,
    pitch: f32,
}

/// The prone crawl `--probe-prone` runs from the placement, facing up the
/// grade: settle, lie down, crawl up, down and both ways across, turn past
/// the 85-degree yaw cap and back, crawl while the view swings, sweep the
/// pitch past +/-45 of the ground, crawl looking down, turn round and crawl
/// downhill, then stand. Then the prone press on a moving player: running
/// forward, backward and sideways, forward with the sight held, and forward
/// from a crouch, each followed by a crawl and a stand.
pub(super) fn prone_script() -> Vec<InputStep> {
    use net::msg::{BUTTON_ADS, WBUTTON_CROUCH, WBUTTON_PRONE};
    const P: u8 = WBUTTON_PRONE;
    const C: u8 = WBUTTON_CROUCH;
    let step = |at_ms, buttons, wbuttons, forward, right, yaw, pitch| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right,
        up: 0,
        yaw,
        pitch,
    };
    vec![
        step(0, 0, 0, 0, 0, 0.0, 0.0),
        step(1000, 0, P, 0, 0, 0.0, 0.0),
        step(3000, 0, P, 127, 0, 0.0, 0.0),
        step(7000, 0, P, -127, 0, 0.0, 0.0),
        step(11000, 0, P, 0, 127, 0.0, 0.0),
        step(14000, 0, P, 0, -127, 0.0, 0.0),
        step(17000, 0, P, 0, 0, 0.0, 0.0),
        step(19500, 0, P, 0, 0, 150.0, 0.0),
        step(21000, 0, P, 0, 0, 150.0, 0.0),
        step(23500, 0, P, 0, 0, 0.0, 0.0),
        step(24500, 0, P, 127, 0, 60.0, 0.0),
        step(25500, 0, P, 127, 0, -60.0, 0.0),
        step(26500, 0, P, 127, 0, 60.0, 0.0),
        step(27500, 0, P, 127, 0, 0.0, 0.0),
        step(28000, 0, P, 0, 0, 0.0, 0.0),
        step(30000, 0, P, 0, 0, 0.0, 80.0),
        step(32000, 0, P, 0, 0, 0.0, -80.0),
        step(33000, 0, P, 0, 0, 0.0, 70.0),
        step(37000, 0, P, 127, 0, 0.0, 70.0),
        step(38000, 0, P, 0, 0, 0.0, 0.0),
        step(41000, 0, P, 0, 0, 180.0, 0.0),
        step(45000, 0, P, 0, 0, 180.0, 0.0),
        step(49000, 0, P, 127, 0, 180.0, 0.0),
        step(50000, 0, 0, 0, 0, 0.0, 0.0),
        step(52000, 0, 0, 127, 0, 0.0, 0.0),
        step(53000, 0, P, 127, 0, 0.0, 0.0),
        step(55500, 0, 0, 0, 0, 0.0, 0.0),
        step(57500, 0, 0, -127, 0, 0.0, 0.0),
        step(58500, 0, P, -127, 0, 0.0, 0.0),
        step(61000, 0, 0, 0, 0, 0.0, 0.0),
        step(63000, 0, 0, 0, 127, 0.0, 0.0),
        step(64000, 0, P, 0, 127, 0.0, 0.0),
        step(66500, 0, 0, 0, 0, 0.0, 0.0),
        step(68500, BUTTON_ADS, 0, 127, 0, 0.0, 0.0),
        step(69500, BUTTON_ADS, P, 127, 0, 0.0, 0.0),
        step(72000, 0, 0, 0, 0, 0.0, 0.0),
        step(74000, 0, C, 0, 0, 0.0, 0.0),
        step(75000, 0, C, 127, 0, 0.0, 0.0),
        step(76000, 0, P, 127, 0, 0.0, 0.0),
        step(78500, 0, 0, 0, 0, 0.0, 0.0),
        step(80500, 0, 0, 0, 0, 0.0, 0.0),
    ]
}

/// The script `--probe-turns` runs from the street placement of
/// `client-probes/probe_prone`, angles relative to world yaw 90 (up the
/// street's grade): lie down, crawl back until the legs near the kerb behind,
/// turn the view a quarter round each way there, where the legs cannot
/// follow everywhere (1.5's swing search and its view pull), crawl forward
/// with the view swinging, a fast turn past the yaw cap and back, stand;
/// then the gaits the footstep rate tells apart: a run, a lean run each way,
/// a lean strafe, a sighted walk, a crouch walk leaning, each from a stop.
pub(super) fn turns_script() -> Vec<InputStep> {
    use net::msg::{
        BUTTON_ADS, WBUTTON_CROUCH, WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT, WBUTTON_PRONE,
    };
    const P: u8 = WBUTTON_PRONE;
    const C: u8 = WBUTTON_CROUCH;
    const L: u8 = WBUTTON_LEAN_LEFT;
    const R: u8 = WBUTTON_LEAN_RIGHT;
    const S: u8 = BUTTON_ADS;
    let step = |at_ms, buttons, wbuttons, forward, right, yaw| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right,
        up: 0,
        yaw,
        pitch: 0.0,
    };
    vec![
        step(0, 0, 0, 0, 0, 0.0),
        step(1000, 0, P, 0, 0, 0.0),
        step(3000, 0, P, -127, 0, 0.0),
        step(5500, 0, P, 0, 0, 0.0),
        // A quarter turn right over 3 s, held, back, then left.
        step(6500, 0, P, 0, 0, -90.0),
        step(9500, 0, P, 0, 0, -90.0),
        step(11500, 0, P, 0, 0, 0.0),
        step(14500, 0, P, 0, 0, 90.0),
        step(17500, 0, P, 0, 0, 90.0),
        step(19500, 0, P, 0, 0, 0.0),
        // Crawl forward with the view swinging, then a fast turn past the
        // cap and back.
        step(21500, 0, P, 127, 0, 45.0),
        step(23000, 0, P, 127, 0, -45.0),
        step(24500, 0, P, 0, 0, 0.0),
        step(25000, 0, P, 0, 0, 150.0),
        step(25500, 0, P, 0, 0, 150.0),
        step(27500, 0, P, 0, 0, 0.0),
        step(29500, 0, 0, 0, 0, 0.0),
        // Gaits from a stop.
        step(31500, 0, 0, 127, 0, 0.0),
        step(33500, 0, 0, 0, 0, 0.0),
        step(35000, 0, L, 127, 0, 0.0),
        step(37000, 0, 0, 0, 0, 0.0),
        step(38500, 0, R, -127, 0, 0.0),
        step(40500, 0, 0, 0, 0, 0.0),
        step(42000, 0, L, 0, 127, 0.0),
        step(44000, 0, 0, 0, 0, 0.0),
        step(45500, S, 0, 0, 0, 0.0),
        step(46500, S, 0, 127, 0, 0.0),
        step(48500, 0, 0, 0, 0, 0.0),
        step(50000, 0, C, 0, 0, 0.0),
        step(51000, 0, C | R, 127, 0, 0.0),
        step(53000, 0, 0, 0, 0, 0.0),
        step(55000, 0, 0, 0, 0, 0.0),
    ]
}

/// The movement script `--probe-moves` runs from where
/// `client-probes/probe_prone` puts the player on mp_carentan's street,
/// facing world yaw 0, along the street: standing jumps spaced past and inside the 1.5
/// jump penalty, running jumps back to back, a held jump, crouch and its
/// walk, prone and its crawl, the prone press while running forward and
/// backward (1.1's dive), diagonal runs, and semi-automatic shots at the hip
/// and down the sight. A stance key holds `up` at -127, as a retail client
/// does (docs/protocol-1.1.md, "Usercmd input bits").
pub(super) fn moves_script() -> Vec<InputStep> {
    use net::msg::{BUTTON_ADS, BUTTON_ATTACK, WBUTTON_CROUCH, WBUTTON_PRONE};
    const P: u8 = WBUTTON_PRONE;
    const C: u8 = WBUTTON_CROUCH;
    const A: u8 = BUTTON_ATTACK;
    const S: u8 = BUTTON_ADS;
    let step = |at_ms, buttons, wbuttons, forward, right, up: i8| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right,
        up: if wbuttons & (P | C) != 0 { -127 } else { up },
        yaw: 0.0,
        pitch: 0.0,
    };
    let mut v = vec![
        step(0, 0, 0, 0, 0, 0),
        // Standing jumps: one alone, then two each 900 ms after the last.
        step(1000, 0, 0, 0, 0, 127),
        step(1150, 0, 0, 0, 0, 0),
        step(4000, 0, 0, 0, 0, 127),
        step(4150, 0, 0, 0, 0, 0),
        step(4900, 0, 0, 0, 0, 127),
        step(5050, 0, 0, 0, 0, 0),
        step(5800, 0, 0, 0, 0, 127),
        step(5950, 0, 0, 0, 0, 0),
        // Running jumps, each pressed soon after the last landed.
        step(8500, 0, 0, 127, 0, 0),
        step(9300, 0, 0, 127, 0, 127),
        step(9450, 0, 0, 127, 0, 0),
        step(10100, 0, 0, 127, 0, 127),
        step(10250, 0, 0, 127, 0, 0),
        step(10900, 0, 0, 127, 0, 127),
        step(11050, 0, 0, 127, 0, 0),
        step(11600, 0, 0, 0, 0, 0),
        // A jump held for two seconds while backing up.
        step(13500, 0, 0, -127, 0, 0),
        step(14000, 0, 0, -127, 0, 127),
        step(16000, 0, 0, 0, 0, 0),
        // Crouch, crouch walk, stand.
        step(17500, 0, C, 0, 0, 0),
        step(18500, 0, C, 127, 0, 0),
        step(19500, 0, C, -127, 0, 0),
        step(20500, 0, C, 0, 0, 0),
        step(21000, 0, 0, 0, 0, 0),
        // Prone from standing, crawl, stand straight up.
        step(22500, 0, P, 0, 0, 0),
        step(24500, 0, P, 127, 0, 0),
        step(25500, 0, P, 0, 0, 0),
        step(26000, 0, 0, 0, 0, 0),
        // The prone press while running forward, then backward.
        step(28000, 0, 0, 127, 0, 0),
        step(29000, 0, P, 127, 0, 0),
        step(30500, 0, P, 0, 0, 0),
        step(31000, 0, 0, 0, 0, 0),
        step(33000, 0, 0, -127, 0, 0),
        step(34000, 0, P, -127, 0, 0),
        step(35500, 0, P, 0, 0, 0),
        step(36000, 0, 0, 0, 0, 0),
        // Crouch from a run, then a stand from prone through the crouch.
        step(38000, 0, 0, 127, 0, 0),
        step(38800, 0, C, 127, 0, 0),
        step(39800, 0, P, 0, 0, 0),
        step(41300, 0, C, 0, 0, 0),
        step(42300, 0, 0, 0, 0, 0),
        // Diagonal and strafe runs.
        step(43500, 0, 0, 127, 127, 0),
        step(44500, 0, 0, -127, -127, 0),
        step(45500, 0, 0, 0, 127, 0),
        step(46300, 0, 0, 0, -127, 0),
        step(47100, 0, 0, 0, 0, 0),
    ];
    // Shots: four hip taps, the sight raised, four more down it.
    let mut t = 48500;
    for buttons in [A, A, A, A] {
        v.push(step(t, buttons, 0, 0, 0, 0));
        v.push(step(t + 100, 0, 0, 0, 0, 0));
        t += 1200;
    }
    v.push(step(t, S, 0, 0, 0, 0));
    t += 1000;
    for _ in 0..4 {
        v.push(step(t, S | A, 0, 0, 0, 0));
        v.push(step(t + 100, S, 0, 0, 0, 0));
        t += 1200;
    }
    v.push(step(t, 0, 0, 0, 0, 0));
    v.push(step(t + 2000, 0, 0, 0, 0, 0));
    v
}

/// `--probe-sprint`, for a United Offensive server, from the street
/// placement facing world yaw 0 with an LMG in hand (`--probe-weapon
/// mg30cal_mp`): a run, a sprint held past exhaustion with the view
/// swinging round, the stop and the regeneration, a sprint pressed too soon
/// after, a sprinting jump, sprint held with each other gait (back, strafe,
/// crouched, sighted, firing), then prone, the bipod deployed from prone
/// with the view swung past its yaw cap both ways, a move and a burst while
/// deployed, the breakdown and the stand (docs/research/coduo-movement.md).
pub(super) fn sprint_script() -> Vec<InputStep> {
    use net::msg::{BUTTON_ADS, BUTTON_ATTACK, BUTTON_SPRINT_V22, WBUTTON_CROUCH, WBUTTON_PRONE};
    const P: u8 = WBUTTON_PRONE;
    const C: u8 = WBUTTON_CROUCH;
    const A: u8 = BUTTON_ATTACK;
    const S: u8 = BUTTON_ADS;
    const R: u8 = BUTTON_SPRINT_V22;
    let step = |at_ms, buttons, wbuttons, forward, right, up: i8, yaw| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right,
        up: if wbuttons & (P | C) != 0 { -127 } else { up },
        yaw,
        pitch: 0.0,
    };
    vec![
        step(0, 0, 0, 0, 0, 0, 0.0),
        step(1000, 0, 0, 127, 0, 0, 0.0),
        // Sprint from the run, then on round a half turn to exhaustion.
        step(2000, R, 0, 127, 0, 0, 0.0),
        step(2600, R, 0, 127, 0, 0, 0.0),
        step(4100, R, 0, 127, 0, 0, 180.0),
        step(7000, R, 0, 127, 0, 0, 180.0),
        // The stop and the regeneration, a sprint pressed early.
        step(7500, 0, 0, 0, 0, 0, 180.0),
        step(10000, R, 0, 127, 0, 0, 180.0),
        step(10800, 0, 0, 0, 0, 0, 180.0),
        // Full again, a sprinting jump back the other way.
        step(19000, R, 0, 127, 0, 0, 180.0),
        step(19500, R, 0, 127, 0, 127, 0.0),
        step(19650, R, 0, 127, 0, 0, 0.0),
        step(20500, R, 0, -127, 0, 0, 0.0),
        step(21300, R, 0, 0, 127, 0, 0.0),
        step(22100, R, C, 127, 0, 0, 0.0),
        step(22900, R, 0, 127, 0, 0, 0.0),
        step(23400, R | S, 0, 127, 0, 0, 0.0),
        step(24200, R, 0, 127, 0, 0, 180.0),
        step(24700, R | A, 0, 127, 0, 0, 180.0),
        step(25000, R, 0, 127, 0, 0, 180.0),
        step(25500, 0, 0, 0, 0, 0, 180.0),
        // Prone, the bipod down, the view past the cap each way.
        step(27000, 0, P, 0, 0, 0, 180.0),
        step(29000, S, P, 0, 0, 0, 180.0),
        step(30500, S, P, 0, 0, 0, 180.0),
        step(32000, S, P, 0, 0, 0, 250.0),
        step(35000, S, P, 0, 0, 0, 110.0),
        step(36500, S, P, 0, 0, 0, 180.0),
        step(37000, S, P, 127, 0, 0, 180.0),
        step(38000, S, P, 0, 127, 0, 180.0),
        step(38500, S | A, P, 0, 0, 0, 180.0),
        step(38800, S, P, 0, 0, 0, 180.0),
        // The breakdown, the stand, a sprint off.
        step(39500, 0, P, 0, 0, 0, 180.0),
        step(41000, 0, 0, 0, 0, 0, 180.0),
        step(42500, R, 0, 127, 0, 0, 180.0),
        step(43500, 0, 0, 0, 0, 0, 180.0),
        step(45000, 0, 0, 0, 0, 0, 180.0),
    ]
}

/// `--probe-autofire`, for a United Offensive server, from the street
/// placement facing world yaw 0 with an automatic in hand (`--probe-weapon
/// mp44_mp`): the trigger held from the hip, the sight raised and the
/// trigger held down it, then four taps (docs/research/coduo-movement.md).
pub(super) fn autofire_script() -> Vec<InputStep> {
    use net::msg::{BUTTON_ADS, BUTTON_ATTACK};
    const A: u8 = BUTTON_ATTACK;
    const S: u8 = BUTTON_ADS;
    let step = |at_ms, buttons| InputStep {
        at_ms,
        buttons,
        wbuttons: 0,
        forward: 0,
        right: 0,
        up: 0,
        yaw: 0.0,
        pitch: 0.0,
    };
    vec![
        step(0, 0),
        step(1000, A),
        step(2500, 0),
        step(4000, S),
        step(5000, S | A),
        step(6500, S),
        step(7500, 0),
        step(9000, A),
        step(9050, 0),
        step(9400, A),
        step(9450, 0),
        step(9800, A),
        step(9850, 0),
        step(10200, A),
        step(10250, 0),
        step(12000, 0),
    ]
}

/// `--probe-talk`, from the street placement facing world yaw 0: the talk
/// bit (`BUTTON_TALK`, the console or the chat line up) held over the
/// sight, a run, a lean, a shot, a jump, a crouch and a prone crawl, each
/// with the bit released after. `PmoveSingle` masks a talking cmd's
/// buttons to the talk bit on 1.1 and keeps the sight beside it on 1.5 and
/// UO (docs/research/cod15-movement.md 5). A stance key holds `up` at -127.
pub(super) fn talk_script() -> Vec<InputStep> {
    use net::msg::{
        BUTTON_ADS, BUTTON_ATTACK, BUTTON_TALK, WBUTTON_CROUCH, WBUTTON_LEAN_LEFT, WBUTTON_PRONE,
    };
    const P: u8 = WBUTTON_PRONE;
    const C: u8 = WBUTTON_CROUCH;
    const L: u8 = WBUTTON_LEAN_LEFT;
    const A: u8 = BUTTON_ATTACK;
    const S: u8 = BUTTON_ADS;
    const T: u8 = BUTTON_TALK;
    let step = |at_ms, buttons, wbuttons, forward, up: i8| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right: 0,
        up: if wbuttons & (P | C) != 0 { -127 } else { up },
        yaw: 0.0,
        pitch: 0.0,
    };
    vec![
        step(0, 0, 0, 0, 0),
        // The sight up, then the talk bit over it, then off again.
        step(1000, S, 0, 0, 0),
        step(2000, S | T, 0, 0, 0),
        step(3500, S, 0, 0, 0),
        step(4500, 0, 0, 0, 0),
        // A run and a lean pressed while talking, then the run alone and
        // the talk bit pressed mid-run.
        step(5500, T, L, 127, 0),
        step(7000, 0, 0, 127, 0),
        step(8000, T, 0, 127, 0),
        step(9000, 0, 0, 0, 0),
        // A shot and a jump while talking.
        step(10000, T | A, 0, 0, 0),
        step(11000, 0, 0, 0, 0),
        step(12000, T, 0, 0, 127),
        step(12500, 0, 0, 0, 0),
        // Crouched: the stance key survives the mask; the sight over it.
        step(13500, T, C, 0, 0),
        step(14500, S | T, C, 0, 0),
        step(15500, S, C, 0, 0),
        step(16500, 0, 0, 0, 0),
        // Prone, sighted, then talking with the crawl key held.
        step(17500, 0, P, 0, 0),
        step(19000, S, P, 0, 0),
        step(20000, S | T, P, 127, 0),
        step(21500, S, P, 127, 0),
        step(22500, 0, P, 0, 0),
        step(23500, 0, 0, 0, 0),
        step(25500, 0, 0, 0, 0),
    ]
}

/// `--probe-moveshot`: which part of [`moveshot_script`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum MoveShot {
    /// From the street spot with an automatic (`thompson_mp`): bursts
    /// while strafing, running, crouched, sighted and leaning.
    Walk,
    /// From the wall spot with a United Offensive LMG (`mg30cal_mp`):
    /// bursts from the bipod with the view swung off it, then prone
    /// without it.
    Bipod,
}

impl MoveShot {
    pub(super) fn name(self) -> &'static str {
        match self {
            MoveShot::Walk => "walk",
            MoveShot::Bipod => "bipod",
        }
    }
}

/// `--probe-pronemove`: which part of [`pronemove_script`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ProneMove {
    /// Any server, with a bolt-action (`--probe-weapon springfield_mp`):
    /// a move key pressed prone while sighted, while rechambering and
    /// while winding up a swing, then a prone press from a crouched walk
    /// back.
    Rifle,
    /// United Offensive, with an LMG (`--probe-weapon mg30cal_mp`): a move
    /// key pressed on the deployed bipod and through its breakdown; then
    /// the binoculars (`+set probe_binoculars 1`), lowered on a prone
    /// press, raised prone with a move key pressed through the raise, and
    /// lowered prone with the stand pressed through the lowering.
    Lmg,
    /// United Offensive, with an LMG (`--probe-weapon mg30cal_mp`), from
    /// `client-probes/probe_uofoot`'s mound placement (`+set probe_spot
    /// mound`, a standing script mount ahead): prone facing up the grade,
    /// stood up, the bipod deployed standing and the view pitched past both
    /// clamps; then the same from a prone facing down it.
    Deploy,
    /// [`ProneMove::Deploy`] crouched from each stand-up on, on the
    /// crouched mount of `+set probe_spot moundcrouch`.
    DeployCrouch,
}

impl ProneMove {
    pub(super) fn name(self) -> &'static str {
        match self {
            ProneMove::Rifle => "rifle",
            ProneMove::Lmg => "lmg",
            ProneMove::Deploy => "deploy",
            ProneMove::DeployCrouch => "deploy-crouch",
        }
    }

    pub(super) fn switches(self) -> &'static [(u64, &'static str)] {
        match self {
            ProneMove::Rifle | ProneMove::Deploy | ProneMove::DeployCrouch => &[],
            ProneMove::Lmg => &[(10000, "binoculars_mp")],
        }
    }
}

/// `--probe-pronemove`, from the street placement facing world yaw 0:
/// `PM_InteruptWeaponWithProneMove`'s cases, each a move key pressed on a
/// prone body (cod11-combat.md 1.13), and the anim timers they leave
/// (player-model-anim-system.md, "The takeoff and the landing").
/// `deploy` is the standing bipod's pitch clamp off a stale
/// `proneTorsoPitch` (coduo-movement.md 7).
pub(super) fn pronemove_script(part: ProneMove) -> Vec<InputStep> {
    use net::msg::{BUTTON_ADS, BUTTON_ATTACK, BUTTON_MELEE, WBUTTON_CROUCH, WBUTTON_PRONE};
    const P: u8 = WBUTTON_PRONE;
    const C: u8 = WBUTTON_CROUCH;
    const A: u8 = BUTTON_ATTACK;
    const S: u8 = BUTTON_ADS;
    const M: u8 = BUTTON_MELEE;
    let step = |at_ms, buttons, wbuttons, forward, right| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right,
        up: if wbuttons & (P | C) != 0 { -127 } else { 0 },
        yaw: 0.0,
        pitch: 0.0,
    };
    match part {
        ProneMove::Rifle => vec![
            step(0, 0, 0, 0, 0),
            step(1000, 0, P, 0, 0),
            // The sight up, then a forward and a right press under it.
            step(3000, S, P, 0, 0),
            step(4500, S, P, 127, 0),
            step(5500, S, P, 0, 0),
            step(7000, S, P, 0, 127),
            step(8000, S, P, 0, 0),
            // A sighted shot, a forward press in its rechamber.
            step(9500, S | A, P, 0, 0),
            step(9600, S, P, 0, 0),
            step(10000, S, P, 127, 0),
            step(11000, S, P, 0, 0),
            step(12500, 0, P, 0, 0),
            // A swing, a forward press 50 ms into its windup.
            step(13500, M, P, 0, 0),
            step(13550, M, P, 127, 0),
            step(14000, 0, P, 127, 0),
            step(14500, 0, P, 0, 0),
            // A plain crawl, the stand.
            step(15000, 0, P, 127, 0),
            step(16000, 0, P, 0, 0),
            step(16500, 0, 0, 0, 0),
            // The prone press from a crouched walk back.
            step(18500, 0, C, 0, 0),
            step(19500, 0, C, -127, 0),
            step(20500, 0, P, -127, 0),
            step(22000, 0, P, 0, 0),
            step(22500, 0, 0, 0, 0),
            step(24500, 0, 0, 0, 0),
        ],
        ProneMove::Lmg => vec![
            step(0, 0, 0, 0, 0),
            step(1000, 0, P, 0, 0),
            // The bipod down, a forward press on it.
            step(3000, S, P, 0, 0),
            step(5000, S, P, 127, 0),
            step(6000, S, P, 0, 0),
            // The breakdown, a forward press 100 ms into it.
            step(7000, 0, P, 0, 0),
            step(7100, 0, P, 127, 0),
            step(8500, 0, P, 0, 0),
            step(9000, 0, 0, 0, 0),
            // The binoculars come up from 10000 (`ProneMove::switches`):
            // raised standing, lowered on the prone press, then lowered
            // 100 ms after it, inside the drop.
            step(12500, S, 0, 0, 0),
            step(14000, 0, P, 0, 0),
            step(15500, 0, 0, 0, 0),
            step(17000, S, 0, 0, 0),
            step(18500, S, P, 0, 0),
            step(18600, 0, P, 0, 0),
            // Raised prone, a forward press 100 ms into the raise.
            step(20500, S, P, 0, 0),
            step(20600, S, P, 127, 0),
            step(21500, S, P, 0, 0),
            // Lowered prone, the stand pressed 200 ms into it.
            step(23500, 0, P, 0, 0),
            step(23700, 0, 0, 0, 0),
            step(26000, 0, 0, 0, 0),
        ],
        // World yaws: 270 faces up the mound and the mount, 90 down it.
        // Each step's view eases in from the step before.
        ProneMove::Deploy | ProneMove::DeployCrouch => {
            // Crouched from each stand-up on, under `DeployCrouch`.
            let c = if part == ProneMove::DeployCrouch {
                C
            } else {
                0
            };
            let aim = |at_ms, buttons, wbuttons: u8, yaw, pitch| InputStep {
                yaw,
                pitch,
                ..step(
                    at_ms,
                    buttons,
                    if wbuttons == 0 { c } else { wbuttons },
                    0,
                    0,
                )
            };
            vec![
                aim(0, 0, 0, 270.0, 0.0),
                // Prone up the grade, then up.
                aim(1000, 0, P, 270.0, 0.0),
                aim(4000, 0, 0, 270.0, 0.0),
                // Deployed standing, pitched up past the clamp, down past
                // it, level, broken down.
                aim(6000, S, 0, 270.0, 0.0),
                aim(8500, S, 0, 270.0, -85.0),
                aim(10500, S, 0, 270.0, 85.0),
                aim(12500, S, 0, 270.0, 0.0),
                aim(13500, 0, 0, 270.0, 0.0),
                // Prone down the grade, up, and back to the mount.
                aim(15500, 0, 0, 90.0, 0.0),
                aim(16500, 0, P, 90.0, 0.0),
                aim(19500, 0, 0, 90.0, 0.0),
                aim(21000, 0, 0, 270.0, 0.0),
                aim(22000, S, 0, 270.0, 0.0),
                aim(24500, S, 0, 270.0, -85.0),
                aim(26500, S, 0, 270.0, 85.0),
                aim(28500, S, 0, 270.0, 0.0),
                aim(29500, 0, 0, 270.0, 0.0),
                aim(31500, 0, 0, 270.0, 0.0),
            ]
        }
    }
}

/// `--probe-moveshot`, on a 1.5 or United Offensive server, from a
/// `client-probes/probe_moveshot` placement facing world yaw 0 and the
/// wall: short bursts whose muzzle the walk bob, the lean or the bipod
/// trace moves (docs/research/cod11-combat.md 2.1). Each moving burst
/// starts after the body is up to speed. A stance key holds `up` at -127.
pub(super) fn moveshot_script(part: MoveShot) -> Vec<InputStep> {
    use net::msg::{
        BUTTON_ADS, BUTTON_ATTACK, WBUTTON_CROUCH, WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT,
        WBUTTON_PRONE, WBUTTON_RELOAD,
    };
    const A: u8 = BUTTON_ATTACK;
    const S: u8 = BUTTON_ADS;
    const C: u8 = WBUTTON_CROUCH;
    const P: u8 = WBUTTON_PRONE;
    const L: u8 = WBUTTON_LEAN_LEFT;
    const R: u8 = WBUTTON_LEAN_RIGHT;
    const RL: u8 = WBUTTON_RELOAD;
    let step = |at_ms, buttons, wbuttons: u8, forward, right, yaw| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right,
        up: if wbuttons & (P | C) != 0 { -127 } else { 0 },
        yaw,
        pitch: 0.0,
    };
    let mut v = vec![step(0, 0, 0, 0, 0, 0.0)];
    match part {
        MoveShot::Walk => {
            // Moving bursts: (buttons, wbuttons, forward, right), each key
            // held 500 ms, the trigger 400 ms of it, then 900 ms still.
            let mut t = 1000;
            let burst = |v: &mut Vec<InputStep>, t: &mut u64, b: u8, wb: u8, f: i8, r: i8| {
                v.push(step(*t, b, wb, f, r, 0.0));
                v.push(step(*t + 500, b | A, wb, f, r, 0.0));
                v.push(step(*t + 900, b, wb, f, r, 0.0));
                v.push(step(*t + 1100, b, wb, 0, 0, 0.0));
                *t += 2000;
            };
            burst(&mut v, &mut t, 0, 0, 0, 127);
            burst(&mut v, &mut t, 0, 0, 0, -127);
            burst(&mut v, &mut t, 0, 0, 127, 0);
            burst(&mut v, &mut t, 0, 0, -127, 0);
            // A reload, then crouched and sighted strafes each way.
            v.push(step(t, 0, RL, 0, 0, 0.0));
            v.push(step(t + 100, 0, 0, 0, 0, 0.0));
            t += 4000;
            v.push(step(t, 0, C, 0, 0, 0.0));
            t += 1000;
            burst(&mut v, &mut t, 0, C, 0, 127);
            burst(&mut v, &mut t, 0, C, 0, -127);
            v.push(step(t, 0, 0, 0, 0, 0.0));
            t += 1000;
            v.push(step(t, S, 0, 0, 0, 0.0));
            t += 1000;
            burst(&mut v, &mut t, S, 0, 0, 127);
            burst(&mut v, &mut t, S, 0, 0, -127);
            v.push(step(t, 0, 0, 0, 0, 0.0));
            t += 1000;
            v.push(step(t, 0, RL, 0, 0, 0.0));
            v.push(step(t + 100, 0, 0, 0, 0, 0.0));
            t += 4000;
            // Leaning still, each way, then leaning while strafing.
            for lean in [L, R] {
                v.push(step(t, 0, lean, 0, 0, 0.0));
                v.push(step(t + 1000, A, lean, 0, 0, 0.0));
                v.push(step(t + 1300, 0, lean, 0, 0, 0.0));
                v.push(step(t + 1600, 0, 0, 0, 0, 0.0));
                t += 2500;
            }
            burst(&mut v, &mut t, 0, L, 0, 127);
            burst(&mut v, &mut t, 0, R, 0, -127);
            v.push(step(t, 0, 0, 0, 0, 0.0));
            v.push(step(t + 1500, 0, 0, 0, 0, 0.0));
        }
        MoveShot::Bipod => {
            // Down, the bipod deployed facing the wall, a burst along it,
            // then one with the view 30 degrees off each side.
            v.push(step(1000, 0, P, 0, 0, 0.0));
            v.push(step(3000, S, P, 0, 0, 0.0));
            v.push(step(5500, S | A, P, 0, 0, 0.0));
            v.push(step(5800, S, P, 0, 0, 0.0));
            v.push(step(6000, S, P, 0, 0, 0.0));
            v.push(step(7000, S, P, 0, 0, 30.0));
            v.push(step(8000, S | A, P, 0, 0, 30.0));
            v.push(step(8300, S, P, 0, 0, 30.0));
            v.push(step(9000, S, P, 0, 0, 30.0));
            v.push(step(10500, S, P, 0, 0, -30.0));
            v.push(step(11500, S | A, P, 0, 0, -30.0));
            v.push(step(11800, S, P, 0, 0, -30.0));
            // The bipod up: prone from the hip, 30 degrees off, no trace.
            v.push(step(12500, 0, P, 0, 0, -30.0));
            v.push(step(16500, A, P, 0, 0, -30.0));
            v.push(step(16800, 0, P, 0, 0, -30.0));
            v.push(step(18000, 0, P, 0, 0, -30.0));
        }
    }
    v
}

/// `--probe-crouchjump`, from a `client-probes/probe_crouchjump`
/// placement facing a window too low for a standing body: a run at the
/// wall under it, a jump, and crouch held from the cmd after the jump's
/// for a second (none with `crouch` false, the control), then the run on
/// through the window and down outside (bot-navigation.md, "Walled-in
/// spawns"). A stance key holds `up` at -127, as a retail client does.
pub(super) fn crouchjump_script(crouch: bool) -> Vec<InputStep> {
    use net::msg::WBUTTON_CROUCH;
    let c = if crouch { WBUTTON_CROUCH } else { 0 };
    let step = |at_ms, wbuttons: u8, forward, up: i8| InputStep {
        at_ms,
        buttons: 0,
        wbuttons,
        forward,
        right: 0,
        up: if wbuttons != 0 { -127 } else { up },
        yaw: 0.0,
        pitch: 0.0,
    };
    vec![
        step(0, 0, 0, 0),
        step(1000, 0, 127, 0),
        step(1900, 0, 127, 127),
        step(1950, c, 127, 0),
        step(2950, 0, 127, 0),
        step(5000, 0, 0, 0),
        step(7000, 0, 0, 0),
    ]
}

/// `--probe-patchwalk`, from a `client-probes/probe_patchwalk` placement
/// facing a patch: a run into it, runs sliding along it both ways,
/// runs at it turned 30 degrees either side, a running jump at it,
/// crouch walks along it, a crawl into and along it and a stand beside
/// it, then a step back (bsp-ibsp59-format.md, "Patch collision on 1.5 and UO").
pub(super) fn patchwalk_script() -> Vec<InputStep> {
    use net::msg::{WBUTTON_CROUCH, WBUTTON_PRONE};
    const C: u8 = WBUTTON_CROUCH;
    const P: u8 = WBUTTON_PRONE;
    let step = |at_ms, wbuttons: u8, forward, right, up: i8, yaw| InputStep {
        at_ms,
        buttons: 0,
        wbuttons,
        forward,
        right,
        up: if wbuttons != 0 { -127 } else { up },
        yaw,
        pitch: 0.0,
    };
    vec![
        step(0, 0, 0, 0, 0, 0.0),
        // Into it, then along it right, left and back to the middle.
        step(1000, 0, 127, 0, 0, 0.0),
        step(1800, 0, 127, 127, 0, 0.0),
        step(2500, 0, 127, -127, 0, 0.0),
        step(3900, 0, 127, 127, 0, 0.0),
        step(4600, 0, -127, 0, 0, 0.0),
        step(4900, 0, 0, 0, 0, 0.0),
        // At it turned 30 degrees either way.
        step(5200, 0, 127, 0, 0, 30.0),
        step(5900, 0, 127, 0, 0, -30.0),
        step(6600, 0, -127, 0, 0, 0.0),
        step(6900, 0, 0, 0, 0, 0.0),
        // A running jump at it.
        step(7200, 0, 127, 0, 127, 0.0),
        step(7350, 0, 127, 0, 0, 0.0),
        // Crouched along it.
        step(8000, C, 127, 127, 0, 0.0),
        step(8700, C, 127, -127, 0, 0.0),
        step(10100, C, 127, 127, 0, 0.0),
        // Prone, a crawl into it and along it, then up beside it.
        step(10800, P, 0, 0, 0, 0.0),
        step(12300, P, 127, 0, 0, 0.0),
        step(13100, P, 127, 127, 0, 0.0),
        step(13800, P, 127, -127, 0, 0.0),
        step(15200, P, 0, 0, 0, 0.0),
        step(15700, 0, 0, 0, 0, 0.0),
        step(17200, 0, -127, 0, 0, 0.0),
        step(17600, 0, 0, 0, 0, 0.0),
        step(18600, 0, 0, 0, 0, 0.0),
    ]
}

/// `--probe-uofoot`, for a United Offensive server, from a
/// `client-probes/probe_uofoot` placement facing its mount (the flag's
/// world yaw), with
/// `--probe-weapon mg30cal_mp`: the bipod sighted onto the mount, swung
/// within and past its 55-degree cap, fired, pushed against with the move
/// and stance keys, broken down, sighted crouched, then the frag
/// ([`UOFOOT_SWITCHES`]): the sight alone, a plain throw, a cook (the sight
/// pressed under the held trigger) thrown, a cook spooned by the sight's
/// release, and a cook held past the fuse, which throws itself and kills
/// the thrower, so the capture ends there (docs/research/coduo-movement.md, 11 and 12).
pub(super) fn uofoot_script() -> Vec<InputStep> {
    use net::msg::{BUTTON_ADS, BUTTON_ATTACK, WBUTTON_CROUCH};
    const C: u8 = WBUTTON_CROUCH;
    const A: u8 = BUTTON_ATTACK;
    const S: u8 = BUTTON_ADS;
    let step = |at_ms, buttons, wbuttons, forward, right, yaw| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right,
        up: if wbuttons & C != 0 { -127 } else { 0 },
        yaw,
        pitch: 0.0,
    };
    vec![
        step(0, 0, 0, 0, 0, 0.0),
        // The deploy on the mount, the view round within the cap and past it.
        step(1000, S, 0, 0, 0, 0.0),
        step(3500, S, 0, 0, 0, 0.0),
        step(5000, S, 0, 0, 0, 40.0),
        step(6500, S, 0, 0, 0, 75.0),
        step(8500, S, 0, 0, 0, -75.0),
        step(9500, S, 0, 0, 0, 0.0),
        step(10000, S | A, 0, 0, 0, 0.0),
        step(10300, S, 0, 0, 0, 0.0),
        // Moves and the stance key do nothing deployed.
        step(10800, S, 0, 127, 0, 0.0),
        step(11300, S, 0, 0, 127, 0.0),
        step(11800, S, C, 0, 0, 0.0),
        step(12300, S, 0, 0, 0, 0.0),
        // The breakdown; then crouched, the sight onto the same mount.
        step(12800, 0, 0, 0, 0, 0.0),
        step(15500, 0, C, 0, 0, 0.0),
        step(16500, S, C, 0, 0, 0.0),
        step(19500, 0, C, 0, 0, 0.0),
        step(22000, 0, 0, 0, 0, 0.0),
        // The frag comes up from 23500 (UOFOOT_SWITCHES). The sight alone.
        step(26500, S, 0, 0, 0, 0.0),
        step(26800, 0, 0, 0, 0, 180.0),
        // Turned away from the mount, a plain throw: the fuse is not
        // running.
        step(27500, A, 0, 0, 0, 180.0),
        step(28700, 0, 0, 0, 0, 180.0),
        // The sight pressed under the trigger spoons it; its release does
        // nothing more; the throw carries what is left.
        step(31000, A, 0, 0, 0, 180.0),
        step(31400, A | S, 0, 0, 0, 180.0),
        step(31700, A, 0, 0, 0, 180.0),
        step(32800, 0, 0, 0, 0, 180.0),
        // The sight held into the pullback, its release the spoon.
        step(35000, S, 0, 0, 0, 180.0),
        step(35300, A | S, 0, 0, 0, 180.0),
        step(35900, A, 0, 0, 0, 180.0),
        step(37500, 0, 0, 0, 0, 180.0),
        // Cooked past the fuse with the trigger down.
        step(40000, A, 0, 0, 0, 180.0),
        step(40300, A | S, 0, 0, 0, 180.0),
        step(40500, A, 0, 0, 0, 180.0),
        step(46000, 0, 0, 0, 0, 180.0),
        step(48000, 0, 0, 0, 0, 180.0),
    ]
}

/// `--probe-uoturret`, for a United Offensive server, from a
/// `client-probes/probe_turret` placement 40 units behind a gun, facing
/// along it (the yaw the steps turn from): the use tap onto the gun, the
/// view swept past the yaw and pitch arcs, a burst, the move, jump and
/// stance keys held against the mount, the use tap off it and a walk back
/// (cod11-turrets.md 14, coduo-diff.md 19).
pub(super) fn uoturret_script() -> Vec<InputStep> {
    use net::msg::{BUTTON_ATTACK, BUTTON_USE, WBUTTON_CROUCH, WBUTTON_PRONE};
    const U: u8 = BUTTON_USE;
    const A: u8 = BUTTON_ATTACK;
    let step = |at_ms, buttons, wbuttons, forward, right, up, yaw, pitch| InputStep {
        at_ms,
        buttons,
        wbuttons,
        forward,
        right,
        up,
        yaw,
        pitch,
    };
    vec![
        step(0, 0, 0, 0, 0, 0, 0.0, 0.0),
        // The mount: a tap, then the view round past both yaw arcs.
        step(1000, U, 0, 0, 0, 0, 0.0, 0.0),
        step(1150, 0, 0, 0, 0, 0, 0.0, 0.0),
        step(2500, 0, 0, 0, 0, 0, 0.0, 0.0),
        step(5000, 0, 0, 0, 0, 0, 70.0, 0.0),
        step(9000, 0, 0, 0, 0, 0, -70.0, 0.0),
        step(10500, 0, 0, 0, 0, 0, 0.0, 0.0),
        // Pitch past both arcs.
        step(12000, 0, 0, 0, 0, 0, 0.0, 60.0),
        step(14000, 0, 0, 0, 0, 0, 0.0, -60.0),
        step(15000, 0, 0, 0, 0, 0, 0.0, 0.0),
        // A burst, swung as it fires.
        step(15500, A, 0, 0, 0, 0, 0.0, 0.0),
        step(16500, A, 0, 0, 0, 0, 20.0, 5.0),
        step(17000, 0, 0, 0, 0, 0, 20.0, 5.0),
        step(17500, 0, 0, 0, 0, 0, 0.0, 0.0),
        // The move, jump and stance keys against the mount.
        step(18000, 0, 0, 127, 0, 0, 0.0, 0.0),
        step(18500, 0, 0, 0, 127, 0, 0.0, 0.0),
        step(19000, 0, 0, -127, -127, 0, 0.0, 0.0),
        step(19500, 0, 0, 0, 0, 127, 0.0, 0.0),
        step(19800, 0, 0, 0, 0, 0, 0.0, 0.0),
        step(20300, 0, WBUTTON_CROUCH, 0, 0, -127, 0.0, 0.0),
        step(20800, 0, 0, 0, 0, 0, 0.0, 0.0),
        step(21300, 0, WBUTTON_PRONE, 0, 0, 0, 0.0, 0.0),
        step(21800, 0, 0, 0, 0, 0, 0.0, 0.0),
        // Off the gun, turned a little, and a walk back from it.
        step(22500, 0, 0, 0, 0, 0, 30.0, 0.0),
        step(23500, U, 0, 0, 0, 0, 30.0, 0.0),
        step(23650, 0, 0, 0, 0, 0, 30.0, 0.0),
        step(24500, 0, 0, -127, 0, 0, 30.0, 0.0),
        step(25500, 0, 0, 0, 0, 0, 30.0, 0.0),
        step(27000, 0, 0, 0, 0, 0, 30.0, 0.0),
    ]
}

/// [`uofoot_script`]'s weapon changes, by name off configstring 7.
pub(super) const UOFOOT_SWITCHES: &[(u64, &str)] = &[(23500, "fraggrenade_mp")];

/// How long after the first spawned snapshot the crawl starts:
/// `probe_prone.gsc` places the player within about a second of the spawn,
/// and the fall onto the grade settles well inside the rest.
const PRONE_PLACE_WAIT: Duration = Duration::from_millis(3000);

/// Runs one slope walk's steps ([`prone_script`], [`moves_script`] and the
/// rest `--save-slope` picks) with their yaws off `uphill`. The angles are world
/// view angles, which `send_frame` rebases on each snapshot's
/// `delta_angles`, so the view holds the script's heading under a prone cap
/// that pushes it, the way `hold_view_yaw` holds the route's.
pub(super) struct InputScript {
    uphill: f32,
    /// Weapons asked for by name from `at_ms` on, through [`WeaponSwitch`].
    switches: &'static [(u64, &'static str)],
    switch: WeaponSwitch,
    switched: usize,
    first_spawned: Option<Instant>,
    started: Option<Instant>,
    steps: Vec<InputStep>,
    done: bool,
}

impl InputScript {
    pub(super) fn new(uphill: f32, steps: Vec<InputStep>) -> Self {
        Self {
            uphill,
            switches: &[],
            switch: WeaponSwitch::default(),
            switched: 0,
            first_spawned: None,
            started: None,
            steps,
            done: false,
        }
    }

    pub(super) fn switching(
        uphill: f32,
        steps: Vec<InputStep>,
        switches: &'static [(u64, &'static str)],
    ) -> Self {
        Self {
            switches,
            ..Self::new(uphill, steps)
        }
    }

    /// The weapon byte while a scripted switch is in flight, or `None` to
    /// follow `ps.weapon`.
    pub(super) fn weapon_byte(
        &mut self,
        now: Instant,
        ps_weapon: u8,
        configstrings: &[String],
    ) -> Option<u8> {
        if let Some(started) = self.started
            && let Some(&(at, name)) = self.switches.get(self.switched)
            && now.duration_since(started).as_millis() as u64 >= at
        {
            self.switched += 1;
            self.switch.rearm();
            let idx = weapon_cs_index(configstrings, name).map_or(0, |(i, _)| i);
            if self.switch.ask(idx) {
                println!("PRONE asking for {name} ({idx})");
            }
        }
        self.switch.byte(ps_weapon)
    }

    pub(super) fn running(&self) -> bool {
        !self.done
    }

    /// The capture opens once the crawl starts, so the teleport is not in it.
    pub(super) fn recording(&self) -> bool {
        self.started.is_some()
    }

    pub(super) fn cmd(&self, now: Instant) -> net::msg::UserCmd {
        let Some(started) = self.started else {
            return net::msg::NULL_USERCMD;
        };
        let ms = now.duration_since(started).as_millis() as u64;
        let i = self.steps.iter().rposition(|s| s.at_ms <= ms).unwrap_or(0);
        let cur = &self.steps[i];
        // The view eases from the previous step's angles to this one's
        // across this step.
        let (yaw, pitch) = match (i.checked_sub(1), self.steps.get(i + 1)) {
            (Some(p), Some(next)) => {
                let prev = &self.steps[p];
                let t = (ms - cur.at_ms) as f32 / (next.at_ms - cur.at_ms) as f32;
                (
                    prev.yaw + (cur.yaw - prev.yaw) * t,
                    prev.pitch + (cur.pitch - prev.pitch) * t,
                )
            }
            _ => (cur.yaw, cur.pitch),
        };
        let word = |deg: f32| (deg * 65536.0 / 360.0).round() as i32 & 0xffff;
        net::msg::UserCmd {
            angles: [word(pitch), word(self.uphill + yaw), 0],
            forward: cur.forward,
            right: cur.right,
            up: cur.up,
            buttons: cur.buttons,
            wbuttons: cur.wbuttons,
            ..net::msg::NULL_USERCMD
        }
    }

    /// Feeds a spawned player's snapshot in; true once the script is over.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let p = wire();
        let Some(started) = self.started else {
            let first = *self.first_spawned.get_or_insert(now);
            if now.duration_since(first) >= PRONE_PLACE_WAIT {
                self.started = Some(now);
                let o = snap.ps.origin(p);
                println!(
                    "PRONE start at [{:.1},{:.1},{:.1}] facing {:.0}",
                    o[0], o[1], o[2], self.uphill
                );
            }
            return false;
        };
        let end = self.steps.last().map_or(0, |s| s.at_ms);
        if now.duration_since(started).as_millis() as u64 >= end {
            self.done = true;
        }
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prone_step(at_ms: u64, forward: i8, yaw: f32, pitch: f32) -> InputStep {
        InputStep {
            at_ms,
            buttons: 0,
            wbuttons: 0,
            forward,
            right: 0,
            up: 0,
            yaw,
            pitch,
        }
    }

    /// A slope script's view eases from the step before's angles to its
    /// step's over the step, off the uphill yaw; the first and last steps
    /// hold their own; the moves are the step's from its first ms.
    #[test]
    fn a_prone_script_eases_the_view_across_each_step() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let word = |deg: f32| (deg * 65536.0 / 360.0).round() as i32 & 0xffff;
        let mut p = InputScript::new(
            30.0,
            vec![
                prone_step(0, 0, 0.0, 0.0),
                prone_step(1000, 127, 90.0, -20.0),
                prone_step(2000, 0, 180.0, 10.0),
            ],
        );
        assert_eq!(p.cmd(ms(0)).angles, [0; 3], "not started");
        p.started = Some(t0);
        let at = |p: &InputScript, n| {
            let c = p.cmd(ms(n));
            (c.angles[0], c.angles[1], c.forward)
        };
        assert_eq!(at(&p, 999), (word(0.0), word(30.0), 0));
        assert_eq!(
            at(&p, 1000),
            (word(0.0), word(30.0), 127),
            "the step's first ms"
        );
        assert_eq!(at(&p, 1500), (word(-10.0), word(75.0), 127), "mid-step");
        assert_eq!(at(&p, 2000), (word(10.0), word(210.0), 0), "the last step");
    }
}
