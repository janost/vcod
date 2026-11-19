//! `--net-probe`, the headless spectate client. Lives in the client crate
//! because cue resolution needs the audio alias tables.

use net::flags::{ET_TURRET, PM_DEAD};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use vcod_common::net::trajectory::Trajectory;
use vcod_common::net::{self, NetClient, NetEvent, NetState, SnapshotCapture, UdpTransport};

mod bump;
mod combat;
mod download;
mod fixture;
mod hit;
mod join;
mod ladder;
mod mapchange;
mod motion;
mod observe;
mod pickup;
mod pvs;
mod scripts;
mod sd;
mod slope;
mod triggers;
mod turret;
mod watch;

use bump::{BumpProbe, BumpTarget, write_bump_fixture};
use combat::{CombatProbe, lean_sightline, longest_sightline, write_combat_fixture};
use download::DownloadProbe;
pub(crate) use fixture::tag_suffix;
use fixture::{
    MOVERS_FIXTURE_DIR, finish_probe, write_configstrings_fixture, write_playerstate_fixture,
    write_scripted_fixture,
};
use hit::{
    ENGAGE_RANGE, EV_BULLET_HIT_SMALL, HitProbe, KILL_CMDS_QUIET, ROUNDRESTART_KILL_AT,
    SWEEP_PITCH_OFFSETS, SWEEP_RANGE, TARGET_KILL_AT, TARGET_SCORE_PERIOD, TargetProbe,
    write_shooter_fixture, write_target_fixture,
};
use join::JoinProbe;
use ladder::{LadderProbe, LadderScript, write_ladder_fixture, write_weapsel_fixture};
use mapchange::{MapChangeProbe, write_mapchange_fixture};
use motion::{MotionProbe, write_motion_fixture};
use observe::{CompassProbe, FallProbe, FollowProbe, ItemsProbe, KillcamProbe, RideProbe};
use pickup::{PickupProbe, write_pickup_fixture};
use pvs::{PvsProbe, write_entities_fixture};
pub use scripts::{MoveShot, ProneMove};
use scripts::{
    ProneScript, UOFOOT_SWITCHES, autofire_script, crouchjump_script, moves_script,
    moveshot_script, patchwalk_script, prone_script, pronemove_script, sprint_script, talk_script,
    turns_script, uofoot_script, uoturret_script,
};
use sd::{SdProbe, SdRole, write_sd_fixture};
use slope::{SlopeCapture, SlopeStats, SlopeWalk, write_slope_fixture};
use triggers::TriggerProbe;
use turret::{TurretProbe, write_turret_fixture};
use watch::{CS_SOUNDS_COUNT, ProbeWatch, sound_cs_alias};

/// Committed fixtures are absolute (the common crate's parser tests read
/// them); scratch paths are cwd-relative.
const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../common/tests/fixtures/net/gamestate.bin"
);
const SCRATCH_PATH: &str = "tmp/gamestate.bin";
const SNAP_FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../common/tests/fixtures/net/snapshots.bin"
);
const SNAP_SCRATCH_PATH: &str = "tmp/snapshots.bin";
/// How many snapshot-bearing messages to capture before a fixture run stops.
const SNAP_CAPTURE_TARGET: usize = 24;

thread_local! {
    static WIRE: std::cell::Cell<&'static net::protocol::Cod1Wire> =
        const { std::cell::Cell::new(&net::protocol::PROTOCOL_V1) };
}

/// The protocol the probe's connection speaks, which every mode reads its
/// snapshots, events and configstrings through. One probe is one connection;
/// [`probe`] sets it each frame from [`NetClient::protocol`]. 1.1 until then,
/// and in the unit tests.
pub(crate) fn wire() -> &'static net::protocol::Cod1Wire {
    WIRE.with(|w| w.get())
}

fn set_wire(p: &'static net::protocol::Cod1Wire) {
    WIRE.with(|w| w.set(p));
}

/// The untagged fixture names hold 1.1's evidence, which a capture off
/// another protocol must never replace: such a run needs `--capture-tag`.
fn check_capture_tag(
    p: &net::protocol::Cod1Wire,
    writes_fixture: bool,
    tag: Option<&str>,
) -> anyhow::Result<()> {
    if writes_fixture && tag.is_none() && !p.is_v1() {
        anyhow::bail!(
            "this is a protocol {} server; the untagged fixture names hold 1.1 evidence, \
             so pass --capture-tag to write this capture beside them",
            p.transport.version
        );
    }
    Ok(())
}

/// The serverinfo's `mapname` and `g_gametype`, which every fixture file
/// name is built from, refused unless they are letters, digits and `_` as
/// every stock name is: a hostile server's `../x` must not pick the file.
fn check_fixture_names(writes_fixture: bool, serverinfo: &str) -> anyhow::Result<()> {
    if !writes_fixture {
        return Ok(());
    }
    for key in ["mapname", "g_gametype"] {
        let v = net::info_value_for_key(serverinfo, key).unwrap_or("");
        if v.is_empty() || !v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            anyhow::bail!(
                "serverinfo {key} \"{}\" cannot name a fixture file",
                escape_ctl(v)
            );
        }
    }
    Ok(())
}

/// `dir/name` for a fixture file, refused when `name` would leave `dir`.
pub(crate) fn fixture_path(dir: &str, name: &str) -> anyhow::Result<String> {
    vcod_common::game_dir::confined_component(std::path::Path::new(dir), name)
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| anyhow::anyhow!("fixture name \"{}\" leaves {dir}", escape_ctl(name)))
}

/// The build a fixture header names, picked from the wire's protocol version
/// alone: a vcod-server on the same protocol gets the same "Retail" label.
pub(crate) fn retail_build() -> &'static str {
    match wire().transport.version {
        6 => "1.5b",
        22 => "UO 1.51b",
        _ => "1.1d",
    }
}

/// `EventTracker::drain` with an obituary's means of death (its parm with
/// bit 7 set) in 1.1's numbering too; one 1.1 has no id for stays raw.
fn drain_v1(
    tracker: &mut vcod_common::net::events::EventTracker,
    snap: &net::snapshot::Snapshot,
    p: &net::protocol::Cod1Wire,
) -> Vec<vcod_common::net::events::GameEvent> {
    let mut events = tracker.drain(snap, p);
    for ev in &mut events {
        ev.event = nearest_v1(ev.event);
        if ev.event == vcod_common::net::event_ids::EV_OBITUARY && ev.parm & 0x80 != 0 {
            let m = (p.mod_to_v1)(ev.parm & 0x7f);
            if m >= 0 {
                ev.parm = m | 0x80;
            }
        }
    }
    events
}

/// The owned-weapon bitfield words, `weapons[0],weapons[1]`, and the two
/// more United Offensive's 128 weapon slots add.
fn weapon_bits(ps: &net::msg::PlayerState, p: &net::protocol::Cod1Wire) -> String {
    (0..4)
        .map(|k| format!("weapons[{k}]"))
        .filter(|n| net::msg::PlayerState::field_index(p, n).is_some())
        .map(|n| ps.field_i32(p, &n).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// An event id off a raw `events[]` slot in vcod's (1.1's) numbering, the
/// one every probe line and capture prints, as `pm_flags` and `eFlags`
/// already are once decoded.
fn ev_v1(raw: i32) -> i32 {
    nearest_v1((wire().event_to_v1)(raw))
}

/// The 1.1 event a United Offensive one the client keeps apart stands
/// for in a probe line: its bullet hit as the small one, its projectile
/// explodes as the grenade's and the rocket's no-marks one.
fn nearest_v1(event: i32) -> i32 {
    use net::event_ids::*;
    match event {
        EV_UO_BULLET_HIT => EV_BULLET_HIT_SMALL,
        EV_UO_PROJECTILE_EXPLODE => EV_GRENADE_EXPLODE,
        EV_UO_PROJECTILE_EXPLODE_NOMARKS => EV_ROCKET_EXPLODE_NOMARKS,
        e => e,
    }
}

/// Which captures a probe run overwrites. Each is a separate flag because
/// each pins a different thing; the flag docs in `main.rs` say why.
#[derive(Clone, Default)]
pub struct Save {
    /// `--probe-vehicle`: a UO tank crew member, no fixture of its own.
    pub vehicle: Option<crate::vehicle_probe::VehicleRole>,
    /// `--probe-uo-weapon`: a UO weapon script on foot.
    pub uo_weapon: Option<crate::vehicle_probe::WeaponScript>,
    pub fixture: bool,
    pub snapshots: bool,
    /// `--save-demo`: `record` from the first snapshot, and every message
    /// past the gamestate as the tagged pair.
    pub demo: bool,
    pub configstrings: bool,
    pub playerstate: bool,
    pub motion: bool,
    pub slope: bool,
    /// `--slope-every-state`: [`SlopeCapture`] keeps every `pm_type`.
    pub slope_every_state: bool,
    /// `--slope-from-spawn`: [`SlopeCapture`] starts at the spawn.
    pub slope_from_spawn: bool,
    /// `--probe-prone <yaw>`: the slope capture's walk is [`ProneScript`]
    /// instead of the route, facing up the grade along that world yaw.
    pub prone: Option<f32>,
    /// `--probe-moves`: the slope capture's walk is [`moves_script`] where
    /// the player spawned.
    pub moves: bool,
    /// `--probe-turns`: the slope capture's walk is [`turns_script`] from the
    /// street placement.
    pub turns: bool,
    /// `--probe-sprint`: the slope capture's walk is [`sprint_script`] from
    /// the street placement, on United Offensive.
    pub sprint: bool,
    /// `--probe-autofire`: the slope capture's walk is [`autofire_script`]
    /// from the street placement, on United Offensive.
    pub autofire: bool,
    /// `--probe-talk`: the slope capture's walk is [`talk_script`] from the
    /// street placement.
    pub talk: bool,
    /// `--probe-moveshot <part>`: the slope capture's walk is
    /// [`moveshot_script`] from a `client-probes/probe_moveshot` placement.
    pub moveshot: Option<MoveShot>,
    /// `--probe-pronemove <part>`: the slope capture's walk is
    /// [`pronemove_script`] from the street placement.
    pub pronemove: Option<ProneMove>,
    /// `--probe-uofoot <yaw>`: the slope capture's walk is [`uofoot_script`]
    /// from a `client-probes/probe_uofoot` placement facing that world yaw,
    /// on United Offensive.
    pub uofoot: Option<f32>,
    /// `--probe-uoturret <yaw>`: the slope capture's walk is
    /// [`uoturret_script`] from a `client-probes/probe_turret` placement
    /// behind a gun facing that world yaw, on United Offensive.
    pub uoturret: Option<f32>,
    /// `--probe-crouchjump <yaw>`: the slope capture's walk is
    /// [`crouchjump_script`] from a `client-probes/probe_crouchjump`
    /// placement facing that world yaw, on United Offensive.
    pub crouchjump: Option<f32>,
    /// `--probe-crouchjump-stand`: the same script with no crouch, the
    /// control run.
    pub crouchjump_stand: bool,
    /// `--probe-patchwalk <yaw>`: the slope capture's walk is
    /// [`patchwalk_script`] from a `client-probes/probe_patchwalk`
    /// placement facing that world yaw.
    pub patchwalk: Option<f32>,
    pub combat: bool,
    pub ads: bool,
    pub grenade: bool,
    /// `--probe-sway`: a measurement on the combat machine, no fixture.
    pub sway: bool,
    /// `--save-lean`: scoped shots leaning, on the combat machine.
    pub lean: bool,
    pub entities: bool,
    pub hit: bool,
    pub target: bool,
    pub mapchange: bool,
    pub roundrestart: bool,
    /// `--probe-plant`: the S&D attacker half of the plant/defuse pair.
    pub plant: bool,
    /// `--probe-defuse`: the defender half, which waits for that plant.
    pub defuse: bool,
    /// `--save-pickup`: the item pickup capture.
    pub pickup: bool,
    /// `--save-turret`: the mounted MG capture.
    pub turret: bool,
    /// `--probe-turret-target-ms`: how long `target` holds the trigger.
    pub turret_target_ms: Option<u64>,
    /// `--save-bump`: the player-clip walker.
    pub bump: bool,
    /// `--probe-bump-target`: the player-clip target, no fixture.
    pub bump_target: bool,
    /// `--save-legs`: the walk behind the moving-body pose capture.
    pub legs: bool,
    /// `--save-shock`: the shellshock capture's client half.
    pub shock: bool,
    /// `--probe-follow`: a spectator pressing the follow buttons, no fixture.
    pub follow: bool,
    /// `--probe-killcam`: a victim that waits its killcam out, no fixture.
    pub killcam: bool,
    /// `--probe-killcam-skip-ms`: press use this long into the killcam.
    pub killcam_skip_ms: Option<u64>,
    /// `--probe-kill-cmds`: the target's cmds behind each `kill`, cycled.
    pub kill_cmds: Vec<usize>,
    /// `--probe-fall`: print every snapshot a landing moved, no fixture.
    pub fall: bool,
    /// `--probe-fall-walk`: the world yaw `--probe-fall` walks at.
    pub fall_walk: Option<f32>,
    /// `--probe-fall-prone`: the world yaw `--probe-fall` lies prone at.
    pub fall_prone: Option<f32>,
    /// `--probe-pitch-flip`: the view pitch every other 400 ms window holds.
    pub pitch_flip: Option<f32>,
    /// `--probe-yaw`: the world yaw a cmd no other mode steers holds.
    pub yaw: Option<f32>,
    /// `--probe-ride`: print every snapshot's movement fields, no fixture.
    pub ride: bool,
    /// `--probe-items`: print every snapshot an item changed, no fixture.
    pub items: bool,
    /// `--probe-compass`: print every snapshot `iCompassFriendInfo` or the
    /// eye changed, no fixture.
    pub compass: bool,
    /// `--probe-flame`: a UO flamethrower victim running the flame report,
    /// no fixture.
    pub flame: bool,
    /// `--probe-ladder`: the world yaw the ladder climb holds forward at.
    pub ladder: Option<f32>,
    /// `--save-ladder [full|climb]`: the ladder climb capture and its script.
    pub save_ladder: Option<String>,
    /// `--probe-weapsel`: the play client's weapon selection against
    /// `client-probes/probe_weapsel`'s script switches and dry rifle.
    pub weapsel: bool,
    /// `--save-scripted <role>`: the scripted gametype capture.
    pub scripted: Option<String>,
    /// `--probe-download <dir>`: fetch the referenced paks the install
    /// lacks into this directory, then check each against its checksum.
    pub download: Option<std::path::PathBuf>,
    /// `--probe-rate` / `--probe-snaps` / `--probe-name`: userinfo values
    /// to connect with.
    pub rate: Option<String>,
    pub snaps: Option<String>,
    pub name: Option<String>,
}

/// Which script the two halves of the hit capture run. The target's own
/// behaviour is the same under all of them; the script picks the shooter's
/// phases and names both fixtures, so a grenade run does not overwrite the
/// committed bullet evidence.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShooterScript {
    /// The stock `--save-hit` shooter: approach, shot, burst, finish.
    #[default]
    Hit,
    /// `--probe-sweep`: one tap per pitch offset, no fixture.
    Sweep,
    /// `--probe-melee`: approach to [`MELEE_RANGE`], tap the melee bit.
    Melee,
    /// `--probe-grenade`: approach to [`GRENADE_RANGE`], cook 1 s, release at
    /// the target's feet, watch the missile, then throw one uncooked.
    Grenade,
    /// `--probe-grenade-death`: as `Grenade`, but `kill` 500 ms into the cook,
    /// which is what puts the death drop's missile on the wire.
    GrenadeDeath,
    /// `--save-roundrestart`'s shooter half: the same approach with no shot in
    /// it, then stand and let [`MapChangeProbe`] record the round out.
    RoundRestart,
}

impl ShooterScript {
    /// The fixture name both halves take: `<map>-<gametype>-<prefix>-<role>.txt`.
    fn prefix(self) -> &'static str {
        match self {
            ShooterScript::Hit | ShooterScript::Sweep => "hit",
            ShooterScript::Melee => "melee",
            ShooterScript::Grenade => "grenade",
            ShooterScript::GrenadeDeath => "grenade-death",
            ShooterScript::RoundRestart => "roundrestart",
        }
    }

    /// How close the approach walks before it stops closing. A bullet is
    /// thrown off by the spread past [`ENGAGE_RANGE`]; a melee reaches 64
    /// units and a thrown grenade wants to land at the target's feet.
    fn engage_range(self) -> f32 {
        match self {
            ShooterScript::Hit | ShooterScript::RoundRestart => ENGAGE_RANGE,
            ShooterScript::Sweep => SWEEP_RANGE,
            ShooterScript::Melee => MELEE_RANGE,
            ShooterScript::Grenade | ShooterScript::GrenadeDeath => GRENADE_RANGE,
        }
    }

    fn throws_grenades(self) -> bool {
        matches!(self, ShooterScript::Grenade | ShooterScript::GrenadeDeath)
    }
}

