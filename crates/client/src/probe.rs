//! `--net-probe`, the headless spectate client. Lives in the client crate
//! because cue resolution needs the audio alias tables.

use net::flags::{ET_TURRET, PM_DEAD, PM_NORMAL};
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
use fixture::{
    MOVERS_FIXTURE_DIR, finish_probe, write_configstrings_fixture, write_playerstate_fixture,
    write_scripted_fixture,
};
pub(crate) use fixture::{cmd_fields, tagged_name, write_fixture_file};
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
use scripts::{
    InputScript, UOFOOT_SWITCHES, autofire_script, crouchjump_script, moves_script,
    moveshot_script, patchwalk_script, prone_script, pronemove_script, sprint_script, talk_script,
    turns_script, uofoot_script, uoturret_script,
};
pub use scripts::{MoveShot, ProneMove};
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

/// Every `--net-probe` flag, flattened into the client's `Args` under its own
/// help heading. The group needs `--net-probe`: a probe flag given alone is an
/// error rather than ignored. [`probe`] takes the whole struct.
#[derive(clap::Args, Clone, Debug)]
#[command(next_help_heading = "Probe flags (all need --net-probe)")]
#[group(id = "probe_args", requires = "net_probe")]
pub struct ProbeArgs {
    /// Overwrite the committed gamestate.bin fixture with the --net-probe capture.
    /// Off by default: the parser tests pin that file, and a capture from another
    /// map or a mid-round server is not a drop-in. Otherwise the probe writes to tmp/.
    #[arg(long)]
    save_fixture: bool,
    /// Overwrite the committed snapshots.bin fixture only; gamestate.bin stays pinned.
    #[arg(long)]
    save_snapshots: bool,
    /// Run `record` from the first snapshot into tmp/probe-<tag>.dm_<n> and
    /// write every server message past the gamestate (not only the snapshot
    /// ones) as the `--capture-tag` pair in crates/common/tests/fixtures/net/,
    /// what `net::tests::a_retail_framed_demo_plays_and_vcod_writes_it_alike`
    /// frames as retail does (cod11-demos.md 10). Needs `--capture-tag`.
    #[arg(long)]
    save_demo: bool,
    /// Write the retail server's non-empty configstring table to
    /// crates/server/tests/fixtures/configstrings/<map>-<gametype>.txt, the
    /// fixture crates/server/tests/configstrings_ab.rs diffs against. Separate
    /// from --save-fixture because that pair pins the parser's byte-exact
    /// captures and this one pins the game module's table.
    #[arg(long)]
    save_configstrings: bool,
    /// Join a team through the stock menu and write the retail playerstate to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>.txt, the
    /// fixture crates/server/tests/playerstate_ab.rs diffs against.
    #[arg(long)]
    save_playerstate: bool,
    /// Join a team, then hold each movement input in turn and write the
    /// retail playerstate settled under each to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-motion.txt.
    /// Separate from --save-playerstate because that one pins a standing
    /// player, which leaves every field the client predicts at zero.
    #[arg(long)]
    save_motion: bool,
    /// Join a team, then run a scripted weapon sequence (single shot,
    /// sustained fire, reload, fire crouched and prone) and write the retail
    /// playerstate to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-combat.txt.
    /// Separate from --save-motion because that one samples a settled state
    /// and a shot is a transient: this fixture carries every snapshot, since
    /// the playerstate event ring holds four slots and overwrites as it fires.
    /// Each firing step waits for weaponstate to read ready before it taps, and
    /// the reload holds the weapon file's reloadTime, so a weapon still busy
    /// from the step before cannot be recorded under a firing label. Each step
    /// also logs every cmd it sent (`!cmds`, its view and movement included)
    /// and each snapshot its
    /// commandTime, which `playerstate_combat_ab` replays frame for frame. The
    /// sequence takes about 40 s, inside the default --probe-secs.
    #[arg(long)]
    save_combat: bool,
    /// Join a team, then run the sight script (sight held, released, held
    /// through a reload, through a shot and while walking, plus a hip shot
    /// and a walk to kick the spread counter) and write every snapshot's
    /// fWeaponPosFrac and aimSpreadScale to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-ads.txt.
    /// The same machine as --save-combat with another script; the two are
    /// separate fixtures because the ramp is a transient the combat steps
    /// never hold still for.
    #[arg(long)]
    save_ads: bool,
    /// Join a team, then run the grenade script (one melee swing on the
    /// rifle, a switch to the frag, a cooked throw, a cook held past the pin,
    /// a cook cancelled by a weapon switch and a throw at the ground) and
    /// write every snapshot's grenadeTimeLeft and weaponDelay, plus a !missile
    /// line per snapshot that had one on the wire, to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-grenade.txt.
    /// The same machine as --save-ads with another script; it stands still, so
    /// a gate replays it from the origin the header carries.
    #[arg(long)]
    save_grenade: bool,
    /// Join a team, raise the sight and tap eight shots down it standing
    /// still, printing every bullet-impact temp entity's origin beside the
    /// shooter's eye and view, which is what measures the sight sway
    /// (docs/research/cod11-combat.md, section 15). A measurement, not a
    /// fixture: the same machine as --save-ads, and it writes nothing.
    /// Use it with --probe-weapon kar98k_sniper_mp: adsSpread 0 makes the
    /// scoped shot deterministic.
    #[arg(long)]
    probe_sway: bool,
    /// Join a team, raise the sight, turn to a wall a few hundred units off
    /// and tap scoped shots level, leaning left, leaning right and leaning
    /// crouched, recording every snapshot's leanf and every bullet-impact
    /// temp entity beside the playerstate it was fired from. Writes
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-lean.txt,
    /// which lean_ab.rs replays. Use it with --probe-weapon
    /// kar98k_sniper_mp: adsSpread 0 leaves only the sway on the shot, so the
    /// impact puts the leaned muzzle on record (combat doc 2.1, 15).
    #[arg(long)]
    save_lean: bool,
    /// The shooter half of the hit capture: join a team, walk toward the other
    /// player until the eye-to-eye trace through the map's collision is clear,
    /// then fire a single shot, a burst and until the target dies, and watch
    /// the corpse and the respawn. Writes
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-hit-shooter.txt.
    /// Run it against a server that already has a --probe-target on it, on the
    /// other team. A run that never finds a line of sight says so in the
    /// fixture header and still records the shots, which then hit the map.
    #[arg(long)]
    save_hit: bool,
    /// The target half of the hit capture: join a team, stand still, kill
    /// itself 10 s in, press use 3 s after each death and record every
    /// snapshot that moves a damage or death field, plus the obituaries, the
    /// corpses and the scoreboards. Writes
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-hit-target.txt.
    /// Separate from --save-hit because the two roles are two probes: this one
    /// is the victim, and its `kill` is what makes the death, the corpse and
    /// the respawn land whether or not the shooter ever gets a shot off.
    #[arg(long)]
    probe_target: bool,
    /// With --probe-target: each `kill` goes out with exactly N usercmds
    /// behind it, one message each, sent back to back so they reach the
    /// server inside one frame, and then no cmd for 150 ms, so the next one
    /// lands past the frame's end. The list is cycled kill by kill (`1,3`),
    /// a kill every 8 s. Measures how many of a death frame's cmds run
    /// before `ClientEndFrame` makes the player dead (combat doc 9.2).
    /// Writes the target fixture with a `!kill` line per kill; tag it
    /// `killcmds`, `killcmds-1.5` or `killcmds-uo`.
    #[arg(long, requires = "probe_target", value_delimiter = ',')]
    probe_kill_cmds: Vec<usize>,
    /// Join a team, stand still and record what the wire does across a map
    /// end and the rotation that follows: every gamestate with its serverId,
    /// every serverCommand with its reliable sequence, every out-of-band
    /// packet (retail announces a map change with `loadingnewmap` and sends no
    /// gamestate with it) and one trace line per snapshot that moved. Writes
    /// crates/server/tests/fixtures/netchan/<map>-<gametype>-mapchange.txt,
    /// named for the map the run started on. Give it --probe-secs enough to
    /// span the limit, the intermission and the next map's load. It sends
    /// `score` every 2 s, since retail answers the `b` scoreboard and never
    /// pushes one: every `b` in that fixture is an answer to this probe.
    #[arg(long)]
    save_mapchange: bool,
    /// The same recording across a round restart instead of a map change, as
    /// a pair: with --probe-target it is the half that kills itself 20 s in
    /// and ends the round, without it the half that walks up and only watches.
    /// Writes <map>-<gametype>-roundrestart-target.txt and
    /// -roundrestart-shooter.txt. A restart re-sends no gamestate, so what the
    /// shooter's weapon column shows either side of it is the pers[] carry the
    /// stock sd.gsc does by hand. Start the target first.
    #[arg(long)]
    save_roundrestart: bool,
    /// Join a team, walk the --probe-pvs route and write the entity trace to
    /// crates/server/tests/fixtures/entities/<map>-<gametype>.txt, the fixture
    /// crates/server/tests/entities_ab.rs diffs against. Separate from
    /// --probe-pvs because that one only prints: this one overwrites committed
    /// evidence. Each run appends its stations to the map's fixture, since one
    /// walk gets one random spawn; delete the file to start that map over.
    #[arg(long)]
    save_entities: bool,
    /// Join a team, then walk a route and print the snapshot entity list at
    /// each station: which entities the server sent, and every add and removal
    /// along the way with the position it happened at. Answers whether the
    /// server culls the entity list by where the client stands. Writes no
    /// fixture; the route is relative to the spawn, so its stations are
    /// positions in one run, not reproducible places.
    #[arg(long)]
    probe_pvs: bool,
    /// Walk the --probe-pvs route with the sight held and count, per second
    /// and for the run, what a slope does to the playerstate: snapshots off
    /// the ground, fWeaponPosFrac reversals, EV_STEP_VIEW (143) events with
    /// their parms and the mean speed. Writes no fixture; the same run
    /// against retail and against ours is the comparison. Pair it with
    /// --probe-cmd-ms 8 to send usercmds at a high-fps client's rate.
    #[arg(long)]
    probe_slope: bool,
    /// Join a team and walk at the map's trigger brushes, read out of the BSP,
    /// nearest unvisited first, steering round geometry with a look-ahead
    /// trace against the map's own collision. It presses use after every death
    /// so a minefield does not end the run, and kills itself when it wedges.
    /// The client writes no fixture: the evidence is the *server's*
    /// games_mp.log, logged by the client-probes/probe_trigger.gsc gametype,
    /// which is what crates/server/tests/triggers_ab.rs replays. Give it a
    /// long --probe-secs; most of a stock map's belt is walled off and each
    /// unreachable brush costs a leg.
    #[arg(long)]
    probe_triggers: bool,
    /// The S&D plant capture: joins the attackers, walks into bombzone_A,
    /// holds use for 2 s and releases (the abort), then holds use through a
    /// full plant with a forward walk sent 2 s in (what a link does to
    /// pmove), and stands still after. Writes
    /// crates/server/tests/fixtures/playerstate/<map>-sd-plant-attacker.txt.
    /// Pair with --probe-defuse on the other team, and run
    /// client-probes/probe_lookat as the gametype so the server's log carries
    /// the lookat fires. Retail evidence when taken against tools/run_probe.sh;
    /// a run against vcod-server overwrites it.
    #[arg(long, conflicts_with = "probe_defuse")]
    probe_plant: bool,
    /// The S&D defuse capture: joins the defenders, waits for the plant,
    /// walks to the bomb, sweeps the view across it with the aim alone (the
    /// lookat trigger's shape; a held use would let bomb_think finish the
    /// defuse mid-sweep), then aims true and holds use through the defuse. Writes <map>-sd-defuse-defender.txt.
    #[arg(long, conflicts_with = "probe_plant")]
    probe_defuse: bool,
    /// The item pickup capture: joins allies, waits for
    /// client-probes/probe_pickup's teleport onto the first fg42, stands on
    /// it, aims at it and takes it with the use key, takes the second one's
    /// ammo by touch after the probe's second teleport, then swaps the
    /// carbine for a panzerfaust and back, logging every cmd and snapshot
    /// from the spawn on. Writes
    /// crates/server/tests/fixtures/items/<map>-dm-pickup.txt.
    /// Retail evidence when taken against tools/run_probe.sh; a run against
    /// vcod-server overwrites it.
    #[arg(long)]
    save_pickup: bool,
    /// With `--net-probe`: mount mp_carentan's MG at (1712 1830 8), sweep it
    /// past both arcs, fire it, dismount, remount from a crouch, and try a
    /// mount from outside the arc; writes
    /// `crates/server/tests/fixtures/turret/<map>-dm-turret.txt`. Needs
    /// `client-probes/probe_turret` as the gametype with `probe_teleport 1`.
    #[arg(long)]
    save_turret: bool,
    /// With `--save-turret`: hold the trigger at the axis client this many ms
    /// instead of 1000. `client-probes/probe_turret`'s `probe_pose 1` mode
    /// wants several seconds of rounds; pair it with a --capture-tag.
    #[arg(long, requires = "save_turret")]
    probe_turret_target_ms: Option<u64>,
    /// With `--net-probe`: the player-clip walker. Waits for
    /// `client-probes/probe_bump`'s placement 200 units behind the
    /// `--probe-bump-target` client on mp_carentan, then walks into it
    /// head-on, at a 20-unit glance and in a jump once per stance it takes;
    /// writes `crates/server/tests/fixtures/playerstate/<map>-dm-bump-walker.txt`.
    /// A `--capture-tag` starting `overlap` runs the overlap script instead
    /// (stand, then walk through the gsc's `probe_overlap 1` setorigins) and
    /// writes `<map>-dm-bump-<tag>-walker.txt`. Retail evidence when taken
    /// against tools/run_probe.sh; a run against vcod-server overwrites it.
    #[arg(long, conflicts_with = "probe_bump_target")]
    save_bump: bool,
    /// With `--net-probe` and `--probe-team allies`: the walk behind the
    /// moving-body pose capture. Waits for `client-probes/probe_legs` to put
    /// the player on its mark on mp_carentan, then runs, stops, backs,
    /// strafes, runs diagonally and crouch-walks at world yaw 0, writing
    /// every usercmd and snapshot to
    /// `crates/server/tests/fixtures/movers/<map>-dm-legs-client.txt`, which
    /// `legs_ab.rs` replays beside the gsc's bone log. Refuses to replace the
    /// fixture without --overwrite-fixture. A `--capture-tag` starting
    /// `torso` fires, reloads and aims on the move instead, beside
    /// `client-probes/probe_torso`, for `torso_ab.rs`; one starting `stopgo`
    /// stops and goes inside the legs blends, beside `probe_legs`.
    #[arg(long)]
    save_legs: bool,
    /// With `--net-probe` and `--probe-team`: the shellshock capture's
    /// client half. Waits for `client-probes/probe_shock` to put the player
    /// on mp_carentan's street, runs forward and back a second each way,
    /// holds `wbuttons` 0x4 while the playerstate's shellshock runs (as the
    /// retail cgame does) and writes every usercmd and snapshot to
    /// `crates/server/tests/fixtures/shellshock/<map>-dm-shock-client.txt`.
    /// Refuses to replace the fixture without --overwrite-fixture.
    #[arg(long)]
    save_shock: bool,
    /// With `--net-probe`: the player-clip target. Joins, stands where
    /// `client-probes/probe_bump` puts it and, once the walker stands on its
    /// mark, stands 35 s, crouches 25 s, stands 1.5 s, lies prone 25 s and
    /// stands. Writes no fixture; prints its playerstate while a push is on it.
    #[arg(long, conflicts_with = "save_bump")]
    probe_bump_target: bool,
    /// With `--net-probe`: stay a spectator and press the follow buttons
    /// (attack, attack, melee, the sight held 2 s, attack, 6 s apart from 8 s
    /// in), printing every snapshot whose clientNum, pm_type, pm_flags or
    /// eFlags moved. Writes no fixture.
    #[arg(long)]
    probe_follow: bool,
    /// With `--net-probe` and `--probe-team`: the killcam victim. Stands
    /// still, never sends `kill`, presses use 20 s after each death and once a
    /// second after that, and prints a `KILLCAM` line per snapshot from the
    /// death until 3 s after the respawn. Writes no fixture.
    #[arg(long)]
    probe_killcam: bool,
    /// With `--probe-killcam`: press use this many ms after the killcam starts
    /// (the first frame following another client) instead, to skip it.
    #[arg(long, requires = "probe_killcam")]
    probe_killcam_skip_ms: Option<u64>,
    /// With `--net-probe` and `--probe-team`: stand still and print a `FALL`
    /// line per snapshot whose ground entity, `pm_flags`, `pm_time`, events
    /// or health moved, a `CMDS` line of the `serverTime`s it sent every 60
    /// cmds, and a `SPAWNED` line at the first spawned snapshot (time,
    /// `commandTime`, origin, view, velocity); `client-probes/probe_fall`
    /// does the dropping. Writes no fixture.
    #[arg(long)]
    probe_fall: bool,
    /// With `--probe-fall`: hold forward on every cmd at this world yaw, so
    /// each landing's stun walks the player into whatever is in the way.
    /// The stun-slide capture walks 315 into the street's south wall.
    #[arg(long, value_name = "YAW", requires = "probe_fall")]
    probe_fall_walk: Option<f32>,
    /// With `--probe-fall`: hold prone on every cmd at this world yaw, the
    /// client half of `client-probes/probe_pronedrop` (the airborne prone
    /// refusal, docs/research/cod11-mantle.md, "Prone Blocked").
    #[arg(
        long,
        value_name = "YAW",
        requires = "probe_fall",
        conflicts_with = "probe_fall_walk"
    )]
    probe_fall_prone: Option<f32>,
    /// With `--net-probe`: hold this world yaw on every cmd no other mode
    /// steers, where the plain probe holds 0 (`delta_angles` taken off), so a
    /// script's turn holds. `client-probes/probe_dyingpose` stands its body
    /// at 90 with it.
    #[arg(long, value_name = "YAW")]
    probe_yaw: Option<f32>,
    /// With `--net-probe` and `--probe-team`: stand at world yaw 0 and hold
    /// this view pitch in alternate 400 ms windows (0 between), printing a
    /// `PITCH` line per snapshot whose pitch moved. The target half of
    /// `client-probes/probe_turret`'s `probe_pose 1` measurement. Writes no
    /// fixture.
    #[arg(long, value_name = "DEG")]
    probe_pitch_flip: Option<f32>,
    /// With `--net-probe` and `--probe-team`: stand still and print a `RIDE`
    /// line per snapshot with the origin, velocity, ground entity and view
    /// yaw, the mover push and ride capture's wire half, and a `RIDE_NEW`
    /// line per entity that enters the snapshot, and `--probe-fall`'s `CMDS`
    /// lines; `client-probes/probe_ride` and `probe_worldcopy` move a brush
    /// model under the player. Writes no fixture.
    #[arg(long)]
    probe_ride: bool,
    /// With `--net-probe` and `--probe-team`: stand still and print an `ITEM`
    /// line per snapshot an item entity appeared, changed or left, with its
    /// index, owner, ground, both trajectories and its event ring, and its
    /// cmds as `SPAWNED` and `CMDS` lines (`--probe-fall`'s);
    /// `client-probes/probe_itemdrop` drops, spawns and respawns the items.
    /// Writes no fixture.
    #[arg(long)]
    probe_items: bool,
    /// With `--net-probe` and `--probe-team`: stand still and print a
    /// `COMPASS` line per snapshot whose `iCompassFriendInfo`, eye or player
    /// list changed, the field decoded as the cgame reads it. Two on one team
    /// measure the out-of-view teammate the server packs there. On a United
    /// Offensive server the line adds `iCompassTankInfo` and the vehicles
    /// in the snapshot (`client-probes/probe_tankcompass`). Writes no
    /// fixture.
    #[arg(long)]
    probe_compass: bool,
    /// With `--net-probe`, `--probe-team` and a United Offensive
    /// `--game-dir`: a flamethrower victim. Runs the play client's flame
    /// report (`fx::flame`) off each firing body's eye, holds `wbuttons`
    /// 0x1 while it does and sends its `fdc`, printing a `FLAME` line per
    /// `fdc`, flag change and health change. Pairs with
    /// `client-probes/probe_flame` (docs/research/coduo-diff.md 19). Writes
    /// no fixture.
    #[arg(long)]
    probe_flame: bool,
    /// With `--net-probe` and `--probe-team`: build every cmd through the
    /// play client's own input (`play::input::PlayInput`), hold forward at
    /// this world yaw for 14 s once the join settles, then press weapon
    /// slot 1 and, 3 s on, `weapnext`, printing a `LADDER` line per snapshot
    /// whose weapon fields, `pm_flags`, sent weapon byte or 64-unit height
    /// band moved.
    /// `client-probes/probe_ladder` puts the player on mp_ship's hold
    /// ladder. Writes no fixture.
    #[arg(long, value_name = "YAW")]
    probe_ladder: Option<f32>,
    /// With `--probe-ladder`: run the climb script instead of the weapon
    /// binds (up at several pitches, down, a strafe and its release, off the
    /// top) and write every cmd and snapshot to
    /// `crates/server/tests/fixtures/playerstate/<map>-dm-ladder.txt`, which
    /// `tests/ladder_ab.rs` replays. `--save-ladder climb` runs the short
    /// script instead: straight up looking 45 up and off the top.
    /// `--save-ladder turn` sweeps the view past `bg_ladder_yawcap` on the
    /// rungs and at the foot (`tests/ladder_yawcap_ab.rs`). `--save-ladder
    /// grab` starts at the first live snapshot, waits for the placement and
    /// walks forward at the probe's yaw into the rungs
    /// (`tests/ladder_ab.rs`'s second-grab gate). Retail
    /// evidence when taken against tools/run_probe.sh; a run against
    /// vcod-server overwrites it.
    #[arg(long, requires = "probe_ladder", num_args = 0..=1, default_missing_value = "full", value_name = "SCRIPT")]
    save_ladder: Option<String>,
    /// With `--net-probe` and `--probe-team`: build every cmd through the
    /// play client's own input with the `a` command and the own events
    /// routed into its weapon selection, pull the trigger every 700 ms from
    /// 15 s to 19 s after the join settles, and print a `WEAPSEL` line per
    /// snapshot whose weapon fields or sent weapon byte moved, and one per
    /// own pickup, dry-fire, drop, raise, putaway and alt event, per `a` and
    /// per press of `weapalt` (26 s, 30 s, 38 s) and `weapon colt_mp` (34 s).
    /// `client-probes/probe_weapsel` switches weapons by script, empties the
    /// rifle and hands over a thompson. Writes
    /// `crates/server/tests/fixtures/playerstate/<map>-dm-weapsel.txt`
    /// (`--capture-tag` names another).
    #[arg(long)]
    probe_weapsel: bool,
    /// With `--net-probe` and `--probe-team`: a United Offensive tank crew
    /// member. `client-probes/probe_vehicle` puts client 0 and 1 beside the
    /// first tank; `driver` (start it first) climbs in 2 s after its spawn,
    /// drives, turns, fires the main gun and the coaxial MG, `gunner` climbs
    /// in 4 s after its spawn and sweeps and fires its MG; `overheat` is the
    /// gunner holding its MG for 6 s; `sit` (alone, beside
    /// `client-probes/probe_vehicle_armor`) sits still in each seat in turn
    /// and gets out. Prints a `VEH`
    /// line per snapshot per vehicle and seated player. Writes no fixture;
    /// add `--save-snapshots --capture-tag <tag>` for one.
    #[arg(long, value_enum, value_name = "ROLE")]
    probe_vehicle: Option<crate::vehicle_probe::VehicleRole>,
    /// With `--net-probe` and `--probe-team`: a United Offensive weapon on
    /// foot, beside `client-probes/probe_uofx` with `probe_weapon` naming
    /// it. Takes the script's `a` select, then `arty` sights and fires the
    /// artillery binoculars, `smoke` throws the smoke grenade, `mg` sights
    /// to deploy the bipod, holds the trigger 3 s and breaks it down, `hold`
    /// sights and holds the trigger (`client-probes/probe_vehicle_armor`
    /// aims it), `aim` holds it at the view the gsc sets every frame, `throw`
    /// pulls a grenade back and lets go twice at that view (`aim` and
    /// `throw` run on any patch: `client-probes/probe_scriptmodel`), and
    /// `direct` holds that view with the buttons, lean, stance and keys the
    /// gsc sends it (`client-probes/probe_leanhit`, `probe_spawnbody`).
    /// Writes no fixture; a following spectator records it (recipe in
    /// docs/research/coduo-diff.md section 19).
    #[arg(
        long,
        value_enum,
        value_name = "SCRIPT",
        conflicts_with = "probe_vehicle"
    )]
    probe_uo_weapon: Option<crate::vehicle_probe::WeaponScript>,
    /// With `--net-probe`: stay a quiet spectator and print a `CLOCK` line a
    /// second, the client clock's lag behind the newest snapshot, how often
    /// it ran past it and how evenly it stepped, beside the same for the
    /// clock it replaced (`OLD`). `--probe-cmd-ms` is the frame time;
    /// `VCOD_NETSIM` adds a bad link. Writes no fixture.
    #[arg(long)]
    pub probe_clock: bool,
    /// With `--net-probe`: a spectator that keeps decoding where a client
    /// would stop, for the server's edge cases: `silence` (the `sv_timeout`
    /// drop), `zombie` (a disconnect before entering the world),
    /// `zombie-active` (one after), `reconnect` (a connect again into the
    /// held slot), `lean-fly` (the lean bits on a spectator's flight),
    /// `spec-fly` (forward held, every origin printed, beside
    /// `client-probes/probe_specspawn`) and `watch` (the entity list as it
    /// changes).
    /// Prints `EDGE` lines; writes no fixture (`edge_probe.rs`).
    #[arg(long, value_enum)]
    pub probe_edge: Option<crate::edge_probe::EdgeMode>,
    /// With `--net-probe` and `--probe-team`: the scripted gametype capture.
    /// Presses use as the gsc probe's `setClientCvar("probe_use", ...)` says
    /// (`tap`, `hold`, `0`), keeps the view the server set, sends `score`
    /// every 2 s and writes every serverCommand and every snapshot whose
    /// HUD, objectives, roster or players moved to
    /// crates/server/tests/fixtures/gametypes/<map>-<gametype>-<ROLE>.txt,
    /// the gametype without its `probe_` prefix. Pairs with
    /// `client-probes/probe_re` and `probe_bel`; the fixture header carries
    /// the recipe. Refuses to replace a fixture without --overwrite-fixture.
    #[arg(long, value_name = "ROLE")]
    save_scripted: Option<String>,
    /// With --net-probe: take the download path a joining client takes.
    /// Every referenced pak no installed pak matches by checksum is fetched
    /// into DIR/<game>/ (a scratch directory, never the install), named as
    /// retail names it, then `donedl`; each prints its checksum beside the
    /// server's `sv_referencedPaks` entry. Writes no fixture. Under
    /// --vanilla it fetches nothing and searches only the stock paks.
    #[arg(long, value_name = "DIR")]
    probe_download: Option<std::path::PathBuf>,
    /// Walk the --probe-slope route and write every usercmd sent and every
    /// snapshot's movement fields to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-slope-<ms>ms.txt,
    /// which playerstate_slope_ab.rs replays on our mover, origin for origin.
    /// Retail evidence when taken against tools/run_server.sh; a run against
    /// vcod-server overwrites it.
    #[arg(long)]
    pub save_slope: bool,
    /// With --save-slope: keep the snapshots of every `pm_type` (dead,
    /// spectator, intermission), not only a playing or linked body's. For
    /// `predict_ab`'s state changes beside `client-probes/probe_uostates`.
    #[arg(long, requires = "save_slope")]
    slope_every_state: bool,
    /// With --save-slope: log from the first playing snapshot, not 3 s
    /// past the weapon pick, so a `--probe-vehicle` capture holds the
    /// spawn, the placement and the boarding.
    #[arg(long, requires = "save_slope")]
    slope_from_spawn: bool,
    /// With --save-slope or --probe-slope: a prone crawl instead of the
    /// route, from where `client-probes/probe_prone` puts the player on
    /// mp_carentan (`+set probe_teleport 1`, `probe_spot` street or mound),
    /// with the value the world yaw up the grade (90 on the street, 270 on
    /// the mound): prone, crawl up, down and across the grade, turn past the
    /// prone yaw cap, sweep the pitch past the prone pitch clamp, crawl
    /// looking down and downhill, stand. Tag the fixture with --capture-tag.
    #[arg(long, value_name = "UPHILL_YAW")]
    probe_prone: Option<f32>,
    /// With --save-slope: the movement script instead of the route, from
    /// where `client-probes/probe_prone` puts the player on mp_carentan's
    /// street (`+set probe_teleport 1 +set probe_spot street`), facing world
    /// yaw 0 along it: standing jumps inside and past
    /// patch 1.5's jump penalty, running jumps, a held jump, crouch and
    /// prone with their walks, the prone press while running (1.1's dive),
    /// diagonal runs and semi-automatic shots at the hip and down the sight.
    /// Tag the fixture with --capture-tag (`moves-1.1`, `moves-1.5`), which
    /// predict_ab.rs replays through the predictor under that version's
    /// movement rules.
    #[arg(long)]
    probe_moves: bool,
    /// With --save-slope: the prone turn and gait script instead of the
    /// route, from the street placement of `client-probes/probe_prone`
    /// (`+set probe_teleport 1 +set probe_spot street`), facing world yaw 90:
    /// a crawl back toward the kerb, quarter turns there and while crawling,
    /// a turn past the prone yaw cap, then a run, lean runs, a lean strafe, a
    /// sighted walk and a leaning crouch walk. Tag the fixture with
    /// --capture-tag (`turns-1.1`, `turns-1.5`), which moves_ab.rs replays
    /// under that version's rules.
    #[arg(long)]
    probe_turns: bool,
    /// With --save-slope, on a United Offensive server: the sprint and
    /// bipod script instead of the route, from the street placement of
    /// `client-probes/probe_prone` (`+set probe_teleport 1 +set probe_spot
    /// street`), facing world yaw 0, with --probe-weapon mg30cal_mp: a
    /// sprint to exhaustion, its regeneration, sprint with each other gait,
    /// and the bipod deployed prone and swung past its yaw cap. Tag the
    /// fixture `sprint-uo`; moves_ab.rs replays it under UO's rules.
    #[arg(long)]
    probe_sprint: bool,
    /// With --save-slope, on a United Offensive server: an automatic held
    /// from the hip, then down the sight, then tapped, from the street
    /// placement of `client-probes/probe_prone`, facing world yaw 0, with
    /// --probe-weapon naming the automatic (`mp44_mp`). Tag the fixture
    /// `autofire-uo`; moves_ab.rs replays it under UO's rules. On 1.1 with
    /// `--probe-weapon springfield_mp` it holds a bolt-action's trigger
    /// through the shot and the rechamber (tag `springfield-held-1.1`).
    #[arg(long)]
    probe_autofire: bool,
    /// With --save-slope: the talk bit (the console or the chat line up)
    /// held over the sight, a run, a lean, a shot, a jump, a crouch and a
    /// prone crawl, from the street placement of `client-probes/probe_prone`,
    /// facing world yaw 0. Tag the fixture `talk-1.1`, `talk-1.5` or
    /// `talk-uo`; moves_ab.rs replays each under its rules.
    #[arg(long)]
    probe_talk: bool,
    /// With --save-slope, on a 1.5 or United Offensive server: shots whose
    /// muzzle moves, from a placement of `client-probes/probe_moveshot` on
    /// mp_carentan facing world yaw 0. `walk` (`+set probe_spot street`,
    /// --probe-weapon thompson_mp) fires bursts strafing, running,
    /// crouched, sighted and leaning; `bipod` (UO, `+set probe_spot wall`,
    /// --probe-weapon mg30cal_mp) from the bipod deployed short of a wall
    /// with the view swung off it. The wait before the script goes in as
    /// `!wsnap` and `!wcmd` lines. Tag the fixture `moveshot-<part>-1.5` or
    /// `-uo`; moveshot_ab.rs replays it (cod11-combat.md 2.1).
    #[arg(long, value_enum, value_name = "PART")]
    probe_moveshot: Option<MoveShot>,
    /// With --save-slope: a move key pressed on a prone body, from the
    /// street placement of `client-probes/probe_prone` (`+set
    /// probe_teleport 1 +set probe_spot street`), facing world yaw 0.
    /// `rifle` (any server, --probe-weapon springfield_mp) presses one
    /// under the sight, in a rechamber and in a swing's windup, then a
    /// prone press from a crouched walk back; `lmg` (United Offensive,
    /// --probe-weapon mg30cal_mp) on the deployed bipod, through its
    /// breakdown, and with the binoculars; `deploy` (United Offensive,
    /// --probe-weapon mg30cal_mp, `client-probes/probe_uofoot` with `+set
    /// probe_spot mound`) deployed standing after a prone up and a prone
    /// down the mound, pitched past the clamp; `deploy-crouch` the same
    /// crouched (`probe_spot moundcrouch`). Tag the fixture
    /// `pronemove-<part>-<version>`; moves_ab.rs and animtimers_ab.rs
    /// replay it.
    #[arg(long, value_enum, value_name = "PART")]
    probe_pronemove: Option<ProneMove>,
    /// With --save-slope, on a United Offensive server: the bipod and
    /// grenade cook script from a placement of
    /// `client-probes/probe_uofoot` on mp_foy (`+set probe_teleport 1
    /// +set probe_spot stand|crouch|mount`), facing the mount the gsc
    /// faces, with --probe-weapon mg30cal_mp: the bipod deployed on
    /// the mount, swung past its cap, fired, broken down, then the frag
    /// thrown plain, cooked and cooked out. The value is the world yaw
    /// to face (the probe's view overrides the gsc's): 90 for stand and
    /// mount, 0 for crouch. Tag the fixture `uofoot-<spot>`; moves_ab.rs
    /// replays it under UO's rules.
    #[arg(long, allow_negative_numbers = true)]
    probe_uofoot: Option<f32>,
    /// With --save-slope, on a United Offensive server: the mounted MG
    /// script from a `client-probes/probe_turret` placement on mp_carentan
    /// (`+set probe_teleport 1`, 40 units behind the gun at (1712 1830)):
    /// the use tap onto the gun, the view swept past both arcs, a burst,
    /// the move, jump and stance keys pushed against the mount, the use tap
    /// off it and a walk away. The value is the gun's world yaw (229 there).
    /// Tag the fixture `uoturret-uo`; predict_ab.rs replays it under UO's
    /// rules.
    #[arg(long, allow_negative_numbers = true)]
    probe_uoturret: Option<f32>,
    /// With --save-slope, on a United Offensive server: a run at the wall
    /// under a window too low for a standing body, a jump and crouch held
    /// in the air, from a placement of `client-probes/probe_crouchjump`
    /// (`+set probe_teleport 1 +set probe_spot cassino|streets|low`). The
    /// value is the world yaw to face: -128 for cassino, 180 for streets,
    /// -90 for low. Tag the fixture `crouchjump-<spot>`; moves_ab.rs
    /// replays it under UO's rules (bot-navigation.md, "Walled-in spawns").
    #[arg(long, allow_negative_numbers = true)]
    probe_crouchjump: Option<f32>,
    /// With --probe-crouchjump: the same run and jump with no crouch, the
    /// control (tag `crouchjump-<spot>-stand`).
    #[arg(long, requires = "probe_crouchjump")]
    probe_crouchjump_stand: bool,
    /// With --save-slope: runs into a patch, slides along it both ways,
    /// runs at it turned, jumps at it, crouch walks and crawls along it and
    /// stands up beside it, from a placement of
    /// `client-probes/probe_patchwalk` on mp_carentan (`+set
    /// probe_teleport 1 +set probe_spot pole|pillar|kerb`). The value is
    /// the world yaw to face, which the gsc logs with the placement. Tag
    /// the fixture `patchwalk-<spot>-<1.1|1.5|uo>`; moves_ab.rs replays it
    /// under that version's rules (bsp-ibsp59-format.md, "Patch collision
    /// on 1.5 and UO").
    #[arg(long, allow_negative_numbers = true)]
    probe_patchwalk: Option<f32>,
    /// Milliseconds between usercmds the probe sends. A retail client at 125
    /// fps sends one every 8 ms; the default is what every capture so far
    /// was taken with.
    #[arg(long, default_value_t = 16)]
    pub probe_cmd_ms: u64,
    /// Turns --save-hit into a measurement instead of a capture: the shooter
    /// taps once per entry of a static table of pitch offsets around the aim
    /// at the target's eye, echoing the offset on each !trace line, and writes
    /// no fixture. Pair those lines with the sHitLoc the server's own
    /// games_mp.log D; records carry and every vertical hit-location boundary
    /// falls out of the two. Give both probes a long --probe-secs: the sweep
    /// spends an offset only on a tap that had a live target to hit.
    #[arg(long, conflicts_with_all = ["probe_melee", "probe_grenade", "probe_grenade_death"])]
    probe_sweep: bool,
    /// Runs the hit pair's melee script instead of its bullet one: the shooter
    /// walks to within 40 units and taps the melee bit rather than the
    /// trigger. Both halves write <map>-<gametype>-melee-shooter.txt and
    /// -melee-target.txt, so pass it to the --probe-target half too or that
    /// half overwrites the committed bullet fixture.
    #[arg(long, conflicts_with_all = ["probe_sweep", "probe_grenade", "probe_grenade_death"])]
    probe_melee: bool,
    /// Runs the hit pair's grenade script: the shooter walks to within 300
    /// units, switches to the frag, cooks for a second, releases at the
    /// target's feet, watches the missile out and then throws a second one,
    /// barely cooked, at the ground beside it. Writes
    /// <map>-<gametype>-grenade-shooter.txt and -grenade-target.txt; pass it
    /// to the --probe-target half as well.
    #[arg(long, conflicts_with_all = ["probe_sweep", "probe_melee", "probe_grenade_death"])]
    probe_grenade: bool,
    /// The grenade script with a `kill` sent 500 ms into the cook, which is
    /// what puts the grenade a death drops on the wire. Writes
    /// <map>-<gametype>-grenade-death-shooter.txt and -grenade-death-target.txt.
    #[arg(long, conflicts_with_all = ["probe_sweep", "probe_melee", "probe_grenade"])]
    probe_grenade_death: bool,
    /// Suffix for the --save-entities, combat, hit pair (--save-hit,
    /// --probe-target) and round-restart fixture names, so a capture
    /// taken under different conditions lands beside the plain one rather
    /// than on top of it: --capture-tag players writes
    /// <map>-<gametype>-players.txt. With --save-snapshots it writes the
    /// gamestate-<tag>.bin and snapshots-<tag>.bin pair and captures for
    /// all of --probe-secs (the 1.5 pair is tagged 1.5). A tagged write
    /// refuses a path that already exists, since a tag can spell a
    /// committed fixture's name; --overwrite-fixture is how you mean it.
    #[arg(long)]
    capture_tag: Option<String>,
    /// Let a --capture-tag write replace a fixture that is already there.
    /// Without it the run fails rather than putting a capture taken against
    /// vcod's own server where the retail oracle lives.
    #[arg(long)]
    overwrite_fixture: bool,
    /// Which team --net-probe answers the stock team menu with: allies, axis,
    /// autoassign or spectator. On its own it makes the probe join and then
    /// report the roster once a second, which is how two probes on opposite
    /// teams measured what clientState.team carries.
    #[arg(long)]
    pub probe_team: Option<String>,
    /// What --net-probe answers the stock weapon menu with, instead of the
    /// nationality's default rifle: any weapon that menu allows, e.g.
    /// kar98k_sniper_mp on axis. A weapon the menu refuses reopens it and the
    /// probe never spawns.
    #[arg(long)]
    pub probe_weapon: Option<String>,
    /// Seconds --net-probe stays connected; an SD round boundary needs a few minutes
    #[arg(long, default_value_t = 65)]
    pub probe_secs: u64,
    /// With --net-probe: `SECS:COMMAND`, a client command sent SECS seconds
    /// after the gamestate, e.g. `5:say_team hello`. Repeatable; keep them
    /// 800 ms apart or retail's flood window drops the later one. With
    /// --probe-team the probe joins first, so `kill` between two says
    /// measures a dead speaker. `{sid}` in COMMAND becomes the current
    /// serverId, so `12:mr {sid} 0 spectator` answers a menu. Every `h`/`i`
    /// chat line that comes back is printed with its control bytes escaped.
    /// Writes no fixture.
    #[arg(long, value_name = "SECS:COMMAND", value_parser = crate::parse_probe_say)]
    probe_say: Vec<(f32, String)>,
    /// With --net-probe: the userinfo `rate` the probe connects with instead
    /// of 25000. Off the LAN it paces every message (docs/research/
    /// cod11-server-handshake.md, "Message pacing").
    #[arg(long)]
    probe_rate: Option<String>,
    /// With --net-probe: the userinfo `snaps` instead of 20.
    #[arg(long)]
    probe_snaps: Option<String>,
    /// With --net-probe: the userinfo `name` instead of `vcod`, so two
    /// probes on one server tell apart (a kick vote names its target).
    #[arg(long)]
    probe_name: Option<String>,
}