/// The `--probe-pitch-flip` window: the pitch is held this long, then 0 as
/// long.
const PITCH_FLIP_MS: u128 = 400;

/// Retail's melee trace reaches 64 units from the player's origin
/// (docs/research/cod11-combat.md, 2.5), so the swing has to land inside this.
pub const MELEE_RANGE: f32 = 40.0;
/// Close enough that a thrown grenade lands on the target rather than short of
/// it, and far enough that the thrower is not inside its own blast radius.
pub const GRENADE_RANGE: f32 = 300.0;

/// Connects, drives the client to `Active`, prints a summary per second and
/// nudges forward now and then to prove the server applies moves. With `fs`
/// it resolves drained events into sound cues; weapon-file cues (fire,
/// reload) are not resolved, the probe loads no weapon files.
///
/// `save_playerstate` turns it into the playerstate capture instead: it answers
/// the stock team and weapon menus, writes the fixture once the spawn has
/// settled and exits. It sends no nudge, which would walk the capture off the
/// spawn point.
///
/// `save_motion` joins the same way, then holds each movement input in turn
/// and captures the playerstate settled under each, which is what a standing
/// capture cannot show: every field the client predicts reads zero while the
/// player stands still.
///
/// `save_combat` joins the same way and runs the weapon script in
/// [`combat_script`], recording every snapshot rather than a settled sample:
/// a shot moves `weaponstate`, `weapAnim` and the four-slot event ring, and
/// the ring overwrites, so a settled sample cannot hold one. It is also the
/// one mode with the stall response on, so a walking step gets past the first
/// wall it meets.
///
/// `target` and `hit` are the two halves of the hit capture, one probe each:
/// the target stands still, kills itself at 10 s and records what the server
/// does to it, and the shooter walks up to it and shoots it. Both write their
/// own fixture, and the target's `kill` is what makes the death half
/// independent of whether the shooter ever finds a line of sight.
///
/// `pvs` joins the same way again, walks the route in [`pvs_route`] and prints
/// the snapshot entity list at each station plus every add and removal along
/// the way, answering what the server sends from where. `save.entities` walks
/// the same route and writes the trace as the entity-list fixture; `pvs` alone
/// writes nothing.
#[expect(clippy::too_many_arguments)]
pub fn probe(
    addr: &str,
    save: Save,
    tag: Option<String>,
    overwrite: bool,
    pvs: bool,
    slope: bool,
    triggers: bool,
    cmd_ms: u64,
    script: ShooterScript,
    team: Option<&str>,
    weapon: Option<&str>,
    secs: u64,
    say: &[(f32, String)],
    fs: Option<&vcod_common::pk3::Pk3Fs>,
) -> anyhow::Result<()> {
    let Save {
        vehicle: probe_vehicle,
        uo_weapon,
        fixture: save_fixture,
        snapshots: save_snapshots,
        demo: save_demo,
        configstrings: save_configstrings,
        playerstate: save_playerstate,
        motion: save_motion,
        slope: save_slope,
        slope_every_state,
        slope_from_spawn,
        prone: prone_uphill,
        moves: probe_moves,
        turns: probe_turns,
        sprint: probe_sprint,
        autofire: probe_autofire,
        talk: probe_talk,
        moveshot: probe_moveshot,
        pronemove: probe_pronemove,
        uofoot: probe_uofoot,
        uoturret: probe_uoturret,
        crouchjump: probe_crouchjump,
        crouchjump_stand,
        patchwalk: probe_patchwalk,
        combat: save_combat,
        ads: save_ads,
        grenade: save_grenade,
        sway: probe_sway,
        lean: save_lean,
        entities: save_entities,
        hit: save_hit,
        target: save_target,
        mapchange: save_mapchange,
        roundrestart: save_roundrestart,
        plant: save_plant,
        defuse: save_defuse,
        pickup: save_pickup,
        turret: save_turret,
        turret_target_ms,
        bump: save_bump,
        legs: save_legs,
        shock: save_shock,
        bump_target: probe_bump_target,
        follow: probe_follow,
        killcam: probe_killcam,
        killcam_skip_ms,
        kill_cmds,
        fall: probe_fall,
        fall_walk,
        fall_prone,
        pitch_flip,
        yaw: probe_yaw,
        ride: probe_ride,
        items: probe_items,
        compass: probe_compass,
        flame: probe_flame,
        ladder: probe_ladder,
        save_ladder,
        weapsel: probe_weapsel,
        scripted: scripted_role,
        download,
        rate,
        snaps,
        name,
    } = save;
    let mut download = download.map(|dir| DownloadProbe::new(dir, fs));
    let mut vehicle = probe_vehicle
        .map(crate::vehicle_probe::VehicleProbe::new)
        .or(uo_weapon.map(crate::vehicle_probe::VehicleProbe::on_foot));
    // The two map-cycle captures record the same lines; the flag picks the
    // role and, for the round restart, which half of the pair this probe is.
    let netchan_capture = save_mapchange || save_roundrestart;
    // The round restart's shooter half is the hit shooter's walk with the
    // firing phases taken out; its target half is the same target that kills
    // itself, which is what ends the round.
    let shooter_walk = save_roundrestart && !save_target;
    let script = if shooter_walk {
        ShooterScript::RoundRestart
    } else {
        script
    };
    // The ADS and grenade captures and the sway measurement are the combat
    // capture running another script.
    let save_combat = save_combat || save_ads || save_grenade || probe_sway || save_lean;
    // The fixture is the route's output, so the capture drives the same walk;
    // the slope measurement walks it too, with the sight held.
    // The prone crawl is the slope capture on another script.
    let prone_crawl = prone_uphill.is_some()
        || probe_moves
        || probe_turns
        || probe_sprint
        || probe_autofire
        || probe_talk
        || probe_moveshot.is_some()
        || probe_pronemove.is_some()
        || probe_uofoot.is_some()
        || probe_uoturret.is_some()
        || probe_crouchjump.is_some()
        || probe_patchwalk.is_some();
    let slope = slope || save_slope || prone_crawl;
    // A crew member's slope capture records its seats, not the route.
    let pvs = pvs || save_entities || (slope && !prone_crawl && probe_vehicle.is_none());
    let mut prone = if probe_moves {
        ProneScript::new(0.0, moves_script())
    } else if probe_turns {
        ProneScript::new(90.0, turns_script())
    } else if probe_sprint {
        ProneScript::new(0.0, sprint_script())
    } else if probe_autofire {
        ProneScript::new(0.0, autofire_script())
    } else if probe_talk {
        ProneScript::new(0.0, talk_script())
    } else if let Some(part) = probe_moveshot {
        ProneScript::new(0.0, moveshot_script(part))
    } else if let Some(part) = probe_pronemove {
        ProneScript::switching(0.0, pronemove_script(part), part.switches())
    } else if let Some(yaw) = probe_uofoot {
        ProneScript::switching(yaw, uofoot_script(), UOFOOT_SWITCHES)
    } else if let Some(yaw) = probe_uoturret {
        ProneScript::new(yaw, uoturret_script())
    } else if let Some(yaw) = probe_crouchjump {
        ProneScript::new(yaw, crouchjump_script(!crouchjump_stand))
    } else if let Some(yaw) = probe_patchwalk {
        ProneScript::new(yaw, patchwalk_script())
    } else {
        ProneScript::new(prone_uphill.unwrap_or(0.0), prone_script())
    };
    let mut slope_stats = SlopeStats::default();
    let mut slope_capture = SlopeCapture {
        every_state: slope_every_state,
        wait_log: probe_moveshot.is_some(),
        ..SlopeCapture::default()
    };
    // Every mode that needs a spawned player drives the same stock-menu join;
    // `--probe-team` alone joins and then just watches the roster.
    let joining = save_playerstate
        || save_motion
        || save_combat
        || save_hit
        || save_target
        || netchan_capture
        || pvs
        || prone_crawl
        || triggers
        || save_plant
        || save_defuse
        || save_pickup
        || save_turret
        || save_bump
        || save_legs
        || save_shock
        || probe_bump_target
        || probe_killcam
        || probe_fall
        || probe_ride
        || probe_items
        || probe_compass
        || probe_flame
        || probe_ladder.is_some()
        || probe_weapsel
        || scripted_role.is_some()
        || probe_vehicle.is_some()
        || uo_weapon.is_some()
        || team.is_some();
    // A sweep is a measurement, not a fixture: it walks a table of pitch
    // offsets instead of aiming at the eye, so the numbers it produces are not
    // what the committed shooter fixture holds.
    let sweep = script == ShooterScript::Sweep && save_hit;
    // The modes that write a capture into a committed fixture directory.
    let writes_fixture = save_fixture
        || save_snapshots
        || save_demo
        || save_configstrings
        || save_playerstate
        || save_motion
        || save_slope
        || (save_combat && !probe_sway)
        || save_entities
        || ((save_hit || save_target) && !sweep)
        || netchan_capture
        || save_plant
        || save_defuse
        || save_pickup
        || save_turret
        || save_bump
        || save_legs
        || save_shock
        || save_ladder.is_some()
        || probe_weapsel
        || scripted_role.is_some();
    let mut client = Connected(NetClient::connect(addr)?);
    if rate.is_some() || snaps.is_some() || name.is_some() {
        let def = net::Userinfo::default();
        client.set_userinfo(net::Userinfo {
            rate: rate.unwrap_or(def.rate),
            snaps: snaps.unwrap_or(def.snaps),
            name: name.unwrap_or(def.name),
            ..def
        });
    }
    // The demo's pair holds every message, so it never replaces the
    // snapshot-only `gamestate.bin` and `snapshots.bin`.
    let save_snapshots = save_snapshots || save_demo;
    if save_demo {
        anyhow::ensure!(tag.is_some(), "--save-demo needs a --capture-tag");
        client.enable_capture_all();
    } else if save_fixture || save_snapshots {
        client.enable_capture();
    }
    let mut quick_chat = crate::quick_chat::QuickChat::new(0x51ee);
    // `--probe-say`: the first gamestate and how many commands went out.
    let mut say_from: Option<Instant> = None;
    let mut say_sent = 0;
    // The playerstate's vote bit (`eFlags` 0x20000) as last printed.
    let mut voted_bit = 0;
    let mut demo_started = false;

    let start = Instant::now();
    let mut last_summary = start;
    let mut reached_active: Option<Instant> = None;
    let mut baseline_origin: Option<[f32; 3]> = None;
    let mut watch = ProbeWatch::default();
    // The S&D pair has to be on opposite teams: allies attack under sd.gsc's
    // default, so the plant joins them and the defuse joins axis.
    let team = team.or(if save_plant {
        Some("allies")
    } else if save_defuse {
        Some("axis")
    } else {
        None
    });
    let mut join = JoinProbe::new(team, weapon);
    let mut wrote_playerstate = false;
    let mut combat_died = false;
    let mut motion = MotionProbe::default();
    let mut combat = if save_grenade {
        CombatProbe::grenade()
    } else if save_ads {
        CombatProbe::ads()
    } else if probe_sway {
        CombatProbe::sway()
    } else if save_lean {
        CombatProbe::lean()
    } else {
        CombatProbe::default()
    };
    let mut hit = HitProbe::new(script);
    let mut target_probe = TargetProbe::new(if save_roundrestart {
        ROUNDRESTART_KILL_AT
    } else {
        TARGET_KILL_AT
    });
    target_probe.bursts = kill_cmds;
    anyhow::ensure!(
        !target_probe.bursts.contains(&0),
        "--probe-kill-cmds counts start at 1"
    );
    // `--probe-kill-cmds`: the last cmd sent, which a burst stamps on from,
    // and the end of the quiet after one.
    let mut last_sent: Option<net::msg::UserCmd> = None;
    let mut quiet_until: Option<Instant> = None;
    let mut wrote_hit = false;
    // The hit capture waits for the first spawn before its script starts.
    let mut spawned = false;
    let mut pvs_probe = PvsProbe::default();
    let mut trigger_probe = TriggerProbe::default();
    let mut netchan = MapChangeProbe::default();
    let mut sd = SdProbe::new(if save_plant {
        SdRole::Attacker
    } else {
        SdRole::Defender
    });
    let mut wrote_sd = false;
    // The S&D pair waits for its own first spawn the way the hit pair does.
    let mut sd_spawned = false;
    let mut pickup = PickupProbe::default();
    let mut wrote_pickup = false;
    let mut pickup_spawned = false;
    // `--probe-pitch-flip`'s last snapshot and the pitch it read.
    let mut last_pitch: Option<(u32, f32)> = None;
    let mut turret = TurretProbe {
        target_for: turret_target_ms.map(Duration::from_millis),
        ..TurretProbe::default()
    };
    let mut wrote_turret = false;
    let mut turret_spawned = false;
    // A tag starting `overlap` picks the walker's overlap script, which pairs
    // with the gsc's `probe_overlap 1`.
    // A tag starting `overlap-wall` runs it at the gsc's `probe_bump_spot
    // wall`, where the push drives the walker into a wall.
    let mut bump = BumpProbe::new(
        tag.as_deref().is_some_and(|t| t.starts_with("overlap")),
        tag.as_deref()
            .is_some_and(|t| t.starts_with("overlap-wall")),
    );
    let mut wrote_bump = false;
    // A tag starting `torso` fires and reloads on the move (probe_torso),
    // one starting `stopgo` stops and goes inside the legs blends.
    let mut legs =
        crate::legs_probe::LegsProbe::new(crate::legs_probe::Walk::of_tag(tag.as_deref()));
    let mut wrote_legs = false;
    let mut shock = crate::shock_probe::ShockProbe::default();
    let mut wrote_shock = false;
    let mut bump_spawned = false;
    let mut bump_target = BumpTarget::default();
    let mut follow = FollowProbe::default();
    let mut killcam = KillcamProbe::new(killcam_skip_ms);
    let mut fall = FallProbe::default();
    let mut ride = RideProbe::default();
    let mut items = ItemsProbe::default();
    let mut compass = CompassProbe::default();
    let mut flame = crate::flame_probe::FlameProbe::default();
    // The ladder and weapon-select probes drive the play client's input.
    let play_probe = probe_ladder.is_some() || probe_weapsel;
    let mut ladder = LadderProbe {
        weapsel: probe_weapsel,
        capture: save_ladder.is_some().then(Vec::new),
        script: match save_ladder.as_deref() {
            Some("climb") => LadderScript::Climb,
            Some("turn") => LadderScript::Turn,
            Some("grab") => LadderScript::Grab,
            _ => LadderScript::Full,
        },
        ..Default::default()
    };
    let save_ladder = save_ladder.is_some();
    let mut wrote_ladder = false;
    let mut scripted = net::capture::ScriptedCapture::default();
    let mut scripted_seen: Option<u32> = None;
    // The fixture is named for the map the run started on, which is not the
    // map cs 0 holds once the rotation has moved on.
    let mut first_map = String::new();
    let mut gamestates = 0u32;
    let mut last_score: Option<Instant> = None;
    // The full table; the map's loadspec filters it at gamestate.
    let aliases_all = fs.map(crate::audio::alias::AliasTable::load);

    loop {
        // A signal skips `finish_probe`: a cut-short run writes no fixture.
        if crate::quit::requested() {
            client.disconnect();
            anyhow::bail!("interrupted; sent disconnect");
        }
        let now = Instant::now();
        let events = client.pump_at(now);
        set_wire(client.protocol());
        if let Some(d) = &mut download {
            d.step(&mut client, &events, now)?;
        }
        for e in events {
            match e {
                NetEvent::GamestateReady => {
                    gamestates += 1;
                    // The serverinfo as the client holds it (handshake doc,
                    // "The challenge key": what `Cvar_InfoString` strips).
                    println!(
                        "cs 0 (serverinfo): \"{}\"",
                        escape_ctl(client.configstrings().first().map_or("", String::as_str))
                    );
                    if let Err(e) =
                        check_capture_tag(client.protocol(), writes_fixture, tag.as_deref())
                            .and_then(|()| {
                                check_fixture_names(writes_fixture, client.configstring(0))
                            })
                    {
                        client.disconnect();
                        return Err(e);
                    }
                    // Retail reruns `ClientConnect` on the new map, which
                    // offers the team menu again: a probe that does not
                    // re-answer it never spawns past the first map.
                    if gamestates > 1 {
                        println!("JOIN: gamestate {gamestates}, re-arming the menu answers");
                        join.rearm();
                    }
                    if netchan_capture {
                        netchan.on_gamestate(now, &client);
                    }
                    if scripted_role.is_some() {
                        scripted.protocol = client.protocol();
                        scripted.on_gamestate(
                            now.duration_since(start).as_millis() as i64,
                            client.server_id(),
                            net::info_value_for_key(client.configstring(0), "mapname")
                                .unwrap_or("?"),
                        );
                    }
                    // The `cp` a pure server waits for before the first cmd;
                    // the map's pak stands in for what a client reads loading it.
                    // Retail sends it once the downloads are over
                    // (`CL_DownloadsComplete`); an early one fails the check
                    // and the server drops the client on its next message.
                    // Under `--probe-download` the scratch directory is on
                    // the search path it reads.
                    let cp_fs = download.as_ref().and_then(DownloadProbe::fs).or(fs);
                    if let Some(fs) = cp_fs
                        && download.as_ref().is_none_or(|d| !d.holds_cmds())
                    {
                        let map = net::info_value_for_key(client.configstring(0), "mapname")
                            .and_then(|m| fs.resolve_map(m));
                        if let Some(bsp) = map {
                            fs.touch(&bsp);
                        }
                        let feed = client.gamestate().map_or(0, |g| g.checksum_feed);
                        let cp = fs.pure_command(feed, client.protocol().pure_dlls);
                        println!("PURE: {cp}");
                        client.send_reliable(&cp);
                    }
                    let gs = client.gamestate().unwrap();
                    println!("systeminfo: {}", gs.configstrings[1]);
                    println!(
                        "gamestate ready: clientNum {}, serverId {}, {} configstrings, {} baselines",
                        gs.client_num,
                        net::server_id_from_gamestate(gs),
                        gs.configstrings.iter().filter(|s| !s.is_empty()).count(),
                        gs.baselines.len(),
                    );
                    // The research configstring tables were taken from this dump.
                    for (i, cs) in client.configstrings().iter().enumerate() {
                        if !cs.is_empty() {
                            log::debug!("cs[{i}] = {cs:?}");
                        }
                    }
                    if save_configstrings {
                        write_configstrings_fixture(
                            client.configstrings(),
                            tag.as_deref(),
                            overwrite,
                        )?;
                    }
                    let cs = client.protocol().cs;
                    let (first, last) = cs.bounds(net::protocol::CsRange::Model);
                    for (i, m) in gs
                        .configstrings
                        .iter()
                        .enumerate()
                        .take(last + 1)
                        .skip(first)
                    {
                        if !m.is_empty() {
                            println!("model {}: {m}", i - cs.models());
                        }
                    }
                    // docs/research/cod11-sound-system.md, section 9.
                    println!("cs 3 (ambient): {:?}", gs.configstrings.get(3));
                    let sounds = cs.sounds();
                    let aliases: Vec<(usize, &String)> = gs
                        .configstrings
                        .iter()
                        .enumerate()
                        .skip(sounds)
                        .take(CS_SOUNDS_COUNT)
                        .filter(|(_, s)| !s.is_empty())
                        .collect();
                    println!(
                        "sound alias configstrings ({sounds}..{}): {} set",
                        sounds + CS_SOUNDS_COUNT,
                        aliases.len()
                    );
                    for (i, s) in &aliases {
                        println!("  cs {i} (alias {}): {s}", i - sounds);
                    }
                    if let Some(all) = &aliases_all {
                        let map = net::info_value_for_key(&gs.configstrings[0], "mapname")
                            .unwrap_or_default()
                            .to_string();
                        watch.aliases = all.for_map(&map);
                        println!(
                            "sound aliases: {} of {} apply on {map:?}",
                            watch.aliases.len(),
                            all.len()
                        );
                    }
                    if first_map.is_empty() {
                        first_map = net::info_value_for_key(&gs.configstrings[0], "mapname")
                            .unwrap_or("unknown")
                            .to_string();
                    }
                    if probe_flame {
                        let cs7 = gs.configstrings.get(7).map_or("", String::as_str);
                        flame.load(fs, &gs.configstrings[0], cs7);
                    }
                    if triggers {
                        let map = net::info_value_for_key(&gs.configstrings[0], "mapname")
                            .unwrap_or_default()
                            .to_string();
                        trigger_probe.load_map(fs, &map);
                    }
                    if save_plant || save_defuse {
                        // The bombzone's bounds and the map's collision, both
                        // out of the BSP the gamestate names.
                        let map = net::info_value_for_key(&gs.configstrings[0], "mapname")
                            .unwrap_or_default()
                            .to_string();
                        sd.load_map(fs, &map);
                    }
                    if save_pickup {
                        pickup.use_configstrings(client.configstrings());
                    }
                    if save_turret {
                        let map = net::info_value_for_key(&gs.configstrings[0], "mapname")
                            .unwrap_or_default()
                            .to_string();
                        turret.load_map(fs, &map);
                    }
                    if save_hit || shooter_walk || probe_sway || save_lean {
                        // The shooter's line-of-sight test and the sway run's
                        // sightline scan need the map's collision, and the
                        // gamestate is where the map is named.
                        let map = net::info_value_for_key(&gs.configstrings[0], "mapname")
                            .unwrap_or_default()
                            .to_string();
                        hit.load_map(fs, &map);
                        hit.use_configstrings(client.configstrings());
                        if sweep {
                            println!(
                                "HIT: sweeping {} pitch offsets, no fixture is written",
                                SWEEP_PITCH_OFFSETS.len()
                            );
                        }
                    }
                    // The join is entered by the first usercmd of the loop
                    // below, the way a retail client enters; nothing is sent
                    // here to start it.
                    say_from.get_or_insert(now);
                }
                NetEvent::Chat { text, team } => {
                    println!(
                        "CHAT {} \"{}\"",
                        if team { 'i' } else { 'h' },
                        escape_ctl(&text)
                    )
                }
                NetEvent::Print(t) => print!("print: {t}"),
                NetEvent::Dropped(r) => {
                    println!("dropped: {}", escape_ctl(&r));
                    return finish_probe(
                        &mut client,
                        save_fixture,
                        save_snapshots,
                        tag.as_deref(),
                        overwrite,
                    );
                }
                NetEvent::ConfigstringChanged(i) => {
                    println!("configstring {i} changed");
                    let vote = client.protocol().cs.vote;
                    if (vote..vote + 4).contains(&i) {
                        // The vote's time, string, yes and no counts, with
                        // the newest snapshot's time for the countdown.
                        println!(
                            "cs {i} (vote): \"{}\" at serverTime {:?}",
                            escape_ctl(client.configstrings().get(i).map_or("", String::as_str)),
                            client.snapshots().newest().map(|s| s.server_time)
                        );
                    } else if i == 0 {
                        println!(
                            "cs 0 (serverinfo): \"{}\"",
                            escape_ctl(client.configstrings().first().map_or("", String::as_str))
                        );
                    } else if i == 3 {
                        // `n\\<alias>\\t\\<fade end time>`.
                        println!("cs 3 (ambient): {:?}", client.configstrings().get(3));
                    } else if let sounds = client.protocol().cs.sounds()
                        && (sounds..sounds + CS_SOUNDS_COUNT).contains(&i)
                    {
                        println!(
                            "cs {i} (alias {}): {:?}",
                            i - sounds,
                            client.configstrings().get(i)
                        );
                    }
                }
                NetEvent::DownloadComplete(n) => println!("downloaded {n}"),
                // The probe plays no demo.
                NetEvent::DemoEnded(_) => {}
                NetEvent::OutOfBand(text) => {
                    println!("oob: {text:?}");
                    if netchan_capture {
                        netchan.on_oob(now, &text);
                    }
                }
                NetEvent::ServerCommand(tokens) => {
                    if probe_killcam {
                        killcam.on_server_command(now, &tokens);
                    }
                    if save_pickup {
                        pickup.on_server_command(now, &tokens);
                    }
                    if save_turret {
                        turret.on_server_command(now, &tokens);
                    }
                    // `b` is the scoreboard, one long line per second at round end.
                    if tokens.first().map(String::as_str) == Some("b") {
                        if save_target {
                            let ms = target_probe.elapsed_ms(now);
                            target_probe.scoreboards.push((ms, tokens.join(" ")));
                        }
                        if probe_follow {
                            follow.on_scoreboard(now, client.snapshots().newest(), &tokens);
                        }
                        continue;
                    }
                    println!("serverCommand: {tokens:?}");
                    if joining {
                        // `n` is retail's map restart (`B` on 1.5). It
                        // reruns `ClientConnect` too, and the menu it may
                        // reopen carries the indices the last one used.
                        if matches!(tokens.first().map(String::as_str), Some("n" | "B")) {
                            join.reopen_menus();
                        }
                        join.on_server_command(&tokens, &mut client, now);
                    }
                    // `s <idx>` is playLocalSound (sound doc, section 9).
                    if tokens.first().map(String::as_str) == Some("s") {
                        match tokens.get(1).and_then(|t| t.parse::<i32>().ok()) {
                            Some(idx) => {
                                match sound_cs_alias(client.configstrings(), client.protocol(), idx)
                                {
                                    Some((cs, name)) => {
                                        println!("  playLocalSound: cs {cs} = {name:?}")
                                    }
                                    None => println!("  playLocalSound: index {idx}: no alias"),
                                }
                            }
                            None => {
                                println!("  playLocalSound: <unparsed index {:?}>", tokens.get(1))
                            }
                        }
                    }
                    // Quick chat (`j/k/l`): resolve the category against the
                    // `.voice` tables when they exist; stock installs ship none.
                    if let Some(fs) = fs {
                        let newest = client.snapshots().newest();
                        let protocol = client.protocol();
                        let handled = quick_chat.on_server_command(fs, &tokens, |num| {
                            newest
                                .and_then(|s| s.clients.get(&num))
                                .map(|c| c.field_i32(protocol, "team"))
                        });
                        if handled {
                            while let Some(line) = quick_chat.drain(f32::INFINITY) {
                                println!(
                                    "  quickchat: {:?} -> {:?} ({line_text})",
                                    line.cue.source,
                                    line.cue.alias,
                                    line_text = line.text
                                );
                            }
                        }
                    }
                }
            }
        }

        if joining {
            let seq = client.command_sequence();
            let cmds = client.take_server_commands();
            if netchan_capture {
                netchan.on_commands(now, seq, &cmds);
            }
            if scripted_role.is_some() {
                scripted.on_commands(
                    now.duration_since(start).as_millis() as i64,
                    &cmds,
                    client.configstrings(),
                );
            }
            for cmd in cmds {
                if play_probe {
                    ladder.on_command(now, &cmd);
                }
                if let Some(v) = vehicle.as_mut() {
                    v.on_command(&cmd);
                }
                println!("JOIN cmd: {cmd}");
                join.commands.push(cmd);
            }
        }

        let mut cmd = net::msg::UserCmd::default();
        let ps_weapon = client
            .snapshots()
            .newest()
            .map_or(0, |s| s.ps.field_i32(client.protocol(), "weapon") as u8);
        // A step or a phase asking for a weapon switch: an index that differs
        // from `ps.weapon`, which is what retail reads as the request. It
        // overrides the follow below until the switch lands.
        let mut weapon_switch: Option<u8> = None;
        if let Some(s) = client.snapshots().newest()
            && !say.is_empty()
        {
            let bit = s.ps.field_i32(client.protocol(), "eFlags") & 0x20000;
            if bit != voted_bit {
                println!(
                    "ps.eFlags vote bit {bit:#x} at serverTime {}",
                    s.server_time
                );
                voted_bit = bit;
            }
        }
        if let Some(from) = say_from {
            while let Some((secs, cmd)) = say.get(say_sent)
                && now.duration_since(from).as_secs_f32() >= *secs
            {
                let cmd = cmd.replace("{sid}", &client.server_id().to_string());
                println!("SAY {:.1} {cmd}", now.duration_since(from).as_secs_f32());
                client.send_reliable(&cmd);
                say_sent += 1;
            }
        }
        // 5 s after going active push forward for 2 s, again every 30 s, so a
        // long run shows whether moves still apply after a map_restart.
        if client.state() == NetState::Active && !joining && !probe_follow {
            let active_at = *reached_active.get_or_insert(now);
            let dt = now.duration_since(active_at).as_secs() % 30;
            if (5..7).contains(&dt) {
                cmd.forward = 127;
            }
        }
        if let Some(v) = vehicle.as_mut() {
            let newest = client.snapshots().newest();
            let p = client.protocol();
            let live = newest.is_some_and(|s| s.ps.field_i32(p, "pm_type") == PM_NORMAL);
            if let Some(s) = newest {
                v.report(s, p);
            }
            cmd = v.cmd(now, live, newest, p);
            weapon_switch = v.weapon();
            if let Some(say) = v.take_say() {
                println!("VEH say {say}");
                client.send_reliable(say);
            }
        } else if scripted_role.is_some() {
            cmd = scripted.cmd(
                now.duration_since(start).as_millis() as i64,
                client.snapshots().newest(),
            );
        } else if save_motion && motion.running() {
            cmd = motion.cmd();
            hold_view_yaw(&mut cmd, client.snapshots().newest(), &mut motion.spawn_yaw);
        } else if save_combat && combat.running() {
            cmd = combat.cmd();
            weapon_switch = combat.weapon_override(ps_weapon);
            // Only a step that declares itself a walk is steered. A turn on
            // every step would leave the detour a stalled walk earned on the
            // cmd of every later step that pushes forward, while the
            // fixture's `!input` records the untouched yaw.
            if combat.step_walks()
                && let Some(o) = client
                    .snapshots()
                    .newest()
                    .map(|s| s.ps.origin(client.protocol()))
            {
                combat.stall.apply(&mut cmd, now, o);
            }
            match combat.aim_yaw {
                Some(yaw) => cmd.angles[1] = yaw,
                None => hold_view_yaw(&mut cmd, client.snapshots().newest(), &mut combat.spawn_yaw),
            }
        } else if (save_hit || shooter_walk) && hit.running() {
            // No `hold_view_yaw`: the aim is absolute, and the shooter's own
            // `viewangles` measured against the bearing say the server takes
            // the usercmd word as the view.
            cmd = hit.cmd(now);
            weapon_switch = hit.weapon_override(ps_weapon);
        } else if save_target && target_probe.running() {
            cmd = target_probe.cmd();
        } else if (save_plant || save_defuse) && sd.running() {
            // No `hold_view_yaw`: the aim is absolute, at the bombzone or the
            // bomb, the way the hit shooter's is.
            cmd = sd.cmd(now);
        } else if save_pickup && pickup.running() {
            // No `hold_view_yaw`: the aim is absolute, at an item or at the
            // view the snapshot reports.
            cmd = pickup.cmd(now);
            weapon_switch = pickup.weapon_byte(ps_weapon);
        } else if save_turret && turret.running() {
            // No `hold_view_yaw`: the aim is absolute, off the gun's yaw.
            cmd = turret.cmd(now);
        } else if save_legs && legs.running() && client.state() == NetState::Active {
            // Absolute: `send_frame` takes `delta_angles` off.
            cmd = legs.cmd(now);
        } else if save_shock && shock.running() && client.state() == NetState::Active {
            cmd = shock.cmd(now);
        } else if save_bump && bump.running() {
            // Absolute yaw 0, the +x the gsc faced both players along.
            cmd = bump.cmd();
        } else if probe_bump_target {
            cmd = bump_target.cmd();
        } else if probe_flame && client.state() == NetState::Active {
            let flood = client.flood_window_open();
            let p = client.protocol();
            if let Some(fdc) = client
                .snapshots()
                .newest()
                .and_then(|s| flame.frame(now, p, s, flood, (fs, client.configstrings())))
            {
                client.send_reliable(&fdc);
            }
            cmd = flame.cmd();
        } else if probe_follow && client.state() == NetState::Active {
            cmd = follow.cmd(now);
        } else if probe_killcam {
            cmd = killcam.cmd(now);
        } else if triggers && trigger_probe.running() {
            // No `hold_view_yaw`: the walk steers at a world position, so its
            // yaw is a bearing rather than a heading off the spawn's facing.
            cmd = trigger_probe.cmd();
        } else if prone_crawl && prone.running() {
            // No `hold_view_yaw`: the script's angles are world angles, which
            // `send_frame` rebases on each snapshot's `delta_angles`.
            cmd = prone.cmd(now);
            weapon_switch = prone.weapon_byte(now, ps_weapon, client.configstrings());
        } else if let Some(pitch) = pitch_flip.filter(|_| client.state() == NetState::Active) {
            // Absolute, as below: world yaw 0, the pitch in alternate windows.
            let active_at = *reached_active.get_or_insert(now);
            if (now.duration_since(active_at).as_millis() / PITCH_FLIP_MS) % 2 == 1 {
                cmd.angles[0] = deg_to_short(pitch);
            }
            cmd.forward = 0;
            if let Some(s) = client.snapshots().newest() {
                let p = s.ps.viewangles(client.protocol())[0];
                if last_pitch.is_none_or(|(n, _)| n != s.message_num)
                    && last_pitch.is_none_or(|(_, q)| q != p)
                {
                    println!("PITCH serverTime={} pitch={p:.2}", s.server_time);
                }
                last_pitch = Some((s.message_num, p));
            }
        } else if let Some(yaw) = fall_prone.filter(|_| client.state() == NetState::Active) {
            // Held as a retail client holds `cl_stance` 2, whatever the
            // server forces: a refusal repeats on every cmd.
            cmd.wbuttons |= net::msg::WBUTTON_PRONE;
            cmd.up = -127;
            cmd.angles[1] = deg_to_short(yaw);
        } else if play_probe && client.state() == NetState::Active {
            let cs7 = client.configstrings().get(7).map_or("", String::as_str);
            ladder.load_weapons(fs, cs7);
            cmd = ladder.cmd(
                now,
                join.settled(now),
                probe_ladder,
                client.snapshots().newest(),
            );
        } else if let Some(yaw) = fall_walk.filter(|_| client.state() == NetState::Active) {
            // Absolute: `send_frame` takes `delta_angles` off.
            cmd.forward = 127;
            cmd.angles[1] = deg_to_short(yaw);
        } else if pvs && pvs_probe.running() {
            cmd = pvs_probe.cmd();
            if slope {
                cmd.buttons |= net::msg::BUTTON_ADS;
            }
            hold_view_yaw(
                &mut cmd,
                client.snapshots().newest(),
                &mut pvs_probe.spawn_yaw,
            );
        } else if let Some(yaw) = probe_yaw.filter(|_| client.state() == NetState::Active) {
            // Absolute: `send_frame` takes `delta_angles` off.
            cmd.angles[1] = deg_to_short(yaw);
        }
        // A retail client sends the weapon it is holding on every cmd, and
        // so does the probe: retail's weapon-change check reads a
        // `cmd.weapon` that differs from `ps.weapon` as a request to holster
        // (docs/research/cod11-combat.md, section 1.8), so a zero would
        // carry putaways the input never asked for. The
        // byte only travels in the full usercmd branch, which a `wbuttons`,
        // `upmove` or `weapon` change forces (docs/protocol-1.1.md).
        // The ladder and weapon-select probes' byte is the play client's own.
        if !play_probe {
            cmd.weapon = weapon_switch.unwrap_or(ps_weapon);
        }
        // A retail client stays `CA_CONNECTED` until its downloads end and
        // creates no usercmds before `CA_PRIMED`, so nothing enters the
        // world mid-download.
        let sent = if download.as_ref().is_some_and(DownloadProbe::holds_cmds)
            || quiet_until.is_some_and(|t| now < t)
        {
            None
        } else {
            client.send_frame(&cmd)
        };
        last_sent = sent.or(last_sent);
        if let Some(c) = sent {
            // `--probe-vehicle`, `--probe-uo-weapon`, `--probe-ride` and
            // `--probe-items` log their cmd times too, which a gate replays
            // where a landing rides the split.
            if probe_fall || probe_ride || probe_items || vehicle.is_some() {
                fall.spawned(client.snapshots().newest());
                fall.record_cmd(
                    c.server_time,
                    fall_walk.map(|_| c.angles[1]),
                    (c.buttons, c.wbuttons, c.up),
                    client.last_sent_sequence(),
                    client.snapshots(),
                );
            }
            if save_plant || save_defuse {
                sd.record(c);
            }
            if save_pickup {
                pickup.record(now, c, cmd.angles);
            }
            if save_turret {
                turret.record(now, c, cmd.angles, client.last_sent_sequence());
            }
            if save_bump {
                bump.record(now, c);
            }
            if probe_ladder.is_some() {
                ladder.record(c);
            }
            if play_probe {
                ladder.sent_cmd(now, &c);
            }
            if save_legs {
                legs.record(now, c, cmd.angles);
            }
            if save_shock {
                shock.record(now, c);
            }
            if save_combat {
                combat.record(&c, cmd.angles);
            }
        }
        if save_slope && let Some(c) = sent {
            slope_capture.sent(c);
        }

        // Retail sends the `b` scoreboard only in answer to `score`, so the
        // one a map end produces is in the capture only if it is asked for.
        // The round-restart pair's target half already asks on its own.
        if (save_mapchange || scripted_role.is_some())
            && last_score.is_none_or(|t| now.duration_since(t) >= TARGET_SCORE_PERIOD)
        {
            last_score = Some(now);
            client.send_reliable("score");
        }

        // Every iteration, not once a second; the event rings hold four slots
        // and `loopSound` can come and go between summaries.
        if let Some(s) = client.snapshots().newest() {
            // Outside the `pm_type == PM_NORMAL` gate below on purpose: the
            // trigger walk's job is to die in a minefield and get up again,
            // and the use press that gets it up only matters while the
            // playerstate says it is dead.
            if triggers && join.settled(now) {
                trigger_probe.observe(now, s);
            }
            if save_slope {
                slope_capture.observe_wait(s);
            }
            if slope && join.settled(now) {
                slope_stats.observe(now, s);
                if save_slope && (!prone_crawl || prone.recording()) {
                    slope_capture.observe(s);
                }
            } else if save_slope && slope_from_spawn {
                slope_capture.observe(s);
            }
            if probe_follow {
                follow.observe(now, s);
            }
            if probe_killcam && join.settled(now) {
                let me = client.gamestate().map_or(-1, |g| g.client_num);
                killcam.observe(now, me, s);
            }
            if probe_fall {
                fall.observe(s);
            }
            if probe_ride {
                ride.observe(s);
            }
            if probe_items {
                items.observe(s);
            }
            if probe_compass {
                compass.observe(s);
            }
            if play_probe {
                ladder.observe(now, s);
            }
            watch.check_sounds(s, client.configstrings(), client.protocol());
            watch.check_movers(s, client.protocol());
            if netchan_capture {
                netchan.sample(now, s);
            }
            if scripted_role.is_some() && scripted_seen != Some(s.message_num) {
                scripted_seen = Some(s.message_num);
                scripted.sample(
                    now.duration_since(start).as_millis() as i64,
                    s,
                    client.configstrings(),
                );
            }
        }

        // Outside the snapshot borrow above: the wedge escape's `kill` is a
        // reliable command and the borrow is immutable.
        for c in std::mem::take(&mut trigger_probe.pending) {
            client.send_reliable(&c);
        }

        if now.duration_since(last_summary) >= Duration::from_secs(1) {
            last_summary = now;
            if let Some(s) = client.snapshots().newest() {
                let o = s.ps.origin(client.protocol());
                let base = *baseline_origin.get_or_insert(o);
                let moved = ((o[0] - base[0]).powi(2)
                    + (o[1] - base[1]).powi(2)
                    + (o[2] - base[2]).powi(2))
                .sqrt();
                // `cmdLag` is serverTime minus the echoed ps.commandTime: how
                // far behind the server's execution of our usercmds runs. A
                // healthy live connection stays under ~100 ms.
                println!(
                    "[{:>3}s] {:?} sid={} snap #{} delta={} t={} cmdLag={} ps.origin=[{:.0},{:.0},{:.0}] moved {:.0}u, {} entities",
                    now.duration_since(start).as_secs(),
                    client.state(),
                    client.server_id(),
                    s.message_num,
                    s.delta_num,
                    s.server_time,
                    s.server_time - s.ps.field_i32(client.protocol(), "commandTime"),
                    o[0],
                    o[1],
                    o[2],
                    moved,
                    s.entities.len(),
                );
                if !s.clients.is_empty() {
                    let p = client.protocol();
                    let roster: Vec<String> = s
                        .clients
                        .iter()
                        .map(|(num, c)| {
                            format!("{num}:team={} {:?}", c.field_i32(p, "team"), c.name(p))
                        })
                        .collect();
                    println!("  roster: {}", roster.join("  "));
                }
                if triggers {
                    println!("  {}", trigger_probe.status(s));
                }
                watch.check(s, client.configstrings(), client.protocol());
            } else {
                println!(
                    "[{:>3}s] {:?}, no snapshot yet",
                    now.duration_since(start).as_secs(),
                    client.state()
                );
            }
        }

        // The hit capture is the one mode that runs across a death, so it is
        // not gated on `pm_type` the way the settled captures below are: what
        // the server does to a dead player is what it is here to record. It
        // still waits for the first spawn, so the script does not start while
        // the join is still on the menus.
        if (save_hit || save_target || shooter_walk) && join.settled(now) && !wrote_hit {
            let done = match client.snapshots().newest() {
                Some(s) => {
                    spawned |= s.ps.field_i32(client.protocol(), "pm_type") == PM_NORMAL;
                    if !spawned {
                        false
                    } else if save_target {
                        target_probe.step(now, s)
                    } else {
                        hit.use_weapon(fs, &join.weapon);
                        hit.step(now, s)
                    }
                }
                None => false,
            };
            let pending: Vec<String> = target_probe
                .pending
                .drain(..)
                .chain(hit.pending.drain(..))
                .collect();
            for c in pending {
                client.send_reliable(&c);
            }
            // Each message carries the queued `kill`; the server runs it
            // once, ahead of the first message's cmd.
            if let Some(n) = target_probe.burst.take()
                && let Some(last) = last_sent
            {
                for k in 1..=n as i32 {
                    client.send_cmds(&[net::msg::UserCmd {
                        server_time: last.server_time + k,
                        ..last
                    }]);
                }
                quiet_until = Some(now + KILL_CMDS_QUIET);
            }
            // A map-cycle capture runs on the probe's own clock and writes
            // its own fixture below; the script here only supplies its motion.
            if done && !netchan_capture {
                if save_target {
                    write_target_fixture(
                        script,
                        client.configstrings(),
                        &join,
                        &target_probe,
                        now,
                        tag.as_deref(),
                        overwrite,
                    )?;
                } else if !sweep {
                    write_shooter_fixture(
                        client.configstrings(),
                        &join,
                        &hit,
                        now,
                        tag.as_deref(),
                        overwrite,
                    )?;
                }
                wrote_hit = true;
                break;
            }
        }

        // The S&D pair runs across a link, whose `pm_type` is 1 rather than
        // `PM_NORMAL`, so it is not gated on the playerstate the way the
        // settled captures below are; it only waits for the first spawn.
        if (save_plant || save_defuse) && join.settled(now) && !wrote_sd {
            let done = match client.snapshots().newest() {
                Some(s) => {
                    sd_spawned |= s.ps.field_i32(client.protocol(), "pm_type") == PM_NORMAL;
                    sd_spawned && sd.step(now, s)
                }
                None => false,
            };
            if done {
                write_sd_fixture(
                    client.configstrings(),
                    &join,
                    &sd,
                    tag.as_deref(),
                    overwrite,
                )?;
                wrote_sd = true;
                break;
            }
        }

        if save_pickup
            && !join.settled(now)
            && let Some(s) = client.snapshots().newest()
        {
            pickup.before_settle(s);
        }
        if save_pickup && join.settled(now) && !wrote_pickup {
            let done = match client.snapshots().newest() {
                Some(s) => {
                    pickup_spawned |= s.ps.field_i32(client.protocol(), "pm_type") == PM_NORMAL;
                    pickup_spawned && pickup.step(now, s)
                }
                None => false,
            };
            if done {
                write_pickup_fixture(
                    client.configstrings(),
                    &join,
                    &pickup,
                    tag.as_deref(),
                    overwrite,
                )?;
                wrote_pickup = true;
                break;
            }
        }

        if save_turret && join.settled(now) && !wrote_turret {
            let done = match client.snapshots().newest() {
                Some(s) => {
                    turret_spawned |= s.ps.field_i32(client.protocol(), "pm_type") == PM_NORMAL;
                    turret_spawned && turret.step(now, s)
                }
                None => false,
            };
            if done {
                write_turret_fixture(
                    client.configstrings(),
                    &join,
                    &turret,
                    tag.as_deref(),
                    overwrite,
                )?;
                wrote_turret = true;
                break;
            }
        }

        // Not gated on `join.settled`: the overlap script is clocked off the
        // placement's server time, which lands before the settle does.
        if save_bump && !wrote_bump {
            let done = match client.snapshots().newest() {
                Some(s) => {
                    bump_spawned |= s.ps.field_i32(client.protocol(), "pm_type") == PM_NORMAL;
                    bump_spawned && bump.step(now, s)
                }
                None => false,
            };
            if done {
                write_bump_fixture(
                    client.configstrings(),
                    &join,
                    &bump,
                    tag.as_deref(),
                    overwrite,
                )?;
                wrote_bump = true;
                break;
            }
        }

        if save_ladder && !wrote_ladder && ladder.captured() {
            write_ladder_fixture(
                client.configstrings(),
                &ladder,
                probe_ladder.unwrap_or(0.0),
                tag.as_deref(),
                overwrite,
            )?;
            wrote_ladder = true;
            break;
        }

        if save_legs
            && !wrote_legs
            && let Some(s) = client.snapshots().newest()
            && legs.observe(now, s)
        {
            legs.write(
                client.configstrings(),
                &join.team,
                &join.weapon,
                MOVERS_FIXTURE_DIR,
                tag.as_deref(),
                overwrite,
            )?;
            wrote_legs = true;
            break;
        }

        if save_shock
            && !wrote_shock
            && let Some(s) = client.snapshots().newest()
            && shock.observe(now, s)
        {
            shock.write(
                client.configstrings(),
                &join.team,
                &join.weapon,
                tag.as_deref(),
                overwrite,
            )?;
            wrote_shock = true;
            break;
        }

        if probe_bump_target
            && join.settled(now)
            && let Some(s) = client.snapshots().newest()
            && s.ps.field_i32(client.protocol(), "pm_type") == PM_NORMAL
        {
            bump_target.step(now, s);
        }

        // The combat script steps only while alive and cannot resume from a
        // respawn elsewhere, and stock gametypes without `scr_forcerespawn`
        // never respawn a probe that presses nothing: a death (its own frag,
        // or another client on a stock dm) would otherwise idle out the run
        // as "raise --probe-secs".
        if save_combat
            && combat.running()
            && let Some(s) = client.snapshots().newest()
            && s.ps.field_i32(client.protocol(), "pm_type") == PM_DEAD
        {
            println!(
                "no combat fixture: died on step {} of {} ({}); the script cannot resume from a respawn",
                combat.idx + 1,
                combat.steps.len(),
                combat.steps[combat.idx.min(combat.steps.len() - 1)].label
            );
            combat_died = true;
            break;
        }
        // `uofoot_script` ends in the hand-held frag's kill: the run is over.
        if probe_uofoot.is_some()
            && prone.recording()
            && let Some(s) = client.snapshots().newest()
            && s.ps.field_i32(client.protocol(), "pm_type") == PM_DEAD
        {
            println!("uofoot: the cooked frag killed the thrower, ending the run");
            break;
        }

        // A refused weapon reopens the same menu, which the probe answers
        // once and then ignores, so a sent answer is not an accepted one; the
        // playerstate is what tells a spawn from a still-spectating client.
        if join.settled(now)
            && let Some(s) = client.snapshots().newest()
            && s.ps.field_i32(client.protocol(), "pm_type") == PM_NORMAL
        {
            if save_motion {
                if motion.step(now, s) {
                    write_motion_fixture(
                        s,
                        client.configstrings(),
                        &join,
                        &motion,
                        tag.as_deref(),
                        overwrite,
                    )?;
                    wrote_playerstate = true;
                    break;
                }
            } else if save_combat {
                // The join names the weapon; the reload step is sized
                // off its `reloadTime` rather than one rifle's number.
                let fresh = !combat.weapon_read && !join.weapon.is_empty();
                combat.use_weapon(fs, &join.weapon, client.configstrings());
                if (probe_sway || save_lean)
                    && combat.aim_yaw.is_none()
                    && let Some(w) = hit.world.as_deref()
                {
                    let p = client.protocol();
                    let o = s.ps.origin(p);
                    let eye = [o[0], o[1], o[2] + s.ps.field_f32(p, "viewHeightCurrent")];
                    combat.aim_yaw = Some(if save_lean {
                        lean_sightline(w, eye)
                    } else {
                        longest_sightline(w, eye)
                    });
                }
                // The first step starts an iteration after the reads above:
                // the weapon file and the sight line take 100-200 ms on the
                // first settled snapshot, and a step begun on it opened with
                // a snapshot gap and a first cmd whose msec spanned it, so
                // `melee_tap`'s whole swing ran inside one chopped cmd.
                if !fresh && combat.step(Instant::now(), s) {
                    // The sway run is a measurement: its impact lines
                    // went to stdout and there is no fixture for it.
                    if !probe_sway {
                        write_combat_fixture(
                            client.configstrings(),
                            &join,
                            &combat,
                            tag.as_deref(),
                            overwrite,
                        )?;
                    }
                    wrote_playerstate = true;
                    break;
                }
            } else if prone_crawl {
                if prone.step(now, s) {
                    if save_slope {
                        write_slope_fixture(
                            client.configstrings(),
                            &join,
                            &slope_capture,
                            cmd_ms,
                            if probe_moves {
                                SlopeWalk::Moves
                            } else if probe_turns {
                                SlopeWalk::Turns
                            } else if probe_sprint {
                                SlopeWalk::Sprint
                            } else if probe_autofire {
                                SlopeWalk::Autofire
                            } else if probe_talk {
                                SlopeWalk::Talk
                            } else if let Some(part) = probe_moveshot {
                                SlopeWalk::MoveShot(part)
                            } else if let Some(part) = probe_pronemove {
                                SlopeWalk::ProneMove(part)
                            } else if probe_uofoot.is_some() {
                                SlopeWalk::UoFoot
                            } else if probe_uoturret.is_some() {
                                SlopeWalk::UoTurret
                            } else if probe_crouchjump.is_some() {
                                SlopeWalk::CrouchJump
                            } else if probe_patchwalk.is_some() {
                                SlopeWalk::PatchWalk
                            } else {
                                SlopeWalk::Prone
                            },
                            client.protocol(),
                            tag.as_deref(),
                            overwrite,
                        )?;
                        wrote_playerstate = true;
                    }
                    break;
                }
            } else if pvs {
                if pvs_probe.step(now, s) {
                    pvs_probe.report();
                    if save_slope {
                        write_slope_fixture(
                            client.configstrings(),
                            &join,
                            &slope_capture,
                            cmd_ms,
                            SlopeWalk::Route,
                            client.protocol(),
                            tag.as_deref(),
                            overwrite,
                        )?;
                        wrote_playerstate = true;
                    }
                    if save_entities {
                        write_entities_fixture(
                            &pvs_probe,
                            client.configstrings(),
                            &join,
                            tag.as_deref(),
                        )?;
                    }
                    break;
                }
            } else if save_playerstate {
                write_playerstate_fixture(
                    s,
                    client.configstrings(),
                    &join,
                    tag.as_deref(),
                    overwrite,
                )?;
                wrote_playerstate = true;
                break;
            }
            // `--probe-team` on its own writes nothing: it stays on
            // and reports the roster, which is what a second probe on
            // the other team needs to be seen by.
        }
        // `record` is refused until `CA_ACTIVE`, the first snapshot in
        // (CoDMP.exe 0x40e540); the earliest it can start is right after.
        if save_demo
            && !demo_started
            && client.state() == NetState::Active
            && client.snapshots().newest().is_some()
        {
            demo_started = true;
            let path = format!(
                "tmp/probe-{}.dm_{}",
                tag.as_deref().unwrap_or("demo"),
                net::demo::extension_number(client.protocol())
            );
            std::fs::create_dir_all("tmp")?;
            let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
            client.start_recording(Box::new(file), "probe")?;
            println!("RECORD: {path} from message {}", client.incoming_sequence());
        }
        // A tagged capture runs for `--probe-secs`: the 1.5 pair
        // (`--capture-tag 1.5`) wants a spawned player's deltas.
        if let Some(count) = client.capture_count()
            && count >= SNAP_CAPTURE_TARGET
            && tag.is_none()
        {
            break;
        }
        if now.duration_since(start) >= Duration::from_secs(secs) {
            break;
        }
        std::thread::sleep(Duration::from_millis(cmd_ms));
    }
    if triggers {
        trigger_probe.report();
    }
    if slope {
        slope_stats.report();
    }
    if save_slope && !wrote_playerstate {
        println!("slope: the run ended before the route did, writing what it has");
        write_slope_fixture(
            client.configstrings(),
            &join,
            &slope_capture,
            cmd_ms,
            if probe_moves {
                SlopeWalk::Moves
            } else if probe_turns {
                SlopeWalk::Turns
            } else if probe_sprint {
                SlopeWalk::Sprint
            } else if probe_autofire {
                SlopeWalk::Autofire
            } else if probe_talk {
                SlopeWalk::Talk
            } else if let Some(part) = probe_moveshot {
                SlopeWalk::MoveShot(part)
            } else if let Some(part) = probe_pronemove {
                SlopeWalk::ProneMove(part)
            } else if probe_uofoot.is_some() {
                SlopeWalk::UoFoot
            } else if probe_uoturret.is_some() {
                SlopeWalk::UoTurret
            } else if probe_crouchjump.is_some() {
                SlopeWalk::CrouchJump
            } else if probe_patchwalk.is_some() {
                SlopeWalk::PatchWalk
            } else if prone_crawl {
                SlopeWalk::Prone
            } else if probe_vehicle.is_some() {
                SlopeWalk::Vehicle
            } else {
                SlopeWalk::Route
            },
            client.protocol(),
            tag.as_deref(),
            overwrite,
        )?;
    }

    // The run's clock can end before the script does; what it has by then is
    // still the capture, and the header says how long it ran.
    if (save_hit || save_target) && !wrote_hit && !sweep && !netchan_capture {
        let now = Instant::now();
        println!("hit: the run ended before the script did, writing what it has");
        if save_target {
            write_target_fixture(
                script,
                client.configstrings(),
                &join,
                &target_probe,
                now,
                tag.as_deref(),
                overwrite,
            )?;
        } else {
            write_shooter_fixture(
                client.configstrings(),
                &join,
                &hit,
                now,
                tag.as_deref(),
                overwrite,
            )?;
        }
    }
    if probe_fall || probe_ride || probe_items || vehicle.is_some() {
        fall.flush();
    }
    if (save_plant || save_defuse) && !wrote_sd {
        println!(
            "sd: the run ended in {} before the script did, writing what it has",
            sd.phase.label()
        );
        // The phase it stopped in is what the file is short of; a reader has
        // to see that before trusting it as the pair's evidence. `Done` never
        // reaches here: the step that enters it writes the file in the loop.
        sd.notes
            .push(format!("# BROKEN run ended in {}", sd.phase.label()));
        write_sd_fixture(
            client.configstrings(),
            &join,
            &sd,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if probe_weapsel {
        write_weapsel_fixture(
            client.configstrings(),
            &join,
            &ladder,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if save_pickup && !wrote_pickup {
        println!(
            "pickup: the run ended in {} before the script did, writing what it has",
            pickup.phase.label()
        );
        pickup
            .notes
            .push(format!("# BROKEN run ended in {}", pickup.phase.label()));
        write_pickup_fixture(
            client.configstrings(),
            &join,
            &pickup,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if save_turret && !wrote_turret {
        println!(
            "turret: the run ended in {} before the script did, writing what it has",
            turret.phase.label()
        );
        turret
            .notes
            .push(format!("# BROKEN run ended in {}", turret.phase.label()));
        write_turret_fixture(
            client.configstrings(),
            &join,
            &turret,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if save_ladder && !wrote_ladder {
        println!("ladder: the run ended before the script did, writing what it has");
        ladder
            .notes
            .push("# BROKEN run ended before the script did".into());
        write_ladder_fixture(
            client.configstrings(),
            &ladder,
            probe_ladder.unwrap_or(0.0),
            tag.as_deref(),
            overwrite,
        )?;
    }
    if save_shock && !wrote_shock {
        println!("shock: the run ended before the script did, writing what it has");
        shock.write(
            client.configstrings(),
            &join.team,
            &join.weapon,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if save_legs && !wrote_legs {
        println!("legs: the run ended before the script did, writing what it has");
        legs.write(
            client.configstrings(),
            &join.team,
            &join.weapon,
            MOVERS_FIXTURE_DIR,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if save_bump && !wrote_bump {
        println!(
            "bump: the run ended in {} before the script did, writing what it has",
            bump.label()
        );
        bump.notes
            .push(format!("# BROKEN run ended in {}", bump.label()));
        write_bump_fixture(
            client.configstrings(),
            &join,
            &bump,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if netchan_capture {
        let role = if save_mapchange {
            "mapchange"
        } else if save_target {
            "roundrestart-target"
        } else {
            "roundrestart-shooter"
        };
        write_mapchange_fixture(
            role,
            client.configstrings(),
            &join,
            &netchan,
            &first_map,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if let Some(role) = &scripted_role {
        write_scripted_fixture(
            role,
            client.configstrings(),
            &join,
            &scripted,
            &first_map,
            tag.as_deref(),
            overwrite,
        )?;
    }
    if save_combat && !wrote_playerstate && !combat_died {
        println!(
            "no combat fixture: the run ended on step {} of {} after {secs}s; raise --probe-secs",
            combat.idx + 1,
            combat.steps.len()
        );
    }
    if save_playerstate && !wrote_playerstate {
        let pm_type = client
            .snapshots()
            .newest()
            .map(|s| s.ps.field_i32(client.protocol(), "pm_type"));
        println!(
            "no playerstate fixture: the join never completed (menus answered: {:?}, pm_type {pm_type:?}); \
             a spectator pm_type means the weapon answer was refused",
            join.answered
        );
    }
    finish_probe(
        &mut client,
        save_fixture,
        save_snapshots,
        tag.as_deref(),
        overwrite,
    )
}

/// The probe's connection, which says `disconnect` however the run ends:
/// an error out of the loop would otherwise leave a ghost player on the
/// server until its timeout, and the next run would not be alone there.
struct Connected(NetClient<UdpTransport>);

impl std::ops::Deref for Connected {
    type Target = NetClient<UdpTransport>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Connected {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Connected {
    fn drop(&mut self) {
        self.0.disconnect();
    }
}

/// `pmove_t`'s first entry, the state a spawned player is in; a client still
/// on the menus sits at 4, the spectator's.
const PM_NORMAL: i32 = 0;

/// `buttons` bit 0, fire (docs/protocol-1.1.md, "Usercmd input bits").
const BUTTON_ATTACK: u8 = 0x01;

/// `buttons` bit 6, use (docs/protocol-1.1.md, "Usercmd input bits"): the
/// press the stock gametypes take as "respawn me".
const BUTTON_USE: u8 = 0x40;

/// The eye a shot is aimed at, and traced from, when the playerstate carries
/// no view height: retail's standing value.
const EYE_HEIGHT: f32 = 60.0;

/// ANGLE2SHORT: the wire's 16 bits over a full turn.
fn deg_to_short(deg: f32) -> i32 {
    (deg * 65536.0 / 360.0).round() as i32
}

/// The view angles that point from `eye` at `target`, as signed wire words.
/// Pitch is positive-down on the wire, which is why the sign flips.
fn aim_at(eye: [f32; 3], target: [f32; 3]) -> (i32, i32) {
    let d = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
    let horiz = (d[0] * d[0] + d[1] * d[1]).sqrt();
    (
        deg_to_short(d[1].atan2(d[0]).to_degrees()),
        deg_to_short(-d[2].atan2(horiz).to_degrees()),
    )
}

/// What [`hold_view_yaw`] keeps: the facing the spawn gave, once known, and
/// until then the last spectating snapshot's `delta_angles[1]` and the yaw
/// the probe asked for under it.
#[derive(Default)]
struct ViewHold {
    spawn: Option<i32>,
    before: Option<(i32, i32)>,
}

/// A scripted yaw is a view direction, an offset from where the spawn faced:
/// every cmd after the spawn asks for that facing plus the script's yaw.
/// `send_frame` takes `delta_angles` off, so a push of `delta_angles` (the
/// prone cone's hold) never turns the heading the script asked for.
///
/// The facing is not read off the first spawned snapshot's view: by then
/// the probe's own cmds have turned it (on 1.1 the spawn's think runs a cmd
/// whose angles are the negated `delta_angles`, so that view reads 0). It is
/// read off `delta_angles`: `SetClientViewAngle` sets them to the spawn's
/// yaw less the last cmd's angle, and that cmd was the asked-for yaw less the
/// spectating `delta_angles` (cod15-movement.md, "The spawn's lie-down";
/// cod11-combat.md 9.2). Taking `delta_angles` off as well would turn the
/// view against every push, and a prone swing past the cap would oscillate
/// (playerstate_motion_ab.rs); latching the first spawned view would face
/// every capture at world yaw 0.
fn hold_view_yaw(
    cmd: &mut net::msg::UserCmd,
    newest: Option<&net::snapshot::Snapshot>,
    hold: &mut ViewHold,
) {
    let Some(s) = newest else {
        return;
    };
    let p = wire();
    let delta = s.ps.field_i32(p, "delta_angles[1]");
    if hold.spawn.is_none() {
        if s.ps.field_i32(p, "pm_type") == PM_NORMAL {
            let (before, asked) = hold.before.unwrap_or((0, 0));
            hold.spawn = Some((delta - before + asked) & 0xffff);
        } else {
            hold.before = Some((delta, cmd.angles[1]));
        }
    }
    if let Some(spawn) = hold.spawn {
        cmd.angles[1] = (spawn + cmd.angles[1]) & 0xffff;
    }
}

fn horiz_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Control bytes as `\xNN`, so a `GAME_PICKUP_AMMO\x14WEAPON_FG42` token
/// survives a text fixture. The gate escapes ours the same way.
fn escape_ctl(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() {
                format!("\\x{:02x}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::bump::{bump_axis_far, bump_stop_distance};
    use super::combat::{
        CombatStep, MissileWatch, Pulse, StallTurn, combat_script, data_secs, reloading,
    };
    use super::fixture::write_capture_in;
    use super::hit::{
        APPROACH_LIMIT, FINISH_TAPS, GRENADE_THROWS, GRENADE_WATCH_HOLD, HitPhase, PhaseCtx,
        SINGLE_SHOT_HOLD, THROW_HOLD, keep_sample,
    };
    use super::join::SPAWN_SETTLE;
    use super::ladder::{
        LADDER_CLIMB_SCRIPT, LADDER_GRAB_SCRIPT, LADDER_SCRIPT, LADDER_TURN_SCRIPT,
    };
    use super::mapchange::mapchange_text;
    use super::pickup::{PICKUP_FG42_XY, PICKUP_SETTLE, PickupPhase};
    use super::pvs::{PvsStation, merge_entity_samples, parse_fixture_samples};
    use super::scripts::ProneStep;
    use super::turret::{
        StrafeStop, TurretGun, TurretPhase, turret_bearing, turret_from_entities,
        turret_strafe_stop, turret_sweep_offset, turret_trace_line,
    };
    use super::*;

    /// A run's last partial block of cmds is printed too: 61 cmds make two
    /// `CMDS` lines (BUG-091).
    #[test]
    fn the_last_partial_block_of_cmds_is_kept() {
        let ring = vcod_common::net::snapshot::SnapshotRing::default();
        let mut fall = FallProbe::default();
        for i in 0..61 {
            fall.record_cmd(1000 + 8 * i, None, (0, 0, 0), i as u32, &ring);
        }
        let line = fall.take_line().expect("the 61st cmd's line");
        assert!(line.starts_with("CMDS st=1480 d= "), "{line}");
        assert_eq!(fall.take_line(), None);
    }

    /// Each `--save-ladder` script is told apart by its value, not by a
    /// const slice's address, and names itself in the fixture (BUG-092).
    #[test]
    fn each_ladder_script_runs_and_names_its_own_phases() {
        for (s, name, phases) in [
            (LadderScript::Full, "full", LADDER_SCRIPT),
            (LadderScript::Climb, "climb", LADDER_CLIMB_SCRIPT),
            (LadderScript::Turn, "turn", LADDER_TURN_SCRIPT),
            (LadderScript::Grab, "grab", LADDER_GRAB_SCRIPT),
        ] {
            assert_eq!(s.name(), name);
            assert_eq!(s.phases().len(), phases.len(), "{name}");
            assert_eq!(s.phases()[0].0, phases[0].0, "{name}");
        }
    }

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

    /// `--vanilla` holds across the download reopen: a pak an earlier run
    /// left in the scratch directory stays off the search path `cp` reads.
    #[test]
    fn a_vanilla_download_reopen_keeps_to_the_stock_paks() {
        use std::io::Write;
        let pk3 = |dir: &std::path::Path, file: &str| {
            std::fs::create_dir_all(dir).unwrap();
            let mut z = zip::ZipWriter::new(std::fs::File::create(dir.join(file)).unwrap());
            z.start_file("readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(file.as_bytes()).unwrap();
            z.finish().unwrap();
        };
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install/main");
        let scratch = root.path().join("scratch");
        pk3(&install, "pak0.pk3");
        pk3(&scratch.join("main"), "mp_custom.pk3");
        for vanilla in [false, true] {
            let fs = vcod_common::pk3::Pk3Fs::open_search(&install, None, None, vanilla).unwrap();
            let mut dl = DownloadProbe::new(scratch.clone(), Some(&fs));
            dl.scratch_dirs = vec![scratch.join("main")];
            dl.reopen();
            let names: Vec<String> = dl
                .fs()
                .unwrap()
                .paks()
                .map(vcod_common::pk3::PakInfo::qualified_name)
                .collect();
            assert_eq!(
                names.contains(&"main/mp_custom".to_string()),
                !vanilla,
                "{names:?}"
            );
            assert_eq!(dl.fs().unwrap().vanilla(), vanilla);
        }
    }

    /// The stock rifle is semi-automatic: a held fire bit is one shot and then
    /// nothing, so every shot needs its own release edge. Counted at the
    /// probe's send rate rather than read off the pattern by eye.
    /// A map change and a dm map restart both rerun retail's `ClientConnect`
    /// and offer the team menu again, under the indices the last one used.
    /// Without the re-arm the probe skips them and never spawns again; a join
    /// left "settled" across a gamestate would have the capture reading the
    /// new map's menu as a spawn.
    #[test]
    fn a_restart_and_a_gamestate_reopen_the_menus() {
        let t0 = Instant::now();
        let mut join = JoinProbe::new(Some("allies"), None);
        join.main_menu = "weapon_american".to_string();
        join.answered = vec![0, 1];
        join.weapon = "m1carbine_mp".to_string();
        join.answered_weapon = Some(t0);
        assert!(join.settled(t0 + SPAWN_SETTLE));

        // A map restart reopens the menu without unspawning the run: the sd
        // pair crosses four of them and its script must not pause.
        join.reopen_menus();
        assert!(join.answered.is_empty());
        assert!(join.main_menu.is_empty());
        assert!(join.settled(t0 + SPAWN_SETTLE));

        join.answered = vec![0, 1];
        join.answered_weapon = Some(t0);
        join.rearm();
        assert!(join.answered.is_empty());
        assert!(!join.settled(t0 + SPAWN_SETTLE));
        // The team is the run's, not the map's: it survives.
        assert_eq!(join.team, "allies");
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

    /// The aim is what decides whether the shooter's shots reach the target at
    /// all, and its two sign conventions (yaw counter-clockwise from +x, pitch
    /// positive-down) are the two things easiest to get backwards.
    #[test]
    fn hit_aim_points_at_the_target() {
        let eye = [0.0, 0.0, 60.0];
        assert_eq!(aim_at(eye, [100.0, 0.0, 60.0]), (0, 0));
        assert_eq!(aim_at(eye, [0.0, 100.0, 60.0]).0, 16384);
        assert_eq!(aim_at(eye, [-100.0, 0.0, 60.0]).0.abs(), 32768);
        let (_, up) = aim_at(eye, [100.0, 0.0, 160.0]);
        assert!(
            up < 0,
            "a target above the eye needs a negative pitch, got {up}"
        );
        let (_, down) = aim_at(eye, [100.0, 0.0, 0.0]);
        assert!(
            down > 0,
            "a target below the eye needs a positive pitch, got {down}"
        );
    }

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

    /// A fixture-writing mode off 1.1 needs a tag; watching does not.
    #[test]
    fn another_protocol_needs_a_capture_tag_to_write() {
        use net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};
        assert!(check_capture_tag(&PROTOCOL_V1, true, None).is_ok());
        assert!(check_capture_tag(&PROTOCOL_V6, true, None).is_err());
        assert!(check_capture_tag(&PROTOCOL_V22, true, None).is_err());
        assert!(check_capture_tag(&PROTOCOL_V22, true, Some("uo")).is_ok());
        assert!(check_capture_tag(&PROTOCOL_V22, false, None).is_ok());
    }

    /// A server's mapname or g_gametype names the fixture file; one that
    /// could leave the fixture directory ends the run.
    #[test]
    fn a_hostile_map_name_cannot_name_a_fixture() {
        let info = |map: &str, gt: &str| format!("\\mapname\\{map}\\g_gametype\\{gt}");
        assert!(check_fixture_names(true, &info("mp_carentan", "dm")).is_ok());
        for (map, gt) in [
            ("../x", "dm"),
            ("mp_x", "../../y"),
            ("a/b", "dm"),
            ("", "dm"),
        ] {
            assert!(
                check_fixture_names(true, &info(map, gt)).is_err(),
                "{map} {gt}"
            );
        }
        assert!(check_fixture_names(false, &info("../x", "dm")).is_ok());

        assert_eq!(
            std::path::PathBuf::from(
                fixture_path("fixtures/items", "mp_carentan-dm-pickup.txt").unwrap()
            ),
            std::path::Path::new("fixtures/items/mp_carentan-dm-pickup.txt")
        );
        for name in ["../x-dm-pickup.txt", "a/b-dm.txt", "..-dm.txt"] {
            assert!(fixture_path("fixtures/items", name).is_err(), "{name}");
        }
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

    /// A snapshot with the given playerstate fields and item entities
    /// (`(num, index, xyz)`), for driving [`PickupProbe::step`].
    fn pickup_snap(
        num: u32,
        ps_fields: &[(&str, i32)],
        items: &[(u32, i32, [f32; 3])],
    ) -> net::snapshot::Snapshot {
        use net::msg::{EntityState, PlayerState};
        let p = &net::protocol::PROTOCOL_V1;
        let mut ps = PlayerState::null(p);
        for &(n, v) in ps_fields {
            ps.fields[PlayerState::field_index(p, n).unwrap()] = v;
        }
        let entities = items
            .iter()
            .map(|&(n, index, at)| {
                let mut e = EntityState::null(p);
                e.number = n;
                let mut set =
                    |f: &str, v: i32| e.fields[EntityState::field_index(p, f).unwrap()] = v;
                set("eType", crate::entities::ET_ITEM);
                set("index", index);
                for (i, c) in at.into_iter().enumerate() {
                    set(&format!("pos.trBase[{i}]"), c.to_bits() as i32);
                }
                (n, e)
            })
            .collect();
        net::snapshot::Snapshot {
            server_time: 1000 + 50 * num as i32,
            message_num: num,
            delta_num: -1,
            snap_flags: 0,
            ps,
            entities,
            clients: BTreeMap::new(),
            valid: true,
        }
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

    /// The spawn's facing is `delta_angles` at the spawn, less the last
    /// spectating one, plus the yaw asked for under it; every cmd after
    /// adds the script's yaw to it, and a later push of `delta_angles`
    /// does not move it.
    #[test]
    fn a_held_view_yaw_latches_the_spawns_facing_off_delta_angles() {
        let snap = |num, pm_type, delta| {
            pickup_snap(
                num,
                &[("pm_type", pm_type), ("delta_angles[1]", delta)],
                &[],
            )
        };
        let asked = |yaw| net::msg::UserCmd {
            angles: [0, yaw, 0],
            ..net::msg::NULL_USERCMD
        };
        let mut hold = ViewHold::default();
        let mut cmd = asked(500);
        hold_view_yaw(&mut cmd, None, &mut hold);
        assert_eq!(cmd.angles[1], 500, "no snapshot yet");
        let spectating = snap(1, 4, 60000);
        let mut cmd = asked(500);
        hold_view_yaw(&mut cmd, Some(&spectating), &mut hold);
        assert_eq!(cmd.angles[1], 500, "spectating, unheld");
        // SetClientViewAngle: delta = spawn yaw - (asked - spectating delta)
        let spawn_yaw = 8192;
        let delta = (spawn_yaw - (500 - 60000)) & 0xffff;
        let spawned = snap(2, PM_NORMAL, delta);
        let mut cmd = asked(100);
        hold_view_yaw(&mut cmd, Some(&spawned), &mut hold);
        assert_eq!(hold.spawn, Some(spawn_yaw));
        assert_eq!(cmd.angles[1], spawn_yaw + 100);
        let pushed = snap(3, PM_NORMAL, delta + 3000);
        let mut cmd = asked(100);
        hold_view_yaw(&mut cmd, Some(&pushed), &mut hold);
        assert_eq!(
            cmd.angles[1],
            spawn_yaw + 100,
            "a push does not turn the heading"
        );
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

    /// The commands of one batch end at the client's reliable sequence;
    /// the fixture interleaves every kind by ms, and at one ms puts the
    /// gamestate first, then the commands, the out-of-band text and the
    /// trace, each kind in its own order.
    #[test]
    fn a_mapchange_capture_numbers_its_commands_and_orders_lines_by_ms_then_kind() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut m = MapChangeProbe::default();
        m.elapsed_ms(ms(0));
        m.on_commands(ms(0), 10, &["a".into(), "b".into(), "c".into()]);
        let restart = wire().map_restart_command;
        m.on_commands(ms(500), 11, &[restart.into()]);
        m.on_oob(ms(500), "disconnect");
        m.sample(ms(500), &pickup_snap(1, &[], &[]));
        m.gamestates.push((500, 7, 3, 12, "mp_brecourt".into()));
        m.on_commands(ms(500), 12, &["e".into()]);
        let seqs: Vec<i32> = m.commands.iter().map(|c| c.1).collect();
        assert_eq!(seqs, [8, 9, 10, 11, 12]);
        let join = JoinProbe::new(Some("allies"), None);
        let text = mapchange_text("mapchange", "dm", &join, &m, "mp_carentan");
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with('!') && !l.starts_with("!observed"))
            .map(|l| l.split(' ').take(3).collect::<Vec<_>>())
            .map(|w| w[0])
            .collect();
        assert_eq!(
            lines,
            [
                "!cmd",
                "!cmd",
                "!cmd",
                "!gamestate",
                "!cmd",
                "!cmd",
                "!oob",
                "!trace"
            ]
        );
        assert!(text.contains(&format!(
            "!cmd ms=500 seq=11 text={restart}\n!cmd ms=500 seq=12 text=e\n"
        )));
        assert!(text.contains("# BROKEN only one gamestate"));
        let text = mapchange_text("roundrestart-target", "sd", &join, &m, "mp_carentan");
        assert!(!text.contains("# BROKEN"), "the wire's restart command");
    }

    fn station(label: &'static str, ents: &[u32]) -> PvsStation {
        let p = wire();
        PvsStation {
            label,
            origin: [1.0, 2.0, 3.0],
            server_time: 1000,
            ents: ents
                .iter()
                .map(|&n| {
                    let mut e = net::msg::EntityState::null(p);
                    e.number = n;
                    e.fields[net::msg::EntityState::field_index(p, "eType").unwrap()] = 2;
                    (n, e)
                })
                .collect(),
        }
    }

    /// The entities fixture's rewrite: a second run's samples join the
    /// first's past its header, at most three of one entity set, numbered
    /// afresh, the first run's bodies byte for byte.
    #[test]
    fn an_entities_rewrite_keeps_three_samples_a_set_and_the_old_bodies() {
        let (first, taken, kept, sets) = merge_entity_samples(
            "",
            &[
                station("a", &[1, 2]),
                station("b", &[1, 2]),
                station("c", &[1]),
            ],
        );
        assert_eq!((taken, kept, sets), (0, 3, 2));
        let file = format!("# a header line\n{first}");
        let read = parse_fixture_samples(&file);
        let ents: Vec<&[u32]> = read.iter().map(|s| s.ents.as_slice()).collect();
        assert_eq!(ents, [&[1, 2][..], &[1, 2], &[1]]);
        assert!(
            read[0]
                .body
                .starts_with("serverTime 1000\norigin[0] 1065353216\n")
        );
        assert!(read[0].body.contains("[ent 2]\neType 2\n"));
        let (second, taken, kept, sets) = merge_entity_samples(
            &file,
            &[
                station("d", &[1, 2]),
                station("e", &[1, 2]),
                station("f", &[3]),
            ],
        );
        assert_eq!((taken, kept, sets), (3, 5, 3));
        let labels: Vec<String> = parse_fixture_samples(&second)
            .iter()
            .map(|s| s.label.clone())
            .collect();
        assert_eq!(
            labels,
            ["a", "b", "c", "d", "f"],
            "e is a fourth of its set"
        );
        assert!(
            second.starts_with(&first),
            "the first run's samples as they were"
        );
        assert!(second.contains("[sample 3] d\n") && second.contains("[sample 4] f\n"));
        assert!(!second.contains("header"));
    }

    fn prone_step(at_ms: u64, forward: i8, yaw: f32, pitch: f32) -> ProneStep {
        ProneStep {
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
        let mut p = ProneScript::new(
            30.0,
            vec![
                prone_step(0, 0, 0.0, 0.0),
                prone_step(1000, 127, 90.0, -20.0),
                prone_step(2000, 0, 180.0, 10.0),
            ],
        );
        assert_eq!(p.cmd(ms(0)).angles, [0; 3], "not started");
        p.started = Some(t0);
        let at = |p: &ProneScript, n| {
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

    /// `FallProbe`'s `CMDS` line: the first `serverTime` and each step,
    /// the yaws, the inputs and snapshots by the cmd they came at, every
    /// snapshot's `commandTime` since the last cmd, and the sequences
    /// with their wrap, as the fall and turret gates read them.
    #[test]
    fn a_fall_cmds_line_carries_its_cmds_by_index() {
        let mut ring = vcod_common::net::snapshot::SnapshotRing::default();
        let snap = |num, t: i32, ct| {
            let mut s = pickup_snap(num, &[("commandTime", ct)], &[]);
            s.server_time = t;
            s
        };
        let mut fall = FallProbe::default();
        let seq0 = u32::MAX - 1;
        for k in 0..4u32 {
            if k == 1 {
                ring.insert(snap(1, 1050, 1040));
            }
            if k == 3 {
                ring.insert(snap(2, 1100, 1090));
                ring.insert(snap(3, 1150, 1140));
            }
            let input = if k < 2 { (1, 0, 0) } else { (0, 4, 127) };
            let yaw = (k == 0).then_some(16384);
            fall.record_cmd(
                1000 + 16 * k as i32,
                yaw,
                input,
                seq0.wrapping_add(k),
                &ring,
            );
        }
        let line = fall.take_line().unwrap();
        let (head, tail) = line.split_once(" w=").unwrap();
        assert_eq!(
            head,
            "CMDS st=1000 d=16,16,16 yaw=16384 in=0:1:0:0,2:0:4:127 snap=1:1050,3:1150 \
             ct=1050:1040,1100:1090,1150:1140"
        );
        assert_eq!(tail.split_once(" q=").unwrap().1, "4294967294,1,1,1");
    }

    /// The bump walk presses toward a far mark until friction and the
    /// velocity's lag would coast it inside 8 units, and never presses
    /// while it moves away faster than 5 u/s.
    #[test]
    fn the_bump_walk_presses_until_its_coast_reaches_the_mark() {
        // PM_Friction from 300 u/s: (300 - 100) / 5.5 + 100^2 / 1100
        assert!((bump_stop_distance(300.0) - 45.4545).abs() < 1e-3);
        assert!((bump_stop_distance(50.0) - 2.2727).abs() < 1e-3);
        // 300 u/s toward coasts 45.45 + 30 of lag
        assert_eq!(bump_axis_far(90.0, 300.0), 127);
        assert_eq!(bump_axis_far(83.0, 300.0), 0);
        assert_eq!(bump_axis_far(-90.0, -300.0), -127);
        assert_eq!(bump_axis_far(9.0, 0.0), 127);
        assert_eq!(bump_axis_far(8.0, 0.0), 0);
        assert_eq!(bump_axis_far(100.0, -6.0), 0, "moving away");
        assert_eq!(bump_axis_far(100.0, -4.0), 127, "drifting");
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

    /// A fixture name already on disk, tagged or not, is refused without
    /// `--overwrite-fixture`, and the refused capture is kept under `tmp/`.
    #[test]
    fn a_write_refuses_to_replace_a_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mp_carentan-tdm-ads-sniper.txt");
        let path = path.to_str().unwrap();

        let keep = dir.path().join("tmp");
        let write = |out, overwrite| write_capture_in(path, out, overwrite, &keep);

        write("retail", false).expect("a name nothing holds yet");
        let err = write("ours", false).expect_err("the second write lands on the first");
        assert!(err.to_string().contains("--overwrite-fixture"), "{err}");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "retail");
        let kept = keep.join("mp_carentan-tdm-ads-sniper.txt");
        assert_eq!(std::fs::read_to_string(kept).unwrap(), "ours");

        write("ours", true).expect("the flag says to");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "ours");
    }
}