impl ProbeArgs {
    /// The `--flags` the command line set explicitly (not a default) that the
    /// game's probe does not read, so the run can refuse them instead of
    /// ignoring them. `read` holds field names.
    pub fn unread(matches: &clap::ArgMatches, read: &[&str]) -> Vec<String> {
        use clap::{Args as _, parser::ValueSource};
        Self::augment_args(clap::Command::new(""))
            .get_arguments()
            .map(|a| a.get_id().as_str())
            .filter(|id| !read.contains(id))
            .filter(|id| matches.value_source(id) == Some(ValueSource::CommandLine))
            .map(|id| format!("--{}", id.replace('_', "-")))
            .collect()
    }
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
pub fn probe(
    addr: &str,
    args: ProbeArgs,
    fs: Option<&vcod_common::pk3::Pk3Fs>,
) -> anyhow::Result<()> {
    let ProbeArgs {
        save_fixture,
        save_snapshots,
        save_demo,
        save_configstrings,
        save_playerstate,
        save_motion,
        save_combat,
        save_ads,
        save_grenade,
        probe_sway,
        save_lean,
        save_hit,
        probe_target: save_target,
        probe_kill_cmds: kill_cmds,
        save_mapchange,
        save_roundrestart,
        save_entities,
        probe_pvs: pvs,
        probe_slope: slope,
        probe_triggers: triggers,
        probe_plant: save_plant,
        probe_defuse: save_defuse,
        save_pickup,
        save_turret,
        probe_turret_target_ms: turret_target_ms,
        save_bump,
        save_legs,
        save_shock,
        probe_bump_target,
        probe_follow,
        probe_killcam,
        probe_killcam_skip_ms: killcam_skip_ms,
        probe_fall,
        probe_fall_walk: fall_walk,
        probe_fall_prone: fall_prone,
        probe_yaw,
        probe_pitch_flip: pitch_flip,
        probe_ride,
        probe_items,
        probe_compass,
        probe_flame,
        probe_ladder,
        save_ladder,
        probe_weapsel,
        probe_vehicle,
        probe_uo_weapon: uo_weapon,
        probe_clock: _,
        probe_edge: _,
        save_scripted: scripted_role,
        probe_download: download,
        save_slope,
        slope_every_state,
        slope_from_spawn,
        probe_prone: prone_uphill,
        probe_moves,
        probe_turns,
        probe_sprint,
        probe_autofire,
        probe_talk,
        probe_moveshot,
        probe_pronemove,
        probe_uofoot,
        probe_uoturret,
        probe_crouchjump,
        probe_crouchjump_stand: crouchjump_stand,
        probe_patchwalk,
        probe_cmd_ms: cmd_ms,
        probe_sweep,
        probe_melee,
        probe_grenade,
        probe_grenade_death,
        capture_tag: tag,
        overwrite_fixture: overwrite,
        probe_team: team,
        probe_weapon: weapon,
        probe_secs: secs,
        probe_say: say,
        probe_rate: rate,
        probe_snaps: snaps,
        probe_name: name,
    } = args;
    let script = match () {
        _ if probe_sweep => ShooterScript::Sweep,
        _ if probe_melee => ShooterScript::Melee,
        _ if probe_grenade => ShooterScript::Grenade,
        _ if probe_grenade_death => ShooterScript::GrenadeDeath,
        _ => ShooterScript::Hit,
    };
    let (team, weapon) = (team.as_deref(), weapon.as_deref());
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
        InputScript::new(0.0, moves_script())
    } else if probe_turns {
        InputScript::new(90.0, turns_script())
    } else if probe_sprint {
        InputScript::new(0.0, sprint_script())
    } else if probe_autofire {
        InputScript::new(0.0, autofire_script())
    } else if probe_talk {
        InputScript::new(0.0, talk_script())
    } else if let Some(part) = probe_moveshot {
        InputScript::new(0.0, moveshot_script(part))
    } else if let Some(part) = probe_pronemove {
        InputScript::switching(0.0, pronemove_script(part), part.switches())
    } else if let Some(yaw) = probe_uofoot {
        InputScript::switching(yaw, uofoot_script(), UOFOOT_SWITCHES)
    } else if let Some(yaw) = probe_uoturret {
        InputScript::new(yaw, uoturret_script())
    } else if let Some(yaw) = probe_crouchjump {
        InputScript::new(yaw, crouchjump_script(!crouchjump_stand))
    } else if let Some(yaw) = probe_patchwalk {
        InputScript::new(yaw, patchwalk_script())
    } else {
        InputScript::new(prone_uphill.unwrap_or(0.0), prone_script())
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
                            while let Some(line) = quick_chat.drain(f64::INFINITY) {
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
    use super::*;
    use vcod_common::net::Msec;

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

    /// A snapshot with the given playerstate fields and item entities
    /// (`(num, index, xyz)`), for driving [`PickupProbe::step`].
    pub(super) fn pickup_snap(
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
            server_time: Msec::from_raw(1000 + 50 * num as i32),
            message_num: num,
            delta_num: -1,
            snap_flags: 0,
            ps,
            entities,
            clients: BTreeMap::new(),
            valid: true,
        }
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
}
