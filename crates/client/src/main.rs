mod audio;
mod camera;
mod clock_probe;
mod console;
mod entities;
mod entity_light;
mod frontend;
mod fx;
mod gamma;
mod head_icon;
mod hud;
mod hud_text;
mod loading;
mod play;
mod probe;
mod quick_chat;
mod quit;
mod renderer;
mod sky;
mod turret;
mod viewmodel;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use glam::Vec3;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use vcod_common::movetrace::MoveWorld;
use vcod_common::pk3::Pk3Fs;
use vcod_common::{bsp, collision, mesh, net, pmove, props, weapon, xmodel};

use camera::{FlyCamera, InputState};
use renderer::{DynamicModelInstance, Renderer};

#[derive(Parser)]
#[command(about = "Call of Duty (2003) map viewer and client")]
struct Args {
    /// Map name, e.g. mp_pavlov
    map: Option<String>,
    /// List all maps found in the pk3 search path
    #[arg(long)]
    list: bool,
    /// Game install (holds main/). Defaults to $COD_DIR, else the executable's directory.
    #[arg(long, default_value_os_t = vcod_common::game_dir::default_game_dir())]
    game_dir: std::path::PathBuf,
    /// Pk3 subdirectory: main for CoD1, uo for United Offensive (untested)
    #[arg(long, default_value = "main")]
    mod_dir: String,
    /// Spawn as a soldier and walk (first person) instead of flying
    #[arg(long)]
    walk: bool,
    /// Start with the debug overlay visible (F3 toggles it at runtime)
    #[arg(long)]
    debug_overlay: bool,
    /// Headless: connect to a CoD server, dump the gamestate, then exit
    #[arg(long)]
    net_probe: Option<String>,
    /// Connect to a CoD server (ip:port) and join it through the stock team
    /// and weapon menus; spectator is one of the team menu's choices
    #[arg(long)]
    connect: Option<String>,
    /// Answer the stock team menu with this after --connect or the console's
    /// `connect`
    #[arg(long)]
    team: Option<String>,
    /// Answer the stock weapon menu with this weapon file name (e.g. m1carbine_mp)
    #[arg(long)]
    weapon: Option<String>,
    /// Overwrite the committed gamestate.bin fixture with the --net-probe capture.
    /// Off by default: the parser tests pin that file, and a capture from another
    /// map or a mid-round server is not a drop-in. Otherwise the probe writes to tmp/.
    #[arg(long)]
    save_fixture: bool,
    /// Overwrite the committed snapshots.bin fixture only; gamestate.bin stays pinned.
    #[arg(long)]
    save_snapshots: bool,
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
    /// Join a team, then run a scripted weapon sequence -- single shot,
    /// sustained fire, reload, fire crouched and prone -- and write the retail
    /// playerstate to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-combat.txt.
    /// Separate from --save-motion because that one samples a settled state
    /// and a shot is a transient: this fixture carries every snapshot, since
    /// the playerstate event ring holds four slots and overwrites as it fires.
    /// Each firing step waits for weaponstate to read ready before it taps, and
    /// the reload holds the weapon file's reloadTime, so a weapon still busy
    /// from the step before cannot be recorded under a firing label. The
    /// sequence takes about 40 s, inside the default --probe-secs.
    #[arg(long)]
    save_combat: bool,
    /// Join a team, then run the sight script -- sight held, released, held
    /// through a reload, through a shot and while walking, plus a hip shot
    /// and a walk to kick the spread counter -- and write every snapshot's
    /// fWeaponPosFrac and aimSpreadScale to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-ads.txt.
    /// The same machine as --save-combat with another script; the two are
    /// separate fixtures because the ramp is a transient the combat steps
    /// never hold still for.
    #[arg(long)]
    save_ads: bool,
    /// Join a team, then run the grenade script -- one melee swing on the
    /// rifle, a switch to the frag, a cooked throw, a cook held past the pin,
    /// a cook cancelled by a weapon switch and a throw at the ground -- and
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
    /// carbine for a panzerfaust and back. Writes
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
    /// or health moved, and a `CMDS` line of the `serverTime`s it sent every
    /// 60 cmds; `client-probes/probe_fall` does the dropping. Writes no
    /// fixture.
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
    /// With `--net-probe` and `--probe-team`: stand at world yaw 0 and hold
    /// this view pitch in alternate 400 ms windows (0 between), printing a
    /// `PITCH` line per snapshot whose pitch moved. The target half of
    /// `client-probes/probe_turret`'s `probe_pose 1` measurement. Writes no
    /// fixture.
    #[arg(long, value_name = "DEG")]
    probe_pitch_flip: Option<f32>,
    /// With `--net-probe` and `--probe-team`: stand still and print a `RIDE`
    /// line per snapshot with the origin, velocity, ground entity and view
    /// yaw, the mover push and ride capture's wire half;
    /// `client-probes/probe_ride` moves the brush model under the player.
    /// Writes no fixture.
    #[arg(long)]
    probe_ride: bool,
    /// With `--net-probe` and `--probe-team`: stand still and print an `ITEM`
    /// line per snapshot an item entity appeared, changed or left, with its
    /// index, owner, ground, both trajectories and its event ring;
    /// `client-probes/probe_itemdrop` drops, spawns and respawns the items.
    /// Writes no fixture.
    #[arg(long)]
    probe_items: bool,
    /// With `--net-probe` and `--probe-team`: stand still and print a
    /// `COMPASS` line per snapshot whose `iCompassFriendInfo`, eye or player
    /// list changed, the field decoded as the cgame reads it. Two on one team
    /// measure the out-of-view teammate the server packs there. Writes no
    /// fixture.
    #[arg(long)]
    probe_compass: bool,
    /// With `--net-probe`: stay a quiet spectator and print a `CLOCK` line a
    /// second, the client clock's lag behind the newest snapshot, how often
    /// it ran past it and how evenly it stepped, beside the same for the
    /// clock it replaced (`OLD`). `--probe-cmd-ms` is the frame time;
    /// `VCOD_NETSIM` adds a bad link. Writes no fixture.
    #[arg(long)]
    probe_clock: bool,
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
    /// server's `sv_referencedPaks` entry. Writes no fixture.
    #[arg(long, value_name = "DIR")]
    probe_download: Option<std::path::PathBuf>,
    /// Walk the --probe-slope route and write every usercmd sent and every
    /// snapshot's movement fields to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-slope-<ms>ms.txt,
    /// which playerstate_slope_ab.rs replays on our mover, origin for origin.
    /// Retail evidence when taken against tools/run_server.sh; a run against
    /// vcod-server overwrites it.
    #[arg(long)]
    save_slope: bool,
    /// With --save-slope or --probe-slope: a prone crawl instead of the
    /// route, from where `client-probes/probe_prone` puts the player on
    /// mp_carentan (`+set probe_teleport 1`, `probe_spot` street or mound),
    /// with the value the world yaw up the grade (90 on the street, 270 on
    /// the mound): prone, crawl up, down and across the grade, turn past the
    /// prone yaw cap, sweep the pitch past the prone pitch clamp, crawl
    /// looking down and downhill, stand. Tag the fixture with --capture-tag.
    #[arg(long, value_name = "UPHILL_YAW")]
    probe_prone: Option<f32>,
    /// Milliseconds between usercmds the probe sends. A retail client at 125
    /// fps sends one every 8 ms; the default is what every capture so far
    /// was taken with.
    #[arg(long, default_value_t = 16)]
    probe_cmd_ms: u64,
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
    /// Suffix for the --save-entities and combat fixture names, so a capture
    /// taken under different conditions lands beside the plain one rather
    /// than on top of it: --capture-tag players writes
    /// <map>-<gametype>-players.txt. A tagged write refuses a path that
    /// already exists, since a tag can spell a committed fixture's name;
    /// --overwrite-fixture is how you mean it.
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
    probe_team: Option<String>,
    /// What --net-probe answers the stock weapon menu with, instead of the
    /// nationality's default rifle: any weapon that menu allows, e.g.
    /// kar98k_sniper_mp on axis. A weapon the menu refuses reopens it and the
    /// probe never spawns.
    #[arg(long)]
    probe_weapon: Option<String>,
    /// Seconds --net-probe stays connected; an SD round boundary needs a few minutes
    #[arg(long, default_value_t = 65)]
    probe_secs: u64,
    /// With --net-probe: `SECS:COMMAND`, a client command sent SECS seconds
    /// after the gamestate, e.g. `5:say_team hello`. Repeatable; keep them
    /// 800 ms apart or retail's flood window drops the later one. With
    /// --probe-team the probe joins first, so `kill` between two says
    /// measures a dead speaker. Every `h`/`i` chat line that comes back is
    /// printed with its control bytes escaped. Writes no fixture.
    #[arg(long, value_name = "SECS:COMMAND", value_parser = parse_probe_say)]
    probe_say: Vec<(f32, String)>,
    /// Run without sound (also what happens when no output device opens).
    #[arg(long)]
    no_audio: bool,
    /// Master volume, 0.0 to 1.0: sets `mss_volume`, which the options
    /// screen's slider also sets and the config keeps (default 0.8).
    #[arg(long)]
    volume: Option<f32>,
}

/// The map every mode reads: the renderer builds from it, entities resolve
/// submodels in it. `None` while `--connect` is between maps.
struct World {
    bsp: bsp::Bsp,
}

/// Where `--connect` is between connecting and drawing a map.
enum Phase {
    Connecting,
    Loading {
        loader: loading::MapLoader,
    },
    /// Boxed to keep the variants a similar size.
    Live(Box<LivePhase>),
}

/// Everything `--connect` needs to draw a map.
struct LivePhase {
    world: collision::CollisionWorld,
    /// Configstring 7's weapons, for prediction.
    weapons: Vec<Option<weapon::WeaponDef>>,
    scene: entities::EntityScene,
    events: net::events::EventTracker,
    /// Our own ring's events already played off the prediction.
    predicted_events: play::events::PredictedEvents,
    /// `cl.serverTime`: entities, the HUD and the cmds run on it.
    clock: play::clock::ServerClock,
    last_loop_snap: Option<u32>,
    /// Last frame's `entity_pos`, what prediction clips (docs/research/cod11-player-clip.md).
    drawn_pos: HashMap<u32, Vec3>,
}

fn live_phase(fs: &Pk3Fs, bsp: &bsp::Bsp, net: &net::NetClient<net::UdpTransport>) -> Phase {
    let world = collision::CollisionWorld::build(bsp, &props::collision_tris(fs, &bsp.entities));
    // A brush model clips only through its snapshot entity
    // (`pmove::movers::SnapshotMovers::place`), as retail's cgame meets it.
    for model in 1..world.model_count() {
        world.set_model_linked(model, false);
    }
    Phase::Live(Box::new(LivePhase {
        world,
        weapons: vcod_common::weapon_table::from_configstring(fs, net.configstring(7)),
        scene: entities::EntityScene::new(),
        events: net::events::EventTracker::new(),
        predicted_events: play::events::PredictedEvents::default(),
        clock: play::clock::ServerClock::default(),
        last_loop_snap: None,
        drawn_pos: HashMap::new(),
    }))
}

enum Mode {
    /// No map and no server: the main menu, or the full-screen console when
    /// no menu is up, as retail's is when disconnected.
    Idle,
    Fly(FlyCamera),
    /// Position follows the interpolated playerstate. The client joins
    /// through the stock menus and plays or spectates as the snapshot says.
    Online {
        /// Boxed to keep the variants a similar size.
        net: Box<net::NetClient<net::UdpTransport>>,
        cam: FlyCamera,
        /// Boxed to keep the variants a similar size.
        input: Box<play::input::PlayInput>,
        clock: play::cmds::CmdClock,
        ring: play::cmds::CmdRing,
        /// Boxed to keep the variants a similar size.
        predictor: Box<play::predict::Predictor>,
        /// Boxed to keep the variants a similar size.
        view: Box<play::view::OnlineView>,
        phase: Phase,
        /// Boxed to keep the variants a similar size.
        join: Box<play::join::Join>,
        /// The open script menu as drawn, keyed by the configstring name it
        /// was built for; rebuilt when `join` opens another.
        menu_view: Option<(String, hud::menu::MenuView)>,
    },
    Walk {
        /// Boxed to keep the variants a similar size.
        world: Box<collision::CollisionWorld>,
        /// Boxed to keep the variants a similar size.
        ps: Box<pmove::PlayerState>,
        input: pmove::PmInput,
        keys: WalkKeys,
        motion: viewmodel::ViewmodelMotion,
        /// `None` when the anims failed to load; the viewmodel then draws
        /// statically. Boxed to keep the variants a similar size.
        view_weapon: Option<Box<viewmodel::ViewWeapon>>,
        /// Minimal configstring table so weapon cues resolve through the same
        /// path as `--connect`: CS 7 carries [`WALK_LOADOUT`].
        configstrings: Vec<String>,
        /// Active index into [`WALK_LOADOUT`].
        weapon_slot: usize,
        /// Rounds behind the clip; refilled per weapon from its file.
        reserve: u32,
        /// Digit press latched until the next redraw loads that slot.
        switch_to: Option<usize>,
        /// Rigs already loaded, so switching back to a weapon is a lookup.
        rigs: viewmodel::RigCache,
        /// Raw counts since the last frame, drained into the sway once per redraw.
        mouse_delta: (f32, f32),
        /// Press edges latched until the next redraw; `ads_held`/`fire_held` are level.
        fire_edge: bool,
        fire_held: bool,
        reload_edge: bool,
        ads_held: bool,
        /// The gun's sway, which a scope overlay is centred on.
        gun_aim: hud::scope::GunAim,
    },
}

/// Held movement keys, folded into `PmInput`'s float axes once per frame.
#[derive(Default)]
struct WalkKeys {
    w: bool,
    s: bool,
    a: bool,
    d: bool,
}

/// The walk-mode arsenal on number keys 1..=N: every retail archetype (semi
/// pistol, full-auto SMGs, auto rifle, big-clip bolt rifle) plus kar98k as
/// the baseline and its scoped twin. Files carry `semiAuto`, `startAmmo`,
/// `adsBobFactor`.
const WALK_LOADOUT: [&str; 7] = [
    "colt_mp",
    "thompson_mp",
    "mp40_mp",
    "mp44_mp",
    "enfield_mp",
    "kar98k_mp",
    "kar98k_sniper_mp",
];

fn digit_slot(code: KeyCode) -> Option<usize> {
    Some(match code {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        KeyCode::Digit4 => 3,
        KeyCode::Digit5 => 4,
        KeyCode::Digit6 => 5,
        KeyCode::Digit7 => 6,
        _ => return None,
    })
}

impl WalkKeys {
    /// (forward, right) in -1..1, opposite keys cancel.
    fn axes(&self) -> (f32, f32) {
        let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as f32;
        (axis(self.w, self.s), axis(self.d, self.a))
    }
}

/// F3 overlay text, top line first. A free function because the caller holds
/// the renderer borrowed out of `App`.
#[allow(clippy::too_many_arguments)]
fn hud_lines(
    mode: &Mode,
    // The online entity scene; fly/walk have none.
    scene: Option<&entities::EntityScene>,
    stats: &hud_text::HudStats,
    // (build_instances, render, fx step+build_quads) ms
    cpu_ms: (f32, f32, f32),
    // (events drained, of those unrecognized)
    ev_counts: (u64, u64),
    // (particles, decals, lights)
    fx_counts: (usize, usize, usize),
    // (hud quads, hud build+upload ms, Hud::unknown); zeros outside Online
    hud_counts: (usize, f32, u64),
    audio: audio::AudioStats,
    r: &Renderer,
) -> Vec<String> {
    let fps = if stats.dt_smooth > 0.0 {
        1.0 / stats.dt_smooth
    } else {
        0.0
    };
    let (build_ms, render_ms, fx_ms) = cpu_ms;
    let mut lines = vec![
        format!(
            "fps {fps:5.1}  ({:5.2} ms, worst {:5.1})",
            stats.dt_smooth * 1000.0,
            stats.worst_ms
        ),
        format!("cpu ms: build {build_ms:5.2}  render {render_ms:5.2}"),
    ];
    let (fx_particles, fx_decals, fx_lights) = fx_counts;
    lines.push(format!(
        "fx p{fx_particles} d{fx_decals} l{fx_lights} {fx_ms:.2}ms"
    ));
    let (draws, insts, bones) = r.debug_counts();
    lines.push(format!("draw: world {draws}  inst {insts}  bones {bones}"));
    let vc = r.vis_counts();
    lines.push(format!(
        "vis: {} cells {}/{} soups {}/{} tris {}/{} props {}/{} occ {} {}  {:.2}ms",
        vc.mode.map_or("-".to_string(), |m| m.to_string()),
        vc.cells.0,
        vc.cells.1,
        vc.soups.0,
        vc.soups.1,
        vc.tris.0,
        vc.tris.1,
        vc.props.0,
        vc.props.1,
        vc.occluders,
        vc.portals_occluded,
        vc.gather_ms
    ));
    let (hud_quads, hud_ms, hud_unknown) = hud_counts;
    lines.push(format!("hud q{hud_quads} {hud_ms:.2}ms unk {hud_unknown}"));
    let (stage_draws, dropped, warns) = r.shader_stats();
    lines.push(format!(
        "shader: sky {}  stages {stage_draws}  dropped {dropped}  warns {warns}",
        r.sky_name().unwrap_or("none")
    ));
    lines.push(format!("fog: {}", r.fog_debug()));
    lines.push(format!(
        "audio v{} plays {} miss {} cull {} drop {} steal {} {:.2}ms",
        audio.voices,
        audio.plays,
        audio.misses,
        audio.culled,
        audio.drops,
        audio.steals,
        audio.step_ms
    ));
    let cam_line = |tag: &str, pos: Vec3, yaw: f32, pitch: f32| {
        format!(
            "{tag}: {:6.0} {:6.0} {:5.0}  yaw {:4.0} pitch {:3.0}",
            pos.x,
            pos.y,
            pos.z,
            yaw.to_degrees(),
            pitch.to_degrees()
        )
    };
    match mode {
        Mode::Idle => {}
        Mode::Fly(cam) => lines.push(cam_line("fly", cam.pos, cam.yaw, cam.pitch)),
        Mode::Walk { ps, .. } => {
            lines.push(cam_line("walk", ps.origin, ps.yaw, ps.pitch));
            lines.push(format!(
                "vel {:4.0}  ground {}",
                ps.velocity.length(),
                ps.on_ground as u8
            ));
        }
        Mode::Online {
            net,
            cam,
            predictor,
            phase,
            ..
        } => {
            lines.push(cam_line("online", cam.pos, cam.yaw, cam.pitch));
            let error = predictor.drawn_error();
            lines.push(format!(
                "pred {} miss {} err {:.1}",
                if error.is_some() { "on" } else { "off" },
                predictor.misses,
                error.unwrap_or(0.0)
            ));
            lines.push(format!(
                "net: {:?}  drops {}",
                net.state(),
                net.packet_drops()
            ));
            if let (Phase::Live(live), Some(snap)) = (phase, net.snapshots().newest()) {
                let c = &live.clock;
                let st = c.stats;
                lines.push(format!(
                    "clock: behind {:3}  reset {} fast {} +{} -{}  extrap {}",
                    snap.server_time - c.server_time(),
                    st.resets,
                    st.fast,
                    st.ahead,
                    st.back,
                    st.extrapolated
                ));
            }
            if let Some(snap) = net.snapshots().newest() {
                lines.push(format!(
                    "snap: ents {}  players {}  age {:3} ms",
                    snap.entities.len(),
                    snap.clients.len(),
                    net.snapshot_age().as_millis()
                ));
            }
            lines.push(format!(
                "interp miss/s {:4.1}  anim restarts/s {:4.1}",
                stats.misses_per_s, stats.restarts_per_s
            ));
            if let Some(scene) = scene
                && scene.stats.pending_assemblies > 0
            {
                lines.push(format!(
                    "loading: {} assemblies pending",
                    scene.stats.pending_assemblies
                ));
            }
            if let Some((got, size)) = net.download_progress() {
                lines.push(format!("download: {got}/{size} bytes"));
            }
            let (ev_seen, ev_unknown) = ev_counts;
            lines.push(format!("ev {ev_seen} unk {ev_unknown}"));
        }
    }
    lines
}

fn main() -> Result<()> {
    console::log::init();
    let args = Args::parse();
    quit::install();

    let dir = args.game_dir.join(&args.mod_dir);

    if let Some(addr) = &args.net_probe
        && args.probe_clock
    {
        return clock_probe::run(addr, args.probe_secs, args.probe_cmd_ms);
    }
    if let Some(addr) = &args.net_probe {
        // Sound cue resolution needs the game data, the wire-level prints do
        // not, so a failed open only costs the audio line.
        let fs = match Pk3Fs::open(&dir) {
            Ok(fs) => Some(fs),
            Err(e) => {
                log::warn!(
                    "cannot open game data in {} ({e:#}); the probe will not resolve sound aliases",
                    dir.display()
                );
                None
            }
        };
        use probe::ShooterScript;
        let script = match () {
            _ if args.probe_sweep => ShooterScript::Sweep,
            _ if args.probe_melee => ShooterScript::Melee,
            _ if args.probe_grenade => ShooterScript::Grenade,
            _ if args.probe_grenade_death => ShooterScript::GrenadeDeath,
            _ => ShooterScript::Hit,
        };
        return probe::probe(
            addr,
            probe::Save {
                fixture: args.save_fixture,
                snapshots: args.save_snapshots,
                configstrings: args.save_configstrings,
                playerstate: args.save_playerstate,
                motion: args.save_motion,
                slope: args.save_slope,
                prone: args.probe_prone,
                combat: args.save_combat,
                ads: args.save_ads,
                grenade: args.save_grenade,
                sway: args.probe_sway,
                entities: args.save_entities,
                hit: args.save_hit,
                target: args.probe_target,
                mapchange: args.save_mapchange,
                roundrestart: args.save_roundrestart,
                plant: args.probe_plant,
                defuse: args.probe_defuse,
                pickup: args.save_pickup,
                turret: args.save_turret,
                turret_target_ms: args.probe_turret_target_ms,
                bump: args.save_bump,
                bump_target: args.probe_bump_target,
                follow: args.probe_follow,
                killcam: args.probe_killcam,
                killcam_skip_ms: args.probe_killcam_skip_ms,
                fall: args.probe_fall,
                fall_walk: args.probe_fall_walk,
                fall_prone: args.probe_fall_prone,
                pitch_flip: args.probe_pitch_flip,
                ride: args.probe_ride,
                items: args.probe_items,
                compass: args.probe_compass,
                scripted: args.save_scripted.clone(),
                download: args.probe_download.clone(),
            },
            args.capture_tag.clone(),
            args.overwrite_fixture,
            args.probe_pvs,
            args.probe_slope,
            args.probe_triggers,
            args.probe_cmd_ms,
            script,
            args.probe_team.as_deref(),
            args.probe_weapon.as_deref(),
            args.probe_secs,
            &args.probe_say,
            fs.as_ref(),
        );
    }

    let fs =
        Pk3Fs::open(&dir).with_context(|| format!("cannot open game data in {}", dir.display()))?;

    if args.list {
        for name in fs.find_maps() {
            println!("{name}");
        }
        return Ok(());
    }

    // The config's binds and cvars, or the defaults when there is none yet.
    let config_path = dir.join(CONFIG_FILE);
    let mut shell = console::shell::Shell::new();
    if let Ok(text) = std::fs::read_to_string(&config_path) {
        for effect in shell.execute(&text) {
            if let console::shell::Effect::Print(line) = effect {
                log::warn!("{CONFIG_FILE}: {line}");
            }
        }
    }
    if let Some(v) = args.volume {
        shell.execute(&format!("set mss_volume {v}"));
    }

    let net_client = match &args.connect {
        Some(addr) => {
            let mut net = net::NetClient::connect(addr)
                .with_context(|| format!("cannot open a socket to {addr}"))?;
            net.set_userinfo(shell.userinfo());
            Some(net)
        }
        None => None,
    };

    // Fly and walk need their map up front; online learns it from the
    // server inside the loop and loads through the same path as a map change.
    // With neither, the window opens on the console.
    let local = if net_client.is_none()
        && let Some(map) = args.map.as_deref()
    {
        let Some(path) = fs.resolve_map(map) else {
            let all = fs.find_maps();
            let needle = map.to_lowercase();
            let similar: Vec<String> = all
                .iter()
                .filter(|m| m.to_lowercase().contains(&needle))
                .cloned()
                .collect();
            let suggestions = if similar.is_empty() { all } else { similar };
            bail!("map '{map}' not found; similar: {}", suggestions.join(", "));
        };
        let data = fs
            .read(&path)
            .with_context(|| format!("cannot read {path}"))?;
        let bsp = bsp::parse(&data).with_context(|| format!("cannot parse {path}"))?;
        Some((map.to_string(), bsp))
    } else {
        None
    };

    let mut rigs = viewmodel::RigCache::default();
    let (viewmodel, view_weapon) = if args.walk && local.is_some() {
        rigs.load(&fs, "kar98k_mp", None).unwrap_or_else(|| {
            log::warn!("no viewmodel; walking without one");
            (Arc::from([]), None)
        })
    } else {
        (Arc::from([]), None)
    };

    let hud = if net_client.is_some() {
        match hud::Hud::new(&fs) {
            Ok(hud) => Some(hud),
            Err(e) => {
                log::warn!("hud: {e}, disabling the on-screen HUD");
                None
            }
        }
    } else {
        None
    };
    // The front end's labels need it too.
    let localized = vcod_common::localize::Localized::load(&fs);

    // Fly and walk have no aliases, but `.efx` spawns carry cues and need a listener.
    let mut audio = audio::AudioSystem::new(
        &fs,
        audio::AudioOpts {
            enabled: !args.no_audio,
            volume: shell.cvar_f32("mss_volume"),
        },
    );
    if !audio.enabled() {
        println!("audio: silent (no output device, or --no-audio)");
    }

    let (mode, world, title) = if let Some(net) = net_client {
        (
            online_mode(net, args.team.clone(), args.weapon.clone()),
            None,
            "vcod — connecting".to_string(),
        )
    } else if let Some((map, bsp)) = local {
        audio.on_gamestate(&map);
        // No server to send configstring 3; every stock MP map ships an
        // `ambient_<map>` alias (iw_sound.csv), unknown names just stay silent.
        let ambient = format!("ambient_{map}");
        audio.set_ambient(&fs, Some(&ambient));
        let mode = if args.walk {
            walk_mode(&map, &bsp, &fs, view_weapon, rigs)?
        } else {
            Mode::Fly(match bsp::find_spawn(&bsp.entities) {
                Some((origin, yaw)) => FlyCamera::new(Vec3::from(origin) + Vec3::Z * 60.0, yaw),
                None => {
                    let (min, max) = mesh::map_bounds(&bsp);
                    let center = (Vec3::from(min) + Vec3::from(max)) * 0.5;
                    log::warn!("no player spawn found, starting at the center of the map");
                    FlyCamera::new(center, 0.0)
                }
            })
        };
        (mode, Some(World { bsp }), format!("vcod — {map}"))
    } else {
        (Mode::Idle, None, "vcod".to_string())
    };

    println!("click to capture mouse, Esc to release");
    if args.walk {
        println!("WASD move, Space jump, Ctrl crouch, Z prone, Q/E lean, Shift walk");
        println!("LMB fire, RMB aim, R reload, 1-6 weapons");
    } else if args.connect.is_some() {
        println!("WASD move, Space jump/stand, C crouch, Ctrl prone, Q/E lean, Shift melee, F use");
        println!("LMB fire, RMB aim, R reload, 1-4 weapon slots, wheel next/prev weapon");
        println!("M opens the script menu; 0-9 or arrows + Enter pick, Esc closes");
    } else if args.map.is_some() {
        println!("WASD + Space/Ctrl fly, Shift boost, scroll changes speed");
    }
    println!("` opens the console");

    fx::registry::init(&fs);

    let console = console::Console::new(&fs);
    // Retail keeps the favourites beside CoDMP.exe; vcod shares the file.
    let ui = new_ui(&fs, &args.game_dir, &args.mod_dir);
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        title,
        world,
        fs,
        game_dir: args.game_dir.clone(),
        mod_dir: args.mod_dir.clone(),
        fs_game: None,
        fs_pure: None,
        user_fs_game: None,
        connect_addr: args.connect.clone(),
        team: args.team.clone(),
        weapon: args.weapon.clone(),
        console,
        shell,
        config_path,
        grab_before_console: false,
        mode,
        viewmodel,
        input: InputState::default(),
        window: None,
        renderer: None,
        grabbed: false,
        fx: fx::sim::FxSystem::new(),
        start: Instant::now(),
        last_frame: Instant::now(),
        debug_overlay: args.debug_overlay,
        cull_mode: renderer::CullMode::On,
        hud_stats: hud_text::HudStats::new(),
        interp_misses: 0,
        ev_seen: 0,
        ev_unknown: 0,
        build_ms: 0.0,
        render_ms: 0.0,
        fx_ms: 0.0,
        look_zoom: (1.0, false),
        ignore_hw_gamma: false,
        hud,
        hud_ms: 0.0,
        localized,
        menus: hud::menu::MenuCache::default(),
        audio,
        quick_chat: quick_chat::QuickChat::new(0x51ee),
        ui,
        ui_pending: Vec::new(),
        exec_depth: 0,
        error: None,
    };
    if matches!(app.mode, Mode::Idle) {
        app.enter_menu(None);
    }
    event_loop.run_app(&mut app)?;
    // Retail's quit goes through `CL_Disconnect`. Without the `disconnect`
    // the server holds the slot, and anything it owns, until `sv_timeout`.
    if let Mode::Online { net, .. } = &mut app.mode {
        net.disconnect();
    }
    match app.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// vcod's config, beside retail's `config_mp.cfg` in the mod directory and
/// written the same way (`Shell::config_text`).
const CONFIG_FILE: &str = "vcod_mp.cfg";

/// `r_gamma` in retail's 0.5..3 range; an out-of-range value is written
/// back to the cvar, as retail's ramp rebuild does (docs/research/cod11-gamma.md).
fn gamma_cvar(shell: &mut console::shell::Shell) -> f32 {
    let g = shell.cvar_f32("r_gamma");
    if g < gamma::GAMMA_MIN {
        shell.execute("set r_gamma 0.5");
        gamma::GAMMA_MIN
    } else if g > gamma::GAMMA_MAX {
        shell.execute("set r_gamma 3.0");
        gamma::GAMMA_MAX
    } else {
        g
    }
}

/// `r_ignorehwgamma`, latched: with 1 retail has no device ramp and bakes
/// `r_gamma` into textures as they load (docs/research/cod11-gamma.md).
fn ignore_hw_gamma(shell: &console::shell::Shell) -> bool {
    shell.cvar_f32("r_ignorehwgamma") != 0.0
}

/// The texture gamma the next world load bakes in: `r_gamma` of that
/// moment with `r_ignorehwgamma 1`, none with the device ramp.
fn image_gamma(shell: &mut console::shell::Shell, ignore_hw: bool) -> Option<f32> {
    ignore_hw.then(|| gamma_cvar(shell))
}

/// `r_mode`'s size (Q3's mode table, which the stock video mode list picks
/// from; -1 keeps the window's own) and whether `r_fullscreen` is on.
fn video_mode(shell: &console::shell::Shell) -> (Option<winit::dpi::PhysicalSize<u32>>, bool) {
    let size = match shell.cvar_f32("r_mode") as i32 {
        0 => Some((320, 240)),
        1 => Some((400, 300)),
        2 => Some((512, 384)),
        3 => Some((640, 480)),
        4 => Some((800, 600)),
        5 => Some((960, 720)),
        6 => Some((1024, 768)),
        7 => Some((1152, 864)),
        8 => Some((1280, 1024)),
        9 => Some((1600, 1200)),
        10 => Some((2048, 1536)),
        11 => Some((856, 480)),
        _ => None,
    };
    let size = size.map(|(w, h)| winit::dpi::PhysicalSize::new(w, h));
    (size, shell.cvar_f32("r_fullscreen") != 0.0)
}

/// A client joining `net` through the stock menus, answering them with
/// `team` and `weapon` when given.
fn online_mode(
    net: net::NetClient<net::UdpTransport>,
    team: Option<String>,
    weapon: Option<String>,
) -> Mode {
    Mode::Online {
        net: Box::new(net),
        cam: FlyCamera::new(Vec3::ZERO, 0.0),
        input: Box::default(),
        clock: play::cmds::CmdClock::default(),
        ring: play::cmds::CmdRing::default(),
        predictor: Box::default(),
        view: Box::default(),
        phase: Phase::Connecting,
        join: Box::new(play::join::Join::new(team, weapon)),
        menu_view: None,
    }
}

/// The camera's (yaw, pitch, roll) in radians for our own playerstate: the
/// raw cmd angles plus `delta_angles`, which is the view the server builds
/// (docs/protocol-1.1.md, "View angles"). Wire pitch is down-positive, the
/// camera's up-positive; roll keeps the wire's sign (positive tilts the head
/// right). Pitch is drawn clamped at retail's limit; the cmd still carries
/// the raw angle.
fn own_view(raw: [i32; 3], delta: [i32; 3]) -> (f32, f32, f32) {
    let short = |i: usize| raw[i].wrapping_add(delta[i]) as i16;
    let deg = |s: i16| s as f32 * 360.0 / 65536.0;
    let pitch = short(0).clamp(-PITCH_CLAMP_SHORT, PITCH_CLAMP_SHORT);
    (
        deg(short(1)).to_radians(),
        -deg(pitch).to_radians(),
        deg(short(2)).to_radians(),
    )
}

/// Retail's view pitch clamp, 87.9 degrees in short units
/// (docs/research/cod11-gsc-object-model.md, the defender fixture's
/// `viewangles[0]`).
const PITCH_CLAMP_SHORT: i16 = 16000;

/// Loads every viewmodel rig configstring 8 registers and uploads its
/// models, at the map load and on each CS 8 change; rigs already cached
/// cost a lookup.
fn prewarm_viewmodels(
    view: &mut play::view::OnlineView,
    r: &mut renderer::Renderer,
    fs: &Pk3Fs,
    configstrings: &[String],
) {
    let t0 = Instant::now();
    let rigs = view.prewarm(fs, configstrings);
    for models in &rigs {
        r.preload_viewmodel(fs, models);
    }
    log::info!(
        "viewmodels: {} rigs preloaded in {:.0} ms",
        rigs.len(),
        t0.elapsed().as_secs_f64() * 1000.0
    );
}

/// Synthetic muzzle for playerState-ring fire events (`entity_num ==
/// u32::MAX` in `net::events`): the ridden body is `skip_num`, never drawn,
/// so it has no real muzzle. 20 forward, 4 right, 3 down of the camera,
/// aimed down the view forward. Same shape as `BuiltScene::muzzles` entries.
fn view_muzzle(cam_pos: Vec3, forward: Vec3, right: Vec3, up: Vec3) -> (Vec3, Vec3) {
    (cam_pos + forward * 20.0 + right * 4.0 - up * 3.0, forward)
}

/// Leave the live map and enter Loading for the server's new map: drop the
/// GPU world and per-map state, reset what a gamestate invalidates, and list
/// the paks the client is missing.
#[allow(clippy::too_many_arguments)]
fn start_loading(
    net: &net::NetClient<net::UdpTransport>,
    hud: &mut Option<hud::Hud>,
    audio: &mut audio::AudioSystem,
    fx: &mut fx::sim::FxSystem,
    world: &mut Option<World>,
    r: &mut Renderer,
    window: Option<&Window>,
    title: &mut String,
    game_dir: &std::path::Path,
    dirs: &[&str],
    allow_download: bool,
) -> Result<Phase> {
    let map = net::info_value_for_key(net.configstring(0), "mapname")
        .map(str::to_string)
        .context("the server sent no mapname in its serverinfo")?;
    r.unload_world();
    *world = None;
    if let Some(hud) = hud {
        hud.on_gamestate();
    }
    audio.on_gamestate(&map);
    fx.clear();
    *title = format!("vcod — loading {map}");
    if let Some(window) = window {
        window.set_title(title);
    }
    // Same candidate order as the old blocking downloader; `MapLoader` caps it.
    let systeminfo = net.configstring(1).to_string();
    let exists = |rel: &std::path::Path| game_dir.join(rel).exists();
    let dir_paths: Vec<std::path::PathBuf> = dirs.iter().map(|d| game_dir.join(d)).collect();
    let local: Vec<i32> =
        vcod_common::pk3::search_paks(&dir_paths.iter().map(|p| p.as_path()).collect::<Vec<_>>())
            .iter()
            .map(|p| p.checksum)
            .collect();
    let have = |c: i32| local.contains(&c);
    let candidates = if allow_download {
        net::download::candidates_for_map(&systeminfo, &map, dirs, have, exists)
            .into_iter()
            .map(|(name, rel)| (format!("{name}.pk3"), game_dir.join(rel)))
            .collect()
    } else {
        // `CL_InitDownloads` with autodownload off: warn, then load what is
        // there (docs/research/cod11-front-end.md, section 16).
        let missing = net::download::missing_paks(&systeminfo, dirs, have, exists);
        if !missing.is_empty() {
            log::warn!(
                "You are missing some files referenced by the server:\n{}\n\
                 You might not be able to join the game (cl_allowDownload is 0)",
                missing.join("\n")
            );
        }
        Vec::new()
    };
    Ok(Phase::Loading {
        loader: loading::MapLoader::new(map, candidates),
    })
}

/// The front end off `fs`, with the favourites file and the Mods menu's
/// directory listing under `game_dir`.
fn new_ui(fs: &Pk3Fs, game_dir: &std::path::Path, mod_dir: &str) -> frontend::Ui {
    frontend::Ui::new(fs)
        .with_server_cache(game_dir.join("servercache.dat"))
        .with_mod_root(game_dir.to_path_buf(), mod_dir.to_string())
}

/// The search path's directories: `<game_dir>/<mod_dir>`, then the
/// connection's `fs_game` directory over it.
fn search_dirs(
    game_dir: &std::path::Path,
    mod_dir: &str,
    fs_game: &Option<String>,
) -> (std::path::PathBuf, Option<std::path::PathBuf>) {
    (
        game_dir.join(mod_dir),
        fs_game.as_ref().map(|g| game_dir.join(g)),
    )
}

/// The directories a download may land in, the same two.
fn download_dirs<'a>(mod_dir: &'a str, fs_game: &'a Option<String>) -> Vec<&'a str> {
    std::iter::once(mod_dir).chain(fs_game.as_deref()).collect()
}

/// `cl_allowDownload`: the systeminfo's value when the server sets one
/// (`CL_SystemInfoChanged` copies every systeminfo key into a cvar), the
/// client's own otherwise.
fn allow_download(shell: &console::shell::Shell, systeminfo: &str) -> bool {
    net::download::server_allows_download(systeminfo)
        .unwrap_or_else(|| shell.cvar_f32("cl_allowDownload") != 0.0)
}

/// Reopens the pk3 search path after a download. Everything read off the old
/// one is read again or dropped to reload lazily, since a new pak may carry
/// files the old ones lacked or replace one under the same name. The GPU
/// half is `Renderer::reopen`, the viewmodel rig `OnlineView::reopen`.
#[allow(clippy::too_many_arguments)]
fn reopen_fs(
    base: &std::path::Path,
    game: Option<&std::path::Path>,
    pure: Option<&[i32]>,
    fs: &mut Pk3Fs,
    localized: &mut vcod_common::localize::Localized,
    menus: &mut hud::menu::MenuCache,
    hud: &mut Option<hud::Hud>,
    audio: &mut audio::AudioSystem,
    fx: &mut fx::sim::FxSystem,
    quick_chat: &mut quick_chat::QuickChat,
) -> Result<()> {
    *fs = Pk3Fs::open_search(base, game, pure)?;
    *localized = vcod_common::localize::Localized::load(fs);
    *menus = hud::menu::MenuCache::default();
    match hud {
        Some(h) => {
            if let Err(e) = h.reopen(fs) {
                log::warn!("hud: {e}, keeping the fonts already loaded");
            }
        }
        None => *hud = hud::Hud::new(fs).ok(),
    }
    audio.reopen(fs);
    fx.reopen();
    fx::registry::init(fs);
    quick_chat.reopen();
    Ok(())
}

/// Parse the map, upload it to the GPU and hand back the live phase.
#[allow(clippy::too_many_arguments)]
fn load_map(
    map: &str,
    net: &net::NetClient<net::UdpTransport>,
    fs: &mut Pk3Fs,
    audio: &mut audio::AudioSystem,
    world: &mut Option<World>,
    r: &mut Renderer,
    window: Option<&Window>,
    title: &mut String,
) -> Result<Phase> {
    let Some(path) = fs.resolve_map(map) else {
        bail!("map '{map}' did not resolve in the pk3 search path");
    };
    let data = fs
        .read(&path)
        .with_context(|| format!("cannot read {path}"))?;
    let bsp = bsp::parse(&data).with_context(|| format!("cannot parse {path}"))?;
    // The ambient rides configstring 3 (docs/research/cod11-sound-system.md,
    // section 9); a fresh gamestate may have changed it.
    audio.set_ambient(fs, net::info_value_for_key(net.configstring(3), "n"));
    *title = format!("vcod — {map}");
    if let Some(window) = window {
        window.set_title(title);
    }
    let phase = live_phase(fs, &bsp, net);
    r.load_world(&bsp, fs)?;
    *world = Some(World { bsp });
    Ok(phase)
}

/// The between-maps frame: the status text over an empty view, with chat
/// still flowing through the normal HUD build (no snapshot clients).
#[allow(clippy::too_many_arguments)]
fn loading_frame(
    r: &mut Renderer,
    fs: &Pk3Fs,
    localized: &vcod_common::localize::Localized,
    hud: &mut Option<hud::Hud>,
    now: f32,
    aspect: f32,
    cull: renderer::CullMode,
    configstrings: &[String],
    text: String,
    hud_quads: &mut Vec<hud::HudQuad>,
) -> renderer::Frame {
    if let Some(hud) = hud {
        let (screen_w, screen_h) = r.screen_size();
        let no_clients = BTreeMap::new();
        let f = hud::HudFrame {
            now,
            screen_w,
            screen_h,
            configstrings,
            clients: &no_clients,
            protocol: &net::protocol::PROTOCOL_V1,
            server_time: 0,
            snap_time: 0,
            fs,
            menu: None,
            ps: None,
            entities: &BTreeMap::new(),
            predicted: None,
            local_player: false,
            weapons: &[],
            localized,
            view_yaw: 0.0,
            eye: [0.0; 3],
            fov: camera::DEFAULT_FOV_DEG,
            entity_origin: &|_| None,
            turret_weapon: None,
            cvar: &|_| None,
            bound_key: &|_| None,
            draw: hud::DrawToggles::default(),
            weapon_select: None,
        };
        *hud_quads = hud.build(&f);
    }
    renderer::Frame {
        // Nothing is drawn, so any projection works.
        view_proj: camera::view_proj_from(
            Vec3::ZERO,
            0.0,
            0.0,
            0.0,
            camera::DEFAULT_FOV_DEG,
            aspect,
        ),
        eye: Vec3::ZERO,
        fwd: camera::basis(0.0, 0.0).0,
        up: Vec3::Z,
        time: now,
        cull,
        hud_lines: vec![text],
    }
}

/// `size == 0` means no block has named the pak's size yet.
fn loading_text(map: &str, progress: Option<loading::Progress>) -> String {
    match progress {
        None => format!("Loading {map}"),
        Some(p) => {
            let size_kb = if p.size == 0 {
                "?".to_string()
            } else {
                format!("{}", p.size as u64 / 1024)
            };
            format!(
                "Downloading {map}: pak {}/{}  {} / {} KB",
                p.pak,
                p.paks,
                p.received / 1024,
                size_kb
            )
        }
    }
}

fn walk_mode(
    map: &str,
    bsp: &bsp::Bsp,
    fs: &Pk3Fs,
    view_weapon: Option<Box<viewmodel::ViewWeapon>>,
    rigs: viewmodel::RigCache,
) -> Result<Mode> {
    let Some((origin, yaw)) = bsp::find_spawn(&bsp.entities) else {
        bail!("map {map} has no player spawn; run without --walk to fly");
    };
    let world = collision::CollisionWorld::build(bsp, &props::collision_tris(fs, &bsp.entities));
    if world.brushes.is_empty() && world.tris.is_empty() {
        bail!("no collidable geometry in {map}; run without --walk to fly");
    }

    let mut ps = pmove::PlayerState::spawn(Vec3::from(origin) + Vec3::Z * 2.0, yaw);
    let drop = world.box_trace(
        ps.origin,
        ps.origin - Vec3::Z * 4096.0,
        ps.mins(),
        ps.maxs(),
    );
    if drop.startsolid {
        // CoD spawns anyway; pmove usually pushes out of solid in the first frames.
        log::warn!("spawn point is inside solid geometry, spawning there anyway");
    } else if drop.fraction < 1.0 {
        ps.origin = drop.endpos;
    } else {
        log::warn!("no ground within 4096 units below the spawn point");
    }

    let mut configstrings = vec![String::new(); 8];
    // `entityState.weapon` is 1-based into CS 7; walk mode carries the loadout.
    configstrings[7] = WALK_LOADOUT.join(" ");
    let start_slot = WALK_LOADOUT
        .iter()
        .position(|&w| w == "kar98k_mp")
        .unwrap_or(0);
    let reserve = view_weapon.as_ref().map_or(0, |w| w.def.start_ammo);

    Ok(Mode::Walk {
        world: Box::new(world),
        ps: Box::new(ps),
        input: pmove::PmInput::default(),
        keys: WalkKeys::default(),
        motion: viewmodel::ViewmodelMotion::new(),
        view_weapon,
        configstrings,
        weapon_slot: start_slot,
        reserve,
        switch_to: None,
        rigs,
        mouse_delta: (0.0, 0.0),
        fire_edge: false,
        fire_held: false,
        reload_edge: false,
        ads_held: false,
        gun_aim: hud::scope::GunAim::default(),
    })
}

struct App {
    title: String,
    world: Option<World>,
    fs: Pk3Fs,
    /// Where downloads land and the pk3 path reopens from.
    game_dir: std::path::PathBuf,
    mod_dir: String,
    /// The connection's systeminfo `fs_game`, layered over `mod_dir`.
    fs_game: Option<String>,
    /// A pure server's `sv_paks`: only paks with these checksums load.
    fs_pure: Option<Vec<i32>>,
    /// The mod the Mods menu picked, the search path between servers.
    user_fs_game: Option<String>,
    /// The last server connected to: the connecting screen's text and what
    /// `reconnect` reconnects to.
    connect_addr: Option<String>,
    /// `--team` / `--weapon`, the stock menu answers for every connect.
    team: Option<String>,
    weapon: Option<String>,
    console: console::Console,
    shell: console::shell::Shell,
    config_path: std::path::PathBuf,
    /// Whether the mouse was captured when the console opened; closing it
    /// captures it again.
    grab_before_console: bool,
    mode: Mode,
    viewmodel: Arc<[xmodel::XModel]>,
    /// Fly-mode keys; walk keeps its own in `Mode::Walk`.
    input: InputState,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    grabbed: bool,
    fx: fx::sim::FxSystem,
    /// Origin of the clock effects are timed against.
    start: Instant,
    last_frame: Instant,
    debug_overlay: bool,
    cull_mode: renderer::CullMode,
    hud_stats: hud_text::HudStats,
    /// Online frames without a straddling snapshot pair. Cumulative; the
    /// overlay shows the rate.
    interp_misses: u64,
    /// Cumulative events drained, and those with an unrecognized code.
    ev_seen: u64,
    ev_unknown: u64,
    build_ms: f32,
    render_ms: f32,
    fx_ms: f32,
    /// Last frame's fov over `cg_fov` and whether the view rides a mounted
    /// gun: the mouse's sensitivity scale ([`play::input::MouseLook`]).
    look_zoom: (f32, bool),
    /// `r_ignorehwgamma` as of the last window start or `vid_restart`
    /// (retail latches it): textures bake `r_gamma` at each world load and
    /// the frame pass only clamps.
    ignore_hw_gamma: bool,
    hud: Option<hud::Hud>,
    hud_ms: f32,
    /// Menu labels; empty outside `--connect`.
    localized: vcod_common::localize::Localized,
    menus: hud::menu::MenuCache,
    audio: audio::AudioSystem,
    quick_chat: quick_chat::QuickChat,
    /// The main menu and server browser, up while no game is.
    ui: frontend::Ui,
    /// Menu effects queued where no event loop is at hand; `about_to_wait`
    /// runs them.
    ui_pending: Vec<frontend::UiEffect>,
    /// Nested `exec`s running.
    exec_depth: u8,
    error: Option<anyhow::Error>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: anyhow::Error) {
        self.error = Some(err);
        event_loop.exit();
    }

    fn set_grab(&mut self, grabbed: bool) {
        let Some(window) = &self.window else { return };
        if grabbed {
            if window.set_cursor_grab(CursorGrabMode::Locked).is_err()
                && window.set_cursor_grab(CursorGrabMode::Confined).is_err()
            {
                log::warn!("this platform does not support grabbing the cursor");
                return;
            }
        } else if window.set_cursor_grab(CursorGrabMode::None).is_err() {
            return;
        }
        window.set_cursor_visible(!grabbed);
        self.grabbed = grabbed;
    }

    /// On grab release or focus loss, so no held key stays latched. Prone
    /// and the `--connect` stance stay, neither is a held key. The scoreboard drops, Tab is held too.
    fn clear_held_keys(&mut self) {
        match &mut self.mode {
            Mode::Idle => {}
            Mode::Fly(_) => self.input = InputState::default(),
            Mode::Online { input, .. } => input.release_all(),
            Mode::Walk {
                input,
                keys,
                fire_edge,
                fire_held,
                reload_edge,
                ads_held,
                ..
            } => {
                *keys = WalkKeys::default();
                input.jump = false;
                input.crouch = false;
                input.walk_slow = false;
                input.lean_left = false;
                input.lean_right = false;
                *fire_edge = false;
                *fire_held = false;
                *reload_edge = false;
                *ads_held = false;
            }
        }
        if let Some(hud) = &mut self.hud {
            hud.scoreboard.visible = false;
        }
    }

    /// The chat field's keys while it is open, online only. Enter sends
    /// `say "<line>"` the way the console's field does (CoDMP.exe 0x40d40b);
    /// Escape drops the line. False when the key is not the field's.
    fn chat_key(&mut self, code: KeyCode, text: Option<&str>) -> bool {
        let Mode::Online { net, .. } = &mut self.mode else {
            return false;
        };
        let Some(hud) = &mut self.hud else {
            return false;
        };
        let Some(field) = &mut hud.chat_field else {
            return false;
        };
        match code {
            KeyCode::Escape => hud.chat_field = None,
            KeyCode::Enter | KeyCode::NumpadEnter => {
                if !field.text.is_empty() {
                    let word = if field.team { "say_team" } else { "say" };
                    net.send_reliable(&format!("{word} \"{}\"", field.text));
                }
                hud.chat_field = None;
            }
            KeyCode::Backspace => {
                field.text.pop();
            }
            _ => {
                // The field is 256 bytes; a quote would end the argument.
                for c in text.unwrap_or("").chars() {
                    if (' '..='~').contains(&c) && c != '"' && field.text.len() < 255 {
                        field.text.push(c);
                    }
                }
            }
        }
        true
    }

    /// Whether keys go to the console: it is down, or there is nothing else.
    fn console_active(&self) -> bool {
        self.console.open || (matches!(self.mode, Mode::Idle) && !self.ui.active())
    }

    /// Whether the front end takes the mouse and keys: with no game up, or
    /// as the in-game main menu.
    fn menu_active(&self) -> bool {
        !self.console.open && self.ui.active()
    }

    /// The main menu, or the error popup over it when `error` says why the
    /// game ended. Its music and clicks use the `menu` loadspec's aliases.
    fn enter_menu(&mut self, error: Option<&str>) {
        self.audio.on_gamestate("menu");
        let mut out = Vec::new();
        match error {
            Some(e) => self.ui.show_error(e, &mut out),
            None => self.ui.open_main(&mut out),
        }
        self.ui_pending.extend(out);
    }

    /// Carries out what a menu script asked for.
    fn ui_effects(&mut self, event_loop: &ActiveEventLoop, effects: Vec<frontend::UiEffect>) {
        use frontend::UiEffect;
        for effect in effects {
            match effect {
                UiEffect::Command(line) => {
                    let effects = self.shell.execute(&line);
                    self.apply(event_loop, effects);
                }
                UiEffect::Sound(alias) => self.audio.play_local(&self.fs, &alias),
                UiEffect::RunMod(game) => self.run_mod(game),
                UiEffect::ExecOnCvar {
                    cvar,
                    value,
                    int,
                    command,
                } => {
                    let cur = self.shell.cvar(&cvar).unwrap_or("");
                    if UiEffect::cvar_matches(cur, value, int) {
                        let effects = self.shell.execute(&command);
                        self.apply(event_loop, effects);
                    }
                }
            }
        }
    }

    /// Opening the console lets go of the game's keys and the mouse, as
    /// retail's key catcher does; closing it captures the mouse again if it
    /// was captured before.
    fn toggle_console(&mut self) {
        self.console.toggle();
        if self.console.open {
            self.grab_before_console = self.grabbed;
            self.set_grab(false);
            self.clear_held_keys();
        } else if self.grab_before_console && !self.ui.active() {
            self.set_grab(true);
        }
    }

    /// A line typed into the console.
    fn console_line(&mut self, event_loop: &ActiveEventLoop, line: &str) {
        let in_game = matches!(
            &self.mode,
            Mode::Online { net, .. } if net.state() == net::NetState::Active
        );
        if let Some(cmd) = console::command_for(line, in_game) {
            let effects = self.shell.execute(&cmd);
            self.apply(event_loop, effects);
        }
    }

    /// A bound key or button's edge, online only.
    fn bound_key(&mut self, event_loop: &ActiveEventLoop, key: &str, pressed: bool) {
        // A gameplay press counts only while the mouse is captured; a
        // release always passes so nothing stays held.
        if pressed && !self.grabbed && self.shell.bind_is_gameplay(key) {
            return;
        }
        let effects = self.shell.key_event(key, pressed);
        self.apply(event_loop, effects);
    }

    /// Carries out what the console's commands asked for.
    fn apply(&mut self, event_loop: &ActiveEventLoop, effects: Vec<console::shell::Effect>) {
        use console::shell::Effect;
        for effect in effects {
            match effect {
                Effect::Print(line) => console::log::print(&line),
                Effect::Clear => self.console.clear(),
                Effect::Connect(addr) => self.connect(addr),
                Effect::Reconnect => match self.connect_addr.clone() {
                    Some(addr) => self.connect(addr),
                    None => console::log::print("Not connected to a server."),
                },
                Effect::Disconnect => {
                    if matches!(self.mode, Mode::Online { .. }) {
                        self.disconnect(None);
                    } else {
                        console::log::print("Not connected to a server.");
                    }
                }
                Effect::Quit => event_loop.exit(),
                Effect::Forward(line) => match &mut self.mode {
                    Mode::Online { net, .. } if net.state() != net::NetState::Disconnected => {
                        net.send_reliable(&line)
                    }
                    _ => console::log::print("Not connected to a server."),
                },
                // `CL_ForwardCommandToServer`: a key-up command goes nowhere,
                // a `+` one or one with no server is unknown.
                Effect::Unknown { word, line } => match &mut self.mode {
                    _ if word.starts_with('-') => {}
                    Mode::Online { net, .. }
                        if net.state() != net::NetState::Disconnected && !word.starts_with('+') =>
                    {
                        net.send_reliable(&line)
                    }
                    _ => console::log::print(&format!("Unknown command \"{word}\"")),
                },
                Effect::Button(action, down) => {
                    if let Mode::Online { input, .. } = &mut self.mode {
                        input.key(action, down);
                    }
                }
                Effect::Impulse(action) => {
                    if let Mode::Online { input, .. } = &mut self.mode {
                        let select = matches!(
                            action,
                            play::input::Action::Slot(_)
                                | play::input::Action::NextWeapon
                                | play::input::Action::PrevWeapon
                        );
                        // `cg_weaponCycleDelay` (cgame 0x30038096): a select
                        // inside the delay from the name's stamp does nothing.
                        let now = (Instant::now() - self.start).as_secs_f32();
                        let delay = self.shell.cvar_f32("cg_weaponCycleDelay") as i32;
                        if select
                            && let Some(hud) = &self.hud
                            && !hud.weapon_cycle_allowed(now, delay)
                        {
                            continue;
                        }
                        input.key(action, true);
                        input.key(action, false);
                        if let (true, Some(hud)) = (select, &mut self.hud) {
                            hud.weapon_selected();
                        }
                    }
                }
                // The server never pushes scores: send `score` on the down
                // edge, and every 2 s while held (see the redraw tick;
                // docs/research/cod11-hud-protocol.md, section 4).
                Effect::Scores(down) => {
                    if let (Mode::Online { net, .. }, Some(hud)) = (&mut self.mode, &mut self.hud) {
                        hud.scoreboard.visible = down;
                        if down {
                            let now = (Instant::now() - self.start).as_secs_f32();
                            net.send_reliable("score");
                            hud.scoreboard.mark_requested(now);
                        }
                    }
                }
                Effect::MessageMode { team } => {
                    if let (Mode::Online { input, .. }, Some(hud)) = (&mut self.mode, &mut self.hud)
                    {
                        input.release_all();
                        hud.chat_field = Some(hud::ChatField {
                            team,
                            text: String::new(),
                        });
                    }
                }
                Effect::ToggleConsole => self.toggle_console(),
                Effect::Userinfo => {
                    if let Mode::Online { net, .. } = &mut self.mode {
                        net.set_userinfo(self.shell.userinfo());
                    }
                }
                Effect::SaveConfig => {
                    if let Err(e) = std::fs::write(&self.config_path, self.shell.config_text()) {
                        log::warn!("cannot write {}: {e}", self.config_path.display());
                    }
                }
                Effect::Exec(file) => self.exec_file(event_loop, &file),
                Effect::VidRestart => {
                    self.ignore_hw_gamma = ignore_hw_gamma(&self.shell);
                    if let Some(w) = &self.window {
                        let (size, fullscreen) = video_mode(&self.shell);
                        w.set_fullscreen(
                            fullscreen.then_some(winit::window::Fullscreen::Borderless(None)),
                        );
                        if let Some(size) = size {
                            let _ = w.request_inner_size(size);
                        }
                    }
                }
            }
        }
    }

    /// `exec <file>`: the file from the paks (`default_mp.cfg` lives in
    /// `localized_english_pak0.pk3`) or else beside the config, run as
    /// console text. A file that execs itself stops at a fixed depth.
    fn exec_file(&mut self, event_loop: &ActiveEventLoop, file: &str) {
        let text = self.fs.read(file).or_else(|| {
            let dir = self.config_path.parent()?;
            std::fs::read(dir.join(file)).ok()
        });
        let Some(text) = text else {
            console::log::print(&format!("couldn't exec {file}"));
            return;
        };
        if self.exec_depth >= 8 {
            log::warn!("exec {file}: too deep, skipped");
            return;
        }
        console::log::print(&format!("execing {file}"));
        self.exec_depth += 1;
        let effects = self.shell.execute(&String::from_utf8_lossy(&text));
        self.apply(event_loop, effects);
        self.exec_depth -= 1;
    }

    /// `connect`: leaves any server first, then joins `addr` the way
    /// `--connect` does. Like retail's `CL_Connect_f`, it closes the console.
    fn connect(&mut self, addr: String) {
        if matches!(self.mode, Mode::Online { .. }) {
            self.disconnect(None);
        }
        let mut net = match net::NetClient::connect(&addr) {
            Ok(net) => net,
            Err(e) => {
                log::error!("cannot open a socket to {addr}: {e:#}");
                return;
            }
        };
        net.set_userinfo(self.shell.userinfo());
        log::info!("connecting to {addr}");
        if self.hud.is_none() {
            self.hud = hud::Hud::new(&self.fs)
                .map_err(|e| log::warn!("hud: {e}, disabling the on-screen HUD"))
                .ok();
            self.localized = vcod_common::localize::Localized::load(&self.fs);
        }
        self.unload_world();
        self.ui.close_all();
        self.mode = online_mode(net, self.team.clone(), self.weapon.clone());
        self.connect_addr = Some(addr);
        self.set_title("vcod — connecting".to_string());
        if self.console.open {
            self.toggle_console();
        }
    }

    /// Leaves the server (`CL_Disconnect`) for the main menu, with
    /// `reason` in the error popup when the server or the load is what
    /// ended it. `Com_Error` localizes the reason as a message before it
    /// reaches `com_errorMessage` (CoDMP.exe 0x435a40,
    /// docs/research/cod11-front-end.md section 15).
    fn disconnect(&mut self, reason: Option<String>) {
        if let Mode::Online { net, .. } = &mut self.mode {
            net.disconnect();
        }
        let reason = reason.map(|r| self.localized.message(&r));
        if let Some(reason) = &reason {
            log::error!("{reason}");
        }
        self.unload_world();
        self.leave_fs_game();
        self.mode = Mode::Idle;
        self.set_grab(false);
        self.set_title("vcod".to_string());
        self.enter_menu(reason.as_deref());
    }

    /// Back to the player's own search path after a server's `fs_game` or
    /// pure list. Retail keeps a server's mod until the next systeminfo names
    /// another; vcod drops it with the connection
    /// (docs/research/cod11-front-end.md, section 16).
    fn leave_fs_game(&mut self) {
        self.switch_fs_game(self.user_fs_game.clone());
    }

    /// Reopens the search path over `game` (no pure list) and rebuilds the
    /// front end off it, as retail's `FS_Restart` and UI reload do. Nothing
    /// happens when that is the search path already.
    fn switch_fs_game(&mut self, game: Option<String>) {
        let pure = self.fs_pure.take();
        if self.fs_game == game && pure.is_none() {
            return;
        }
        self.fs_game = game;
        let (base, game) = search_dirs(&self.game_dir, &self.mod_dir, &self.fs_game);
        match reopen_fs(
            &base,
            game.as_deref(),
            None,
            &mut self.fs,
            &mut self.localized,
            &mut self.menus,
            &mut self.hud,
            &mut self.audio,
            &mut self.fx,
            &mut self.quick_chat,
        ) {
            Ok(()) => {
                if let Some(r) = &mut self.renderer {
                    r.reopen(&self.fs);
                }
                self.ui = new_ui(&self.fs, &self.game_dir, &self.mod_dir);
            }
            Err(e) => log::error!("cannot reopen {}: {e:#}", base.display()),
        }
    }

    /// The Mods menu's `RunMod` (and `Quake3` with `None`): `fs_game` and a
    /// `vid_restart`, which brings the main menu back up off the mod's
    /// paks. In a game, `vid_restart` keeps the connection: the search path
    /// reopens under the server's pure list, the UI reloads closed and the
    /// cgame half sends `cp` again (CoDMP.exe 0x40fbe0,
    /// docs/research/cod11-front-end.md section 17).
    fn run_mod(&mut self, game: Option<String>) {
        log::info!("fs_game {}", game.as_deref().unwrap_or("(none)"));
        self.user_fs_game = game.clone();
        if !matches!(self.mode, Mode::Online { .. }) {
            self.switch_fs_game(game);
            self.enter_menu(None);
            return;
        }
        if game != self.fs_game {
            self.fs_game = game;
            let (base, game) = search_dirs(&self.game_dir, &self.mod_dir, &self.fs_game);
            if let Err(e) = reopen_fs(
                &base,
                game.as_deref(),
                self.fs_pure.as_deref(),
                &mut self.fs,
                &mut self.localized,
                &mut self.menus,
                &mut self.hud,
                &mut self.audio,
                &mut self.fx,
                &mut self.quick_chat,
            ) {
                self.disconnect(Some(format!("cannot reopen {}: {e:#}", base.display())));
                return;
            }
            if let Some(r) = &mut self.renderer {
                r.reopen(&self.fs);
            }
        }
        self.ui = new_ui(&self.fs, &self.game_dir, &self.mod_dir);
        self.after_menu();
        if let Mode::Online {
            net,
            view,
            phase,
            menu_view,
            ..
        } = &mut self.mode
        {
            *menu_view = None;
            view.reopen();
            // `CL_InitCGame` and `CL_SendPureChecksums` run only past
            // `CA_LOADING`; before that the load sends `cp` itself.
            if matches!(phase, Phase::Live(_)) {
                let feed = net.gamestate().map_or(0, |g| g.checksum_feed);
                net.send_reliable(&self.fs.pure_command(feed));
            }
        }
    }

    /// Drops the map, its sounds and effects, between servers.
    fn unload_world(&mut self) {
        if let Some(r) = &mut self.renderer {
            r.unload_world();
        }
        self.world = None;
        self.fx.clear();
        self.audio.on_gamestate("");
        if let Some(hud) = &mut self.hud {
            hud.on_gamestate();
            hud.chat_field = None;
            hud.scoreboard.visible = false;
        }
    }

    fn set_title(&mut self, title: String) {
        if let Some(window) = &self.window {
            window.set_title(&title);
        }
        self.title = title;
    }

    /// The open script menu's keys, ahead of every other binding, and M to
    /// open the main menu when none is.
    fn menu_key(&mut self, code: KeyCode) -> ScriptMenuKey {
        let Mode::Online {
            net,
            join,
            menu_view,
            ..
        } = &mut self.mode
        else {
            return ScriptMenuKey::Ignored;
        };
        let Some((_, view)) = menu_view else {
            if code == KeyCode::KeyM {
                join.open_main(net.configstrings());
                return ScriptMenuKey::Used;
            }
            return ScriptMenuKey::Ignored;
        };
        let response = match code {
            KeyCode::Escape => {
                join.close();
                return ScriptMenuKey::Closed;
            }
            KeyCode::ArrowUp => {
                view.up();
                return ScriptMenuKey::Used;
            }
            KeyCode::ArrowDown => {
                view.down();
                return ScriptMenuKey::Used;
            }
            // The "Main Menu" tab: `close <menu>; open main`.
            KeyCode::Enter if view.main_menu_selected() => {
                join.close();
                return ScriptMenuKey::MainMenu;
            }
            KeyCode::Enter => view.selected_response(),
            _ => match digit_key(code) {
                Some(key) => view.response_for_key(key),
                None => return ScriptMenuKey::Ignored,
            },
        };
        if let Some(cmd) = response.and_then(|r| join.choose(r, net.server_id())) {
            net.send_reliable(&cmd);
        }
        ScriptMenuKey::Used
    }

    /// Esc in a game with nothing else holding the keys, as `CL_KeyEvent`
    /// does it (docs/research/cod11-front-end.md section 13): once the game
    /// is live the `g_scriptMainMenu` script menu (vcod falls back to the
    /// in-game main menu when the server named none), before that the main
    /// menu. The game keeps running; the mouse goes to the menu.
    fn escape_in_game(&mut self) {
        let Mode::Online {
            net, join, phase, ..
        } = &mut self.mode
        else {
            return;
        };
        let live = matches!(phase, Phase::Live(_));
        if live {
            join.open_main(net.configstrings());
        }
        if !live || join.open().is_none() {
            let mut out = Vec::new();
            if live {
                self.ui.open_ingame(&mut out);
            } else {
                self.ui.open_main(&mut out);
            }
            self.ui_pending.extend(out);
        }
        self.set_grab(false);
        self.grab_before_console = false;
        self.clear_held_keys();
    }

    /// Esc outside the front end: the script menu's own Esc when one is
    /// open, else the in-game menus online and a mouse release offline.
    fn menu_key_or_escape(&mut self) {
        match self.menu_key(KeyCode::Escape) {
            ScriptMenuKey::Closed => {
                if !self.console.open {
                    self.set_grab(true);
                }
            }
            ScriptMenuKey::Ignored if matches!(self.mode, Mode::Online { .. }) => {
                self.escape_in_game()
            }
            _ => {
                self.set_grab(false);
                self.clear_held_keys();
            }
        }
    }

    /// The last menu over a game closed (Back to Game, Esc): the mouse goes
    /// back to the game.
    fn after_menu(&mut self) {
        if matches!(self.mode, Mode::Online { .. }) && !self.ui.active() && !self.console.open {
            self.set_grab(true);
        }
    }

    /// The in-game main menu, from a script menu's "Main Menu" tab.
    fn open_ingame_menu(&mut self) {
        let mut out = Vec::new();
        self.ui.open_ingame(&mut out);
        self.ui_pending.extend(out);
        self.set_grab(false);
        self.clear_held_keys();
    }
}

/// What a key did to the open script menu.
enum ScriptMenuKey {
    /// Not the menu's key.
    Ignored,
    Used,
    /// Esc closed it.
    Closed,
    /// Its "Main Menu" tab closed it for the UI's `main`.
    MainMenu,
}

/// A menu `execKey` name for the digit row.
fn digit_key(code: KeyCode) -> Option<&'static str> {
    Some(match code {
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        _ => return None,
    })
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 900.0));
        let (size, fullscreen) = video_mode(&self.shell);
        self.ignore_hw_gamma = ignore_hw_gamma(&self.shell);
        if let Some(size) = size {
            attrs = attrs.with_inner_size(size);
        }
        if fullscreen {
            attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                return self.fail(
                    event_loop,
                    anyhow::Error::new(e).context("cannot create a window"),
                );
            }
        };
        match Renderer::new(window.clone(), &self.fs) {
            Ok(mut r) => {
                r.set_image_gamma(image_gamma(&mut self.shell, self.ignore_hw_gamma));
                if let Some(w) = &self.world
                    && let Err(e) = r.load_world(&w.bsp, &self.fs)
                {
                    return self.fail(event_loop, e);
                }
                // Offline there is no snapshot to place the brush models, so
                // every one stands where the map put it.
                if let Some(w) = &self.world
                    && !matches!(self.mode, Mode::Online { .. })
                {
                    r.set_submodels(
                        &(1..w.bsp.models.len())
                            .map(|m| (m, glam::Mat4::IDENTITY))
                            .collect::<Vec<_>>(),
                    );
                }
                if !self.viewmodel.is_empty() {
                    r.set_viewmodel(&self.fs, &self.viewmodel);
                }
                self.renderer = Some(r);
            }
            Err(e) => return self.fail(event_loop, e),
        }
        window.request_redraw();
        self.window = Some(window);
        self.last_frame = Instant::now();
    }

    // The loop runs `ControlFlow::Poll`, so this sees a signal within a frame.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if quit::requested() {
            event_loop.exit();
        }
        let pending = std::mem::take(&mut self.ui_pending);
        self.ui_effects(event_loop, pending);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // ` and ~ toggle the console whatever they are bound to
                // (CoDMP.exe 0x40dc30).
                if code == KeyCode::Backquote {
                    if pressed && !event.repeat {
                        self.toggle_console();
                    }
                    return;
                }
                if self.console_active() {
                    if !pressed {
                        return;
                    }
                    match code {
                        // In a game Esc reaches the menus under the console
                        // and leaves it down (CoDMP.exe 0x40ddca, 0x4180a0).
                        KeyCode::Escape
                            if self.console.open && matches!(self.mode, Mode::Online { .. }) =>
                        {
                            if self.ui.active() {
                                let (_, effects) = self.ui.key(code);
                                self.ui_effects(event_loop, effects);
                            } else {
                                self.menu_key_or_escape();
                            }
                        }
                        KeyCode::Escape if self.console.open => self.toggle_console(),
                        _ => {
                            let shell = &self.shell;
                            if let Some(line) = self
                                .console
                                .key(code, event.text.as_deref(), |p| shell.complete(p))
                            {
                                self.console_line(event_loop, &line);
                            }
                        }
                    }
                    return;
                }
                // F3 and F4 fall through the menu; in a game nothing else
                // does, so no bind fires under it.
                if self.menu_active() {
                    if !pressed {
                        return;
                    }
                    // A bind waiting for its key or an edit field takes
                    // every key.
                    if self.ui.waiting_for_key() {
                        if let Some(key) = console::keys::key_name(code) {
                            let effects = self.ui.bind_key(key, &self.shell);
                            self.ui_effects(event_loop, effects);
                        }
                        return;
                    }
                    if self.ui.editing() {
                        let effects = self.ui.edit_key(code, event.text.as_deref(), &self.shell);
                        self.ui_effects(event_loop, effects);
                        return;
                    }
                    let (used, effects) = self.ui.key(code);
                    self.ui_effects(event_loop, effects);
                    self.after_menu();
                    if used || !matches!(code, KeyCode::F3 | KeyCode::F4) {
                        return;
                    }
                }
                if pressed && self.chat_key(code, event.text.as_deref()) {
                    return;
                }
                // auto-repeat would retrigger the jump and walk's prone toggle
                if event.repeat {
                    return;
                }
                if pressed && code == KeyCode::Escape {
                    self.menu_key_or_escape();
                    return;
                }
                if pressed {
                    match self.menu_key(code) {
                        ScriptMenuKey::Ignored => {}
                        ScriptMenuKey::MainMenu => {
                            self.open_ingame_menu();
                            return;
                        }
                        ScriptMenuKey::Used | ScriptMenuKey::Closed => return,
                    }
                }
                if code == KeyCode::F3 && pressed {
                    self.debug_overlay = !self.debug_overlay;
                    return;
                }
                if code == KeyCode::F4 && pressed {
                    self.cull_mode = self.cull_mode.next();
                    return;
                }
                if matches!(self.mode, Mode::Online { .. }) {
                    if let Some(key) = console::keys::key_name(code) {
                        self.bound_key(event_loop, key, pressed);
                    }
                    return;
                }
                let grabbed = self.grabbed;
                match &mut self.mode {
                    Mode::Idle | Mode::Online { .. } => {}
                    Mode::Fly(_) => match code {
                        KeyCode::KeyW => self.input.forward = pressed,
                        KeyCode::KeyS => self.input.back = pressed,
                        KeyCode::KeyA => self.input.left = pressed,
                        KeyCode::KeyD => self.input.right = pressed,
                        KeyCode::Space => self.input.up = pressed,
                        KeyCode::ControlLeft => self.input.down = pressed,
                        KeyCode::ShiftLeft => self.input.boost = pressed,
                        _ => {}
                    },
                    Mode::Walk {
                        input,
                        keys,
                        reload_edge,
                        switch_to,
                        ..
                    } => match code {
                        // weapon actions count only while the mouse is captured
                        KeyCode::KeyR if pressed && grabbed => *reload_edge = true,
                        KeyCode::KeyW => keys.w = pressed,
                        KeyCode::KeyS => keys.s = pressed,
                        KeyCode::KeyA => keys.a = pressed,
                        KeyCode::KeyD => keys.d = pressed,
                        KeyCode::Space => input.jump = pressed,
                        KeyCode::ControlLeft => input.crouch = pressed,
                        KeyCode::KeyZ if pressed => input.prone = !input.prone,
                        KeyCode::KeyQ => input.lean_left = pressed,
                        KeyCode::KeyE => input.lean_right = pressed,
                        KeyCode::ShiftLeft => input.walk_slow = pressed,
                        _ => {
                            if pressed
                                && grabbed
                                && let Some(slot) = digit_slot(code)
                            {
                                *switch_to = Some(slot);
                            }
                        }
                    },
                }
            }
            WindowEvent::Focused(false) => self.clear_held_keys(),
            WindowEvent::CursorMoved { position, .. } => {
                if self.menu_active()
                    && let Some(r) = &self.renderer
                {
                    let (w, h) = r.screen_size();
                    let effects =
                        self.ui
                            .mouse_move(position.x as f32, position.y as f32, w, h, &self.shell);
                    self.ui_effects(event_loop, effects);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                use winit::event::MouseButton;
                let pressed = state == ElementState::Pressed;
                if self.menu_active() {
                    if pressed
                        && self.ui.waiting_for_key()
                        && let Some(key) = console::keys::mouse_name(button)
                    {
                        let effects = self.ui.bind_key(key, &self.shell);
                        self.ui_effects(event_loop, effects);
                    } else if button == MouseButton::Left && pressed {
                        let effects = self.ui.click(Instant::now(), &self.shell);
                        self.ui_effects(event_loop, effects);
                        self.after_menu();
                    } else if button == MouseButton::Left {
                        self.ui.release();
                    }
                    return;
                }
                if self.console_active() {
                    return;
                }
                // the click that captures the mouse must not also fire
                if button == MouseButton::Left && pressed && !self.grabbed {
                    self.set_grab(true);
                    return;
                }
                if matches!(self.mode, Mode::Online { .. }) {
                    if let Some(key) = console::keys::mouse_name(button) {
                        self.bound_key(event_loop, key, pressed);
                    }
                    return;
                }
                if !self.grabbed {
                    return;
                }
                match &mut self.mode {
                    Mode::Walk {
                        fire_edge,
                        fire_held,
                        ads_held,
                        ..
                    } => match button {
                        MouseButton::Left if pressed => {
                            *fire_edge = true;
                            *fire_held = true;
                        }
                        MouseButton::Left => *fire_held = false,
                        MouseButton::Right => *ads_held = pressed,
                        _ => {}
                    },
                    Mode::Idle | Mode::Online { .. } | Mode::Fly(_) => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                };
                if self.menu_active() {
                    if scroll != 0.0 && self.ui.waiting_for_key() {
                        let key = if scroll > 0.0 {
                            "MWHEELUP"
                        } else {
                            "MWHEELDOWN"
                        };
                        let effects = self.ui.bind_key(key, &self.shell);
                        self.ui_effects(event_loop, effects);
                    } else if scroll != 0.0 {
                        self.ui.scroll(scroll > 0.0);
                    }
                    return;
                }
                if self.console_active() {
                    if scroll != 0.0 {
                        self.console.scroll(scroll > 0.0);
                    }
                    return;
                }
                let grabbed = self.grabbed;
                match &mut self.mode {
                    Mode::Fly(cam) => cam.adjust_speed(scroll),
                    Mode::Online { menu_view, .. }
                        if grabbed && menu_view.is_none() && scroll != 0.0 =>
                    {
                        let key = if scroll < 0.0 {
                            "MWHEELDOWN"
                        } else {
                            "MWHEELUP"
                        };
                        self.bound_key(event_loop, key, true);
                        self.bound_key(event_loop, key, false);
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;
                let elapsed = now - self.start;
                let time = elapsed.as_secs_f32();
                let local_ms = elapsed.as_secs_f64() * 1000.0;
                let cull = self.cull_mode;
                self.audio
                    .set_master_volume(self.shell.cvar_f32("mss_volume"));
                let gamma = gamma_cvar(&mut self.shell);
                let fullscreen = self
                    .window
                    .as_ref()
                    .is_some_and(|w| w.fullscreen().is_some());
                let overbright = gamma::overbright_bits(fullscreen, !self.ignore_hw_gamma);
                let Some(r) = &mut self.renderer else { return };
                r.set_gamma(if self.ignore_hw_gamma { 1.0 } else { gamma }, overbright);
                let aspect = r.aspect();
                // Set inside the online arm where `self` is borrowed out
                // field-by-field; acted on once the borrows end.
                let mut fatal: Option<anyhow::Error> = None;
                // What the mode draws on the 2D layer; the console goes on top.
                let mut hud_quads: Vec<hud::HudQuad> = Vec::new();
                let (mut frame, vm) = match &mut self.mode {
                    Mode::Idle => (
                        renderer::Frame {
                            // Nothing is drawn, so any projection works.
                            view_proj: camera::view_proj_from(
                                Vec3::ZERO,
                                0.0,
                                0.0,
                                0.0,
                                camera::DEFAULT_FOV_DEG,
                                aspect,
                            ),
                            eye: Vec3::ZERO,
                            fwd: camera::basis(0.0, 0.0).0,
                            up: Vec3::Z,
                            time,
                            cull,
                            hud_lines: Vec::new(),
                        },
                        None,
                    ),
                    Mode::Fly(cam) => {
                        cam.update(&self.input, dt);
                        let (cam_forward, cam_right, cam_up) = camera::basis(cam.yaw, cam.pitch);
                        // u32::MAX: no entity pinned to the camera.
                        self.audio
                            .set_listener(cam.pos, cam_forward, cam_right, cam_up, u32::MAX);
                        // no CollisionWorld in fly mode, so usePhysics fx don't collide
                        let fx_t0 = Instant::now();
                        self.fx.step(dt, time, None);
                        self.audio.step(&HashMap::new(), None);
                        r.set_fx_quads(
                            &self.fs,
                            self.fx.build_quads(cam.pos, cam_right, cam_up, time),
                        );
                        r.set_fx_lights(&self.fx.lights(cam.pos, time));
                        self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;
                        (
                            renderer::Frame {
                                view_proj: cam.view_proj(aspect),
                                eye: cam.pos,
                                fwd: cam_forward,
                                up: Vec3::Z,
                                time,
                                cull,
                                hud_lines: Vec::new(),
                            },
                            None,
                        )
                    }
                    Mode::Online {
                        net,
                        cam,
                        input,
                        clock: cmd_clock,
                        ring,
                        predictor,
                        view,
                        phase,
                        join,
                        menu_view,
                    } => {
                        let mut vm = None;
                        let events = net.pump();
                        let mut gamestate_ready = false;
                        for ev in &events {
                            if let Some(hud) = &mut self.hud {
                                hud.on_net_event(ev, time, &self.localized);
                            }
                            match ev {
                                // `player_talk` with every chat line, as
                                // `CG_ServerCommand`'s `h`/`i` play it.
                                net::NetEvent::Chat { text, .. } => {
                                    console::log::print(&self.localized.message(text));
                                    self.audio.play_local(&self.fs, "player_talk");
                                }
                                net::NetEvent::Print(s) => console::log::print(s),
                                net::NetEvent::Dropped(why) => fatal = Some(anyhow!("{why}")),
                                // Map ambient; each round restart re-sends it
                                // with a new fade deadline, which is ignored.
                                net::NetEvent::ConfigstringChanged(3) => {
                                    self.audio.set_ambient(
                                        &self.fs,
                                        net::info_value_for_key(net.configstring(3), "n"),
                                    );
                                }
                                // `CG_ConfigStringModified` re-runs
                                // `CG_RegisterItems` on CS 8 (cgame 0x3002c70a).
                                net::NetEvent::ConfigstringChanged(8)
                                    if matches!(phase, Phase::Live(_)) =>
                                {
                                    prewarm_viewmodels(view, r, &self.fs, net.configstrings());
                                }
                                net::NetEvent::ConfigstringChanged(7) => {
                                    if let Phase::Live(live) = phase {
                                        live.weapons = vcod_common::weapon_table::from_configstring(
                                            &self.fs,
                                            net.configstring(7),
                                        );
                                    }
                                }
                                // `j/k/l` is quick chat; `s <idx>` is the announcer.
                                net::NetEvent::ServerCommand(tokens) => {
                                    if tokens.first().is_some_and(|t| t == "n") {
                                        join.on_restart();
                                    }
                                    // `g`, the bold game message, beeps.
                                    if tokens.first().is_some_and(|t| t == "g") {
                                        self.audio.play_local(&self.fs, "game_message");
                                    }
                                    for cmd in join.on_server_command(
                                        tokens,
                                        net.configstrings(),
                                        net.server_id(),
                                    ) {
                                        net.send_reliable(&cmd);
                                    }
                                    let newest = net.snapshots().newest();
                                    let protocol = &net::protocol::PROTOCOL_V1;
                                    let quick_chat = self.quick_chat.on_server_command(
                                        &self.fs,
                                        tokens,
                                        |num| {
                                            newest
                                                .and_then(|s| s.clients.get(&num))
                                                .map(|c| c.field_i32(protocol, "team"))
                                        },
                                    );
                                    if !quick_chat {
                                        self.audio.on_server_command(
                                            &self.fs,
                                            tokens,
                                            net.configstrings(),
                                        );
                                    }
                                }
                                net::NetEvent::GamestateReady => {
                                    join.on_gamestate();
                                    gamestate_ready = true;
                                    predictor.fall_heights =
                                        pmove::FallHeights::from_systeminfo(net.configstring(1));
                                    self.shell.set_cheats(sv_cheats(net.configstring(1)));
                                }
                                net::NetEvent::ConfigstringChanged(1) => {
                                    predictor.fall_heights =
                                        pmove::FallHeights::from_systeminfo(net.configstring(1));
                                    self.shell.set_cheats(sv_cheats(net.configstring(1)));
                                }
                                _ => {}
                            }
                        }

                        hud::menu::sync(
                            menu_view,
                            join,
                            &mut self.menus,
                            &self.fs,
                            &self.localized,
                            net.configstrings(),
                        );

                        // One quick-chat line per second, played and shown
                        // like a chat line (retail queues text + alias).
                        if let Some(line) = self.quick_chat.drain(time) {
                            let name = net
                                .snapshots()
                                .newest()
                                .and_then(|s| s.clients.get(&line.client_num))
                                .map(|c| c.name(&net::protocol::PROTOCOL_V1))
                                .unwrap_or_else(|| format!("player {}", line.client_num));
                            console::log::print(&format!("{name}: {}", line.text));
                            if let Some(hud) = &mut self.hud {
                                let now_ms = (time * 1000.0) as i32;
                                hud.chat.push(&format!("{name}: {}", line.text), now_ms);
                            }
                            self.audio.play(&self.fs, line.cue);
                        }

                        // Re-send `score` every 2 s while Tab is held, as the
                        // stock client does (cod11-hud-protocol.md, section 4).
                        if let Some(hud) = &mut self.hud
                            && hud.scoreboard.due(time)
                        {
                            net.send_reliable("score");
                            hud.scoreboard.mark_requested(time);
                        }

                        if gamestate_ready {
                            cmd_clock.reset();
                            ring.clear();
                            predictor.reset();
                            // `CL_SystemInfoChanged` sets `fs_game` and the
                            // pure list, and the gamestate parse restarts the
                            // search path when either changed
                            // (docs/research/cod11-front-end.md, section 16).
                            // A server with no `fs_game` sends no key, which
                            // leaves the player's own mod in place.
                            let game = net::download::fs_game(net.configstring(1), &self.mod_dir)
                                .or_else(|| self.user_fs_game.clone());
                            let pure = net::download::pure_paks(net.configstring(1));
                            if game != self.fs_game || pure != self.fs_pure {
                                log::info!(
                                    "fs_game {}{}: restarting the search path",
                                    game.as_deref().unwrap_or("(none)"),
                                    if pure.is_some() { ", pure server" } else { "" },
                                );
                                self.fs_game = game;
                                self.fs_pure = pure;
                                let (base, game) =
                                    search_dirs(&self.game_dir, &self.mod_dir, &self.fs_game);
                                match reopen_fs(
                                    &base,
                                    game.as_deref(),
                                    self.fs_pure.as_deref(),
                                    &mut self.fs,
                                    &mut self.localized,
                                    &mut self.menus,
                                    &mut self.hud,
                                    &mut self.audio,
                                    &mut self.fx,
                                    &mut self.quick_chat,
                                ) {
                                    Ok(()) => {
                                        *menu_view = None;
                                        r.reopen(&self.fs);
                                        view.reopen();
                                        self.ui = new_ui(&self.fs, &self.game_dir, &self.mod_dir);
                                    }
                                    Err(e) => fatal = Some(e),
                                }
                            }
                        }
                        // Only with the map up: the first cmd after a gamestate
                        // is what enters the client into the world
                        // (docs/protocol-1.1.md, "Entering the world").
                        if matches!(phase, Phase::Live(_))
                            && !gamestate_ready
                            && net.state() == net::NetState::Active
                        {
                            let realtime = local_ms as i32;
                            // Before the first snapshot there is no clock yet;
                            // the entering cmd goes out on the gamestate's.
                            let server_now = match (&mut *phase, net.snapshots().newest()) {
                                (Phase::Live(live), Some(s)) => {
                                    let nudge = self.shell.cvar_f32("cl_timeNudge") as i32;
                                    live.clock.update(realtime, s.server_time, nudge);
                                    live.clock.cmd_time(s.server_time)
                                }
                                _ => net.server_clock_ms(),
                            };
                            let times = cmd_clock.due(server_now);
                            if !times.is_empty() {
                                let held = net.snapshots().newest().map_or_else(
                                    play::input::Held::default,
                                    |s| {
                                        play::input::Held::from_ps(
                                            &s.ps,
                                            &net::protocol::PROTOCOL_V1,
                                        )
                                    },
                                );
                                input.cl_run =
                                    play::input::ClRun(self.shell.cvar_f32("cl_run") as i32);
                                for &t in &times {
                                    ring.push(input.build(t, &held));
                                }
                            }
                            let lan = net.transport().is_lan();
                            let max_packets = self.shell.cvar_f32("cl_maxpackets") as i32;
                            if ring.packet_due(realtime, lan, max_packets) {
                                let dup = self.shell.cvar_f32("cl_packetdup") as i32;
                                net.send_cmds(&ring.packet(realtime, dup));
                            }
                        }

                        let progress = net.download_progress();
                        let frame = match phase {
                            Phase::Connecting => {
                                // The net client's own resend and gamestate
                                // timers decide when a slow server is given up
                                // on; a mod's connect notice can hold the
                                // `connectResponse` back for seconds.
                                if gamestate_ready {
                                    match start_loading(
                                        net,
                                        &mut self.hud,
                                        &mut self.audio,
                                        &mut self.fx,
                                        &mut self.world,
                                        r,
                                        self.window.as_deref(),
                                        &mut self.title,
                                        &self.game_dir,
                                        &download_dirs(&self.mod_dir, &self.fs_game),
                                        allow_download(&self.shell, net.configstring(1)),
                                    ) {
                                        Ok(next) => *phase = next,
                                        Err(e) => fatal = Some(e),
                                    }
                                }
                                loading_frame(
                                    r,
                                    &self.fs,
                                    &self.localized,
                                    &mut self.hud,
                                    time,
                                    aspect,
                                    cull,
                                    net.configstrings(),
                                    format!(
                                        "Connecting to {}...",
                                        self.connect_addr.as_deref().unwrap_or("?")
                                    ),
                                    &mut hud_quads,
                                )
                            }
                            Phase::Loading { loader } => {
                                let map = loader.map().to_string();
                                let map_resolves = self.fs.resolve_map(&map).is_some();
                                let mut waited = None;
                                // A Ready or Failed loader is never stepped again:
                                // Ready swaps the phase, Failed exits.
                                match loader.step(&events, progress, map_resolves, now) {
                                    loading::Action::BeginDownload { remote, dest } => {
                                        if let Err(e) = net.begin_download(&remote, &dest) {
                                            fatal = Some(e.context("cannot start the download"));
                                        }
                                    }
                                    loading::Action::Reopen => {
                                        let (base, game) = search_dirs(
                                            &self.game_dir,
                                            &self.mod_dir,
                                            &self.fs_game,
                                        );
                                        match reopen_fs(
                                            &base,
                                            game.as_deref(),
                                            self.fs_pure.as_deref(),
                                            &mut self.fs,
                                            &mut self.localized,
                                            &mut self.menus,
                                            &mut self.hud,
                                            &mut self.audio,
                                            &mut self.fx,
                                            &mut self.quick_chat,
                                        ) {
                                            Ok(()) => {
                                                *menu_view = None;
                                                r.reopen(&self.fs);
                                                view.reopen();
                                            }
                                            Err(e) => fatal = Some(e),
                                        }
                                    }
                                    loading::Action::FinishDownloads => net.finish_downloads(),
                                    loading::Action::Ready => {
                                        r.set_image_gamma(image_gamma(
                                            &mut self.shell,
                                            self.ignore_hw_gamma,
                                        ));
                                        match load_map(
                                            &map,
                                            net,
                                            &mut self.fs,
                                            &mut self.audio,
                                            &mut self.world,
                                            r,
                                            self.window.as_deref(),
                                            &mut self.title,
                                        ) {
                                            Ok(next) => {
                                                *phase = next;
                                                // `CL_DownloadsComplete` sends it
                                                // after the cgame loads; a pure
                                                // server drops a client without it.
                                                let feed =
                                                    net.gamestate().map_or(0, |g| g.checksum_feed);
                                                net.send_reliable(&self.fs.pure_command(feed));
                                                prewarm_viewmodels(
                                                    view,
                                                    r,
                                                    &self.fs,
                                                    net.configstrings(),
                                                );
                                                view.new_gamestate();
                                            }
                                            Err(e) => fatal = Some(e),
                                        }
                                    }
                                    loading::Action::Failed(msg) => fatal = Some(anyhow!(msg)),
                                    loading::Action::Wait(p) => waited = p,
                                }
                                loading_frame(
                                    r,
                                    &self.fs,
                                    &self.localized,
                                    &mut self.hud,
                                    time,
                                    aspect,
                                    cull,
                                    net.configstrings(),
                                    loading_text(&map, waited),
                                    &mut hud_quads,
                                )
                            }
                            Phase::Live(live) => {
                                if gamestate_ready {
                                    match start_loading(
                                        net,
                                        &mut self.hud,
                                        &mut self.audio,
                                        &mut self.fx,
                                        &mut self.world,
                                        r,
                                        self.window.as_deref(),
                                        &mut self.title,
                                        &self.game_dir,
                                        &download_dirs(&self.mod_dir, &self.fs_game),
                                        allow_download(&self.shell, net.configstring(1)),
                                    ) {
                                        Ok(next) => *phase = next,
                                        Err(e) => fatal = Some(e),
                                    }
                                    let map = match phase {
                                        Phase::Loading { loader } => loader.map().to_string(),
                                        _ => String::new(),
                                    };
                                    loading_frame(
                                        r,
                                        &self.fs,
                                        &self.localized,
                                        &mut self.hud,
                                        time,
                                        aspect,
                                        cull,
                                        net.configstrings(),
                                        format!("Loading {map}"),
                                        &mut hud_quads,
                                    )
                                } else {
                                    let LivePhase {
                                        world,
                                        weapons,
                                        scene,
                                        events,
                                        predicted_events,
                                        clock,
                                        last_loop_snap,
                                        drawn_pos,
                                    } = &mut **live;
                                    let p = &net::protocol::PROTOCOL_V1;
                                    let client_num = net.gamestate().map_or(-1, |g| g.client_num);
                                    // While following, ps.clientNum is the followed
                                    // client's (docs/research/cod11-events-and-fx.md,
                                    // section 7); the camera sits inside that body, so
                                    // skip it instead of ours.
                                    let ps_client = net
                                        .snapshots()
                                        .newest()
                                        .map_or(-1, |s| s.ps.field_i32(p, "clientNum"));
                                    let following = ps_client >= 0 && ps_client != client_num;
                                    // Intermission (5) and dead (6, 7) take no view
                                    // from the cmd (Q3 `PM_UpdateViewAngles`), so
                                    // those draw the snapshot's view, as following does.
                                    let pm_type = net
                                        .snapshots()
                                        .newest()
                                        .map_or(0, |s| s.ps.field_i32(p, "pm_type"));
                                    let snapshot_view = following || (5..=7).contains(&pm_type);
                                    let skip_num = if following { ps_client } else { client_num };
                                    // Rebuilt every frame so absent entities (PVS churn) drop out.
                                    let mut instances: Vec<DynamicModelInstance> = Vec::new();
                                    // Per-shooter muzzle transforms for flashes and tracers.
                                    let mut muzzles: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
                                    let mut weapon_flash: HashMap<i32, String> = HashMap::new();
                                    let mut entity_pos: HashMap<u32, Vec3> = HashMap::new();
                                    let mut heads: HashMap<u32, Vec3> = HashMap::new();
                                    let mut turret_eye = None;

                                    let render_time = net
                                        .snapshots()
                                        .newest()
                                        .and(clock.delta())
                                        .map(|_| clock.server_time());
                                    if let Some(render_time) = render_time
                                        && let Some((a, b)) =
                                            net.snapshots().two_for_time(render_time)
                                    {
                                        // No straddling pair: one frame, held.
                                        if std::ptr::eq(a, b) {
                                            self.interp_misses += 1;
                                        }
                                        let oa = Vec3::from(a.ps.origin(p));
                                        let ob = Vec3::from(b.ps.origin(p));
                                        let f = ((render_time - a.server_time) as f32
                                            / (b.server_time - a.server_time).max(1) as f32)
                                            .clamp(0.0, 1.0);
                                        let t0 = Instant::now();
                                        // Last frame's drawn bodies, for the gunner's
                                        // trace down.
                                        let bodies = play::predict::solid_bodies(
                                            p,
                                            &b.entities,
                                            client_num as u32,
                                            drawn_pos,
                                        );
                                        scene.swing_speed = self.shell.cvar_f32("bg_swingSpeed");
                                        let built = entities::build_instances(
                                            scene,
                                            (a, b, f),
                                            render_time,
                                            skip_num,
                                            net.configstrings(),
                                            &self.fs,
                                            Some(MoveWorld::new(world, &bodies, u32::MAX)),
                                            r,
                                            p,
                                        );
                                        self.build_ms = t0.elapsed().as_secs_f32() * 1000.0;
                                        instances = built.instances;
                                        muzzles = built.muzzles;
                                        weapon_flash = built.weapon_flash;
                                        entity_pos = built.entity_pos;
                                        heads = built.heads;
                                        turret_eye = built.turret_eye;
                                        r.set_submodels(&built.submodels);
                                        // Over 512 u is a teleport, not motion.
                                        let pos = if oa.distance(ob) > 512.0 {
                                            ob
                                        } else {
                                            oa.lerp(ob, f)
                                        };
                                        // viewHeightCurrent: eye offset above the feet origin.
                                        let vh = a.ps.field_f32(p, "viewHeightCurrent") * (1.0 - f)
                                            + b.ps.field_f32(p, "viewHeightCurrent") * f;
                                        cam.pos = pos + Vec3::Z * vh;
                                        if snapshot_view {
                                            let va_a = a.ps.viewangles(p);
                                            let va_b = b.ps.viewangles(p);
                                            cam.yaw = camera::lerp_angle(va_a[1], va_b[1], f)
                                                .to_radians();
                                            cam.pitch = -camera::lerp_angle(va_a[0], va_b[0], f)
                                                .to_radians();
                                        }
                                    }
                                    let predicted = if ps_client == client_num {
                                        net.snapshots().newest().and_then(|s| {
                                            let bodies = play::predict::solid_bodies(
                                                p,
                                                &s.entities,
                                                client_num as u32,
                                                drawn_pos,
                                            );
                                            let movers = (
                                                pmove::movers::SnapshotMovers::from_entities(
                                                    p,
                                                    &s.entities,
                                                ),
                                                s.server_time,
                                            );
                                            predictor.predict(
                                                p, &s.ps, ring, world, &bodies, &movers, weapons,
                                                local_ms,
                                            )
                                        })
                                    } else {
                                        predictor.reset();
                                        None
                                    };
                                    drawn_pos.clone_from(&entity_pos);
                                    if let Some(v) = &predicted {
                                        cam.pos = v.origin + Vec3::Z * v.view_height;
                                    }
                                    if let Some(s) = net.snapshots().newest() {
                                        view.track_spawn((
                                            s.ps.field_i32(p, "clientNum"),
                                            s.ps.arrays.stats[5],
                                        ));
                                    }
                                    // A followed player's view weapon, zoom and scope
                                    // come off the snapshot, as retail draws them.
                                    let view_ps = net.snapshots().newest().and_then(|s| {
                                        pmove::predict::predictable(pm_type).then(|| {
                                            match &predicted {
                                                Some(v) => play::view::ViewPs::from_predicted(
                                                    &v.pred,
                                                    s.ps.field_i32(p, "viewmodelIndex"),
                                                ),
                                                None => play::view::ViewPs::from_snapshot(p, &s.ps),
                                            }
                                        })
                                    });
                                    if let Some(ps) = &view_ps
                                        && let Some(models) =
                                            view.sync_rig(&self.fs, net.configstrings(), ps)
                                    {
                                        r.set_viewmodel(&self.fs, &models);
                                        self.viewmodel = models;
                                    }
                                    let cg_fov = cg_fov(&self.shell);
                                    let (vm_draw, fov) =
                                        view.frame(cg_fov, weapons, view_ps.as_ref(), dt, local_ms);
                                    self.look_zoom =
                                        (fov / cg_fov, view_ps.as_ref().is_some_and(|v| v.mounted));
                                    vm = vm_draw;
                                    // Next frame's cmds carry this kick, as
                                    // `CL_FinishMove` reads last frame's syscall 0x56.
                                    input.kick = view.view_kick();
                                    // `CG_CalcViewValues` draws the predicted
                                    // `viewangles[2]`, the kick's roll included.
                                    let mut view_roll = 0.0;
                                    if !snapshot_view {
                                        let delta = match &predicted {
                                            Some(v) => v.delta_angles,
                                            None => net.snapshots().newest().map_or([0; 3], |s| {
                                                net::DELTA_ANGLE_FIELDS
                                                    .map(|name| s.ps.field_i32(p, name))
                                            }),
                                        };
                                        (cam.yaw, cam.pitch, view_roll) =
                                            own_view(input.cmd_angles(), delta);
                                    }
                                    // On a mounted gun the view rides the gun's
                                    // `tag_player` and barrel, not the cmd's angles
                                    // (docs/research/cod11-turrets.md section 14).
                                    if let Some(eye) = &turret_eye {
                                        cam.pos = eye.pos;
                                        (cam.yaw, cam.pitch) = eye.view();
                                        view_roll = 0.0;
                                    }

                                    let (cam_forward, cam_right, cam_up) =
                                        camera::basis(cam.yaw, cam.pitch);
                                    // The ridden body is `skip_num`, never in `entity_pos`,
                                    // so the listener is told which entity rides the camera.
                                    self.audio.set_listener(
                                        cam.pos,
                                        cam_forward,
                                        cam_right,
                                        cam_up,
                                        audio::cues::ps_entity(ps_client),
                                    );

                                    let (muzzle_pos, muzzle_dir) = view
                                        .muzzle(cam.pos, (cam_forward, cam_right, cam_up))
                                        .unwrap_or_else(|| {
                                            view_muzzle(cam.pos, cam_forward, cam_right, cam_up)
                                        });
                                    // Under a scope our own shots draw no flash: with
                                    // no muzzle the fire event resolves to nothing.
                                    if !view.scoped() {
                                        muzzles.insert(u32::MAX, (muzzle_pos, muzzle_dir));
                                    }
                                    // Bullet hits carry the shooter's number in
                                    // `other_entity_num`, and the body the camera rides
                                    // (ours, or the followed player's) is excluded from
                                    // the muzzle map, so key the view muzzle under it too
                                    // for the whizby; its tracer is `view_body`'s call.
                                    if let Ok(num) = u32::try_from(skip_num) {
                                        muzzles.insert(num, (muzzle_pos, muzzle_dir));
                                    }

                                    r.set_dynamic_models(&instances);

                                    // Step before this frame's events spawn, or the
                                    // [now-dt, now] integration would move particles born
                                    // at `now` (a tracer would start ahead of its muzzle).
                                    let fx_t0 = Instant::now();
                                    self.fx.step(dt, time, Some(&*world));

                                    let (screen_w, screen_h) = r.screen_size();

                                    let no_clients = BTreeMap::new();
                                    let no_entities = BTreeMap::new();
                                    let newest = net.snapshots().newest();
                                    let local_player = ps_client == client_num
                                        && pmove::predict::predictable(pm_type);
                                    let entity_origin = |num: i32| {
                                        let num = u32::try_from(num).ok()?;
                                        entity_pos.get(&num).map(|v| v.to_array())
                                    };
                                    // Retail reads `cg_entities[n].currentState`;
                                    // the newest snapshot stands in.
                                    let turret_weapon = newest.and_then(|s| {
                                        let int = |n: &str| s.ps.field_i32(p, n);
                                        let num = turret::ridden(
                                            int("eFlags"),
                                            int("viewlocked"),
                                            int("viewlocked_entNum"),
                                        )?;
                                        let w = s.entities.get(&num)?.field_i32(p, "weapon");
                                        usize::try_from(w).ok()
                                    });
                                    let client_cvar =
                                        |c: &str| join.cvars.get(c, net.configstrings());
                                    let hud_frame = hud::HudFrame {
                                        now: time,
                                        screen_w,
                                        screen_h,
                                        configstrings: net.configstrings(),
                                        clients: newest.map_or(&no_clients, |s| &s.clients),
                                        protocol: p,
                                        // The render clock, so hudelem tweens and
                                        // timers move between snapshots.
                                        server_time: render_time.unwrap_or(0),
                                        snap_time: render_time
                                            .and_then(|t| net.snapshots().two_for_time(t))
                                            .map_or(0, |(a, _)| a.server_time),
                                        fs: &self.fs,
                                        menu: menu_view.as_ref().map(|(_, v)| v),
                                        ps: newest.map(|s| &s.ps),
                                        entities: newest.map_or(&no_entities, |s| &s.entities),
                                        predicted: predicted
                                            .as_ref()
                                            .filter(|_| local_player)
                                            .map(|v| &v.pred),
                                        local_player,
                                        weapons,
                                        localized: &self.localized,
                                        view_yaw: cam.yaw.to_degrees(),
                                        eye: cam.pos.to_array(),
                                        fov,
                                        entity_origin: &entity_origin,
                                        turret_weapon,
                                        cvar: &client_cvar,
                                        bound_key: &|cmd| self.shell.key_text(cmd, &self.localized),
                                        draw: hud::DrawToggles {
                                            crosshair: self.shell.cvar_f32("cg_drawCrosshair")
                                                as i32
                                                != 0,
                                            status: self.shell.cvar_f32("cg_drawStatus") as i32
                                                != 0,
                                        },
                                        weapon_select: input
                                            .weapon_select()
                                            .filter(|_| local_player),
                                    };

                                    // Events use the newest snapshot, not the interpolation
                                    // pair; they must not wait out the interp delay.
                                    if let Some(newest) = newest {
                                        let ctx = fx::registry::ResolveCtx {
                                            muzzles: &muzzles,
                                            weapon_flash: &weapon_flash,
                                            view_flash: vm.is_some().then_some(weapons.as_slice()),
                                            view_body: fx::registry::view_body(
                                                newest.ps.field_i32(p, "pm_flags"),
                                                ps_client,
                                            ),
                                        };
                                        // Our own ring plays off the prediction, and
                                        // the snapshot's copy of it is skipped. The
                                        // frame we die or reach the intermission still
                                        // skips the copies, then tracking stops.
                                        let own = ps_client == client_num;
                                        let predicting =
                                            own && pmove::predict::predictable(pm_type);
                                        let mut evs = Vec::new();
                                        if !own {
                                            predicted_events.stop();
                                        } else if let Some(v) =
                                            predicted.as_ref().filter(|_| predicting)
                                        {
                                            predicted_events.start_after(events.ps_sequence());
                                            let pred = &v.pred;
                                            evs.extend(
                                                predicted_events
                                                    .take_predicted(
                                                        pred.ring.seq,
                                                        pred.ring.events,
                                                        pred.ring.parms,
                                                    )
                                                    .into_iter()
                                                    .map(|(_, event, parm)| {
                                                        play::events::game_event(
                                                            pred, ps_client, event, parm,
                                                        )
                                                    }),
                                            );
                                        }
                                        for (seq, ev) in events.drain_seq(newest, p) {
                                            if own
                                                && seq.is_some_and(|s| {
                                                    !predicted_events.filter_snapshot(s, ev.event)
                                                })
                                            {
                                                continue;
                                            }
                                            evs.push(ev);
                                        }
                                        if !predicting {
                                            predicted_events.stop();
                                        }
                                        for ev in evs {
                                            self.ev_seen += 1;
                                            // `CG_FireWeapon` kicks only for the
                                            // view's own body (`0x30038bc8`).
                                            if (ev.entity_num == u32::MAX
                                                || ev.entity_num == ps_client as u32)
                                                && let Some(ps) = &view_ps
                                            {
                                                for _ in 0..play::recoil::fire_calls(ev.event) {
                                                    view.fire(weapons, ps);
                                                }
                                            }
                                            if let Some(hud) = &mut self.hud {
                                                hud.on_game_event(&ev, &hud_frame);
                                            }
                                            let empty = input.weapon_select().is_none()
                                                && newest.ps.field_i32(p, "weapon") == 0;
                                            if let Some(s) =
                                                play::events::forced_stance(&ev, client_num)
                                            {
                                                input.force_stance(s);
                                            }
                                            if let Some(w) = play::events::pickup_selects(
                                                &ev,
                                                ctx.view_body,
                                                empty,
                                            ) {
                                                input.select(w);
                                                if let Some(hud) = &mut self.hud {
                                                    hud.weapon_selected();
                                                }
                                            }
                                            self.audio.on_game_event(
                                                &self.fs,
                                                &ev,
                                                net.configstrings(),
                                                &muzzles,
                                            );
                                            for r in fx::registry::resolve(&ev, &ctx) {
                                                match r {
                                                    fx::registry::Resolved::Spawn { path, at } => {
                                                        let sounds = self
                                                            .fx
                                                            .spawn(&self.fs, &path, at, time);
                                                        self.audio.play_fx(&self.fs, sounds);
                                                    }
                                                    fx::registry::Resolved::Tracer {
                                                        muzzle,
                                                        impact,
                                                        flesh,
                                                    } => {
                                                        self.fx.spawn_tracer(
                                                            muzzle, impact, flesh, time, dt,
                                                        );
                                                    }
                                                    fx::registry::Resolved::Known => {}
                                                    fx::registry::Resolved::Unknown => {
                                                        self.ev_unknown += 1;
                                                        log::debug!(
                                                            "unknown event {} parm {}",
                                                            ev.event,
                                                            ev.parm
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // `es.loopSound` (docs/research/cod11-sound-system.md,
                                    // section 9), reconciled once per snapshot. An entity
                                    // that left the snapshot is absent from the map, which
                                    // is what stops its loop.
                                    if let Some(newest) = newest
                                        && *last_loop_snap != Some(newest.message_num)
                                    {
                                        *last_loop_snap = Some(newest.message_num);
                                        let loops: HashMap<u32, (i32, Vec3)> = newest
                                            .entities
                                            .iter()
                                            .filter_map(|(&num, e)| {
                                                let idx = e.field_i32(p, "loopSound");
                                                (idx != 0)
                                                    .then(|| (num, (idx, Vec3::from(e.origin(p)))))
                                            })
                                            .collect();
                                        self.audio.set_loop_sounds(
                                            &self.fs,
                                            net.configstrings(),
                                            &loops,
                                        );
                                    }

                                    // After the drain, so new voices get this frame's positions.
                                    self.audio.step(&entity_pos, Some(&*world));

                                    let mut fx_quads =
                                        self.fx.build_quads(cam.pos, cam_right, cam_up, time);
                                    if let Some(newest) = newest {
                                        fx_quads.extend(head_icon::quads(
                                            &head_icon::Scene {
                                                protocol: p,
                                                entities: &newest.entities,
                                                clients: &newest.clients,
                                                configstrings: net.configstrings(),
                                                viewer: ps_client,
                                                heads: &heads,
                                                entity_pos: &entity_pos,
                                            },
                                            cam_right,
                                            cam_up,
                                        ));
                                        fx::sim::sort_back_to_front(&mut fx_quads, cam.pos);
                                    }
                                    r.set_fx_quads(&self.fs, fx_quads);
                                    r.set_fx_lights(&self.fx.lights(cam.pos, time));
                                    r.set_fog(
                                        net::FogParams::parse(
                                            net.configstring(net::protocol::CS_FOG_V1),
                                        ),
                                        time,
                                    );
                                    self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;

                                    if let Some(hud) = &mut self.hud {
                                        hud.set_gun_kick(view.gun_kick());
                                        let hud_t0 = Instant::now();
                                        hud_quads = hud.build(&hud_frame);
                                        self.hud_ms = hud_t0.elapsed().as_secs_f32() * 1000.0;
                                    }

                                    renderer::Frame {
                                        view_proj: camera::view_proj_from(
                                            cam.pos, cam.yaw, cam.pitch, view_roll, fov, aspect,
                                        ),
                                        eye: cam.pos,
                                        fwd: cam_forward,
                                        up: Vec3::Z,
                                        time,
                                        cull,
                                        hud_lines: Vec::new(),
                                    }
                                }
                            }
                        };
                        (frame, vm)
                    }
                    Mode::Walk {
                        world,
                        ps,
                        input,
                        keys,
                        motion,
                        view_weapon,
                        configstrings,
                        weapon_slot,
                        reserve,
                        switch_to,
                        rigs,
                        mouse_delta,
                        fire_edge,
                        fire_held,
                        reload_edge,
                        ads_held,
                        gun_aim,
                    } => {
                        // Before this frame's fire event spawns, or the
                        // [now-dt, now] integration would move the new tracer
                        // ahead of its muzzle.
                        let fx_t0 = Instant::now();
                        self.fx.step(dt, time, Some(&*world));

                        if let Some(slot) = switch_to.take()
                            && slot != *weapon_slot
                            && slot < WALK_LOADOUT.len()
                        {
                            let name = WALK_LOADOUT[slot];
                            let t0 = Instant::now();
                            match rigs.load(&self.fs, name, None) {
                                Some((models, vw)) => {
                                    let t1 = Instant::now();
                                    r.set_viewmodel(&self.fs, &models);
                                    log::debug!(
                                        "switch to {name}: rig {:.2} ms, upload {:.2} ms",
                                        (t1 - t0).as_secs_f64() * 1e3,
                                        t1.elapsed().as_secs_f64() * 1e3
                                    );
                                    *reserve = vw.as_ref().map_or(0, |w| w.def.start_ammo);
                                    self.viewmodel = models;
                                    *view_weapon = vw;
                                    *weapon_slot = slot;
                                }
                                None => log::warn!(
                                    "weapon {name} failed to load; keeping {}",
                                    WALK_LOADOUT[*weapon_slot]
                                ),
                            }
                        }

                        (input.forward, input.right) = keys.axes();
                        let mw = MoveWorld::bare(world);
                        for ev in pmove::pmove(ps, input, &mw, dt, &[]) {
                            // A refused prone toggles the key back off, as
                            // retail's `cl_stance` would be.
                            if matches!(
                                ev.event,
                                net::event_ids::EV_STANCE_FORCE_STAND
                                    | net::event_ids::EV_STANCE_FORCE_CROUCH
                            ) {
                                input.prone = false;
                            }
                            self.audio.on_game_event(
                                &self.fs,
                                &net::events::GameEvent {
                                    event: ev.event,
                                    parm: 0,
                                    // playerState-ring form: no entity, rides the listener
                                    entity_num: u32::MAX,
                                    client_num: -1,
                                    weapon: 0,
                                    surf_type: 0,
                                    pos: ps.origin.to_array(),
                                    dir: [0.0; 3],
                                    other_entity_num: u32::MAX,
                                    attacker_entity_num: -1,
                                },
                                configstrings,
                                &HashMap::new(),
                            );
                        }
                        input.jump = false; // Q3: a held jump doesn't autohop
                        let ground_speed = if ps.on_ground {
                            ps.velocity.truncate().length()
                        } else {
                            0.0
                        };

                        let v = ps.view();
                        let (eye_forward, eye_right, eye_up) = camera::basis(v.yaw, v.pitch);
                        self.audio
                            .set_listener(v.eye, eye_forward, eye_right, eye_up, u32::MAX);
                        let mut bone_sets = Vec::new();
                        let mut fov = camera::DEFAULT_FOV_DEG;
                        let mut damp = 1.0;
                        let mut scope_quads = Vec::new();
                        let mut scoped = false;
                        if let Some(w) = view_weapon {
                            let out = w.state.update(
                                dt,
                                weapon::WeaponInput {
                                    fire: *fire_edge,
                                    fire_held: *fire_held,
                                    ads: *ads_held,
                                    reload: *reload_edge,
                                },
                            );
                            w.pose_sight(out.ads_frac, *ads_held);
                            let clip = w
                                .anims
                                .get(&out.anim)
                                .or_else(|| w.anims.get(&weapon::WeaponAnim::Idle));
                            if let Some((anim, binding)) = clip {
                                let frame = anim.frame_pos(out.anim_time, out.looping);
                                w.pose.apply(anim, binding, frame);
                                // two models, hands then gun, in set_viewmodel order
                                bone_sets = (0..2)
                                    .map(|m| w.pose.skin_matrices(&w.skeleton, m))
                                    .collect();
                            }
                            fov = weapon::view_fov_x(
                                cg_fov(&self.shell),
                                Some(&w.def),
                                out.ads_frac,
                                *ads_held,
                                false,
                                false,
                            );
                            damp = 1.0 + (w.def.ads_view_bob_mult - 1.0) * out.ads_frac;
                            damp *= 1.0 + (w.def.ads_bob_factor - 1.0) * out.ads_frac;
                            // The walk sight runs outside pmove, so the gun's
                            // sway reads its fraction off a copy.
                            let mut sighted = **ps;
                            sighted.weapon_pos_frac = out.ads_frac;
                            let gun = gun_aim.step(Some(&w.def), &sighted, (time * 1000.0) as i32);
                            if hud::scope::overlay_frac(&w.def, out.ads_frac, *ads_held).is_some() {
                                let screen = r.screen_size();
                                let fovs = (fov, camera::fov_y(fov, aspect));
                                let at =
                                    hud::scope::gun_point(gun.unwrap_or_default(), fovs, screen);
                                hud::scope::build(&w.def, at, screen, &mut scope_quads);
                                // The gun hides under it, as online.
                                scoped = true;
                            }
                            if let Some(cue) = out.cue {
                                if matches!(
                                    cue,
                                    weapon::WeaponCue::Reload | weapon::WeaponCue::ReloadFromEmpty
                                ) {
                                    let need = w.def.clip_size.saturating_sub(w.state.ammo());
                                    *reserve -= need.min(*reserve);
                                }
                                self.audio.on_game_event(
                                    &self.fs,
                                    &net::events::GameEvent {
                                        event: match cue {
                                            weapon::WeaponCue::Fire => fx::registry::EV_FIRE_WEAPON,
                                            weapon::WeaponCue::LastShot => {
                                                fx::registry::EV_FIRE_WEAPON_LASTSHOT
                                            }
                                            weapon::WeaponCue::Rechamber => {
                                                fx::registry::EV_RECHAMBER_WEAPON
                                            }
                                            weapon::WeaponCue::Reload => fx::registry::EV_RELOAD,
                                            weapon::WeaponCue::ReloadFromEmpty => {
                                                fx::registry::EV_RELOAD_FROM_EMPTY
                                            }
                                            weapon::WeaponCue::Raise => {
                                                fx::registry::EV_RAISE_WEAPON
                                            }
                                        },
                                        parm: 0,
                                        entity_num: u32::MAX,
                                        client_num: -1,
                                        // 1-based CS7 index of the active loadout slot
                                        weapon: *weapon_slot as i32 + 1,
                                        surf_type: 0,
                                        pos: ps.origin.to_array(),
                                        dir: [0.0; 3],
                                        other_entity_num: u32::MAX,
                                        attacker_entity_num: -1,
                                    },
                                    configstrings,
                                    &HashMap::new(),
                                );
                            }
                            if out.fired {
                                /// Q3/CoD bullet trace length.
                                const FIRE_RANGE: f32 = 8192.0;
                                // a point trace against solids only: playerclip-only
                                // geometry stops movement but not bullets
                                let tr = world.shot_trace(v.eye, v.eye + eye_forward * FIRE_RANGE);
                                // A muzzle inside solid returns no normal, which
                                // would build a NaN quad that never leaves the ring.
                                if tr.fraction < 1.0 && !tr.startsolid {
                                    // Same csv resolution as a server-driven hit:
                                    // surfType rides the surface flags' bits 20-24.
                                    let mut muzzles = HashMap::new();
                                    muzzles.insert(
                                        u32::MAX,
                                        (v.eye + eye_forward * 16.0 - Vec3::Z * 2.0, eye_forward),
                                    );
                                    let ctx = fx::registry::ResolveCtx {
                                        muzzles: &muzzles,
                                        weapon_flash: &HashMap::new(),
                                        view_flash: None,
                                        view_body: None,
                                    };
                                    let ev = net::events::GameEvent {
                                        event: fx::registry::EV_BULLET_HIT_SMALL,
                                        parm: net::events::dir_to_byte(tr.normal.to_array()),
                                        entity_num: u32::MAX,
                                        client_num: -1,
                                        weapon: *weapon_slot as i32 + 1,
                                        surf_type: collision::sound_material(tr.surface_flags),
                                        pos: tr.endpos.to_array(),
                                        dir: [0.0; 3],
                                        other_entity_num: u32::MAX,
                                        attacker_entity_num: -1,
                                    };
                                    for r in fx::registry::resolve(&ev, &ctx) {
                                        match r {
                                            fx::registry::Resolved::Spawn { path, at } => {
                                                let sounds =
                                                    self.fx.spawn(&self.fs, &path, at, time);
                                                self.audio.play_fx(&self.fs, sounds);
                                            }
                                            fx::registry::Resolved::Tracer {
                                                muzzle,
                                                impact,
                                                flesh,
                                            } => {
                                                self.fx
                                                    .spawn_tracer(muzzle, impact, flesh, time, dt);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        *fire_edge = false;
                        *reload_edge = false;

                        motion.update(
                            dt,
                            ground_speed,
                            ps.on_ground,
                            mouse_delta.0,
                            mouse_delta.1,
                            damp,
                        );
                        *mouse_delta = (0.0, 0.0);
                        hud_quads = scope_quads;
                        self.audio.step(&HashMap::new(), None);
                        r.set_fx_quads(
                            &self.fs,
                            self.fx.build_quads(v.eye, eye_right, eye_up, time),
                        );
                        r.set_fx_lights(&self.fx.lights(v.eye, time));
                        r.set_fog(
                            net::FogParams::parse(
                                configstrings
                                    .get(net::protocol::CS_FOG_V1)
                                    .map(String::as_str)
                                    .unwrap_or(""),
                            ),
                            time,
                        );
                        self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;
                        (
                            renderer::Frame {
                                view_proj: camera::view_proj_from(
                                    v.eye, v.yaw, v.pitch, v.roll, fov, aspect,
                                ),
                                eye: v.eye,
                                fwd: camera::basis(v.yaw, v.pitch).0,
                                up: camera::up_hint(v.yaw, v.pitch, v.roll),
                                time,
                                cull,
                                hud_lines: vec![format!(
                                    "[{}] {} {} / {}",
                                    *weapon_slot + 1,
                                    WALK_LOADOUT[*weapon_slot],
                                    view_weapon.as_ref().map_or(0, |w| w.state.ammo()),
                                    reserve
                                )],
                            },
                            (!scoped).then(|| renderer::VmDraw {
                                transform: motion.transform(),
                                light_origin: ps.origin + Vec3::Z * ps.view_height(),
                                fov_x: fov,
                                bone_sets,
                            }),
                        )
                    }
                };
                // A drop or a failed load ends the session, not the program:
                // retail falls back to its console and menus.
                if let Some(err) = fatal {
                    self.disconnect(Some(format!("{err:#}")));
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if self.ui.active() {
                    self.ui.frame(now, &self.shell);
                    let (w, h) = r.screen_size();
                    hud_quads.extend(self.ui.build(w, h, &self.localized, &self.shell));
                }
                self.console.drain_log();
                self.console.update(
                    dt * 1000.0,
                    self.shell.cvar_f32("scr_conspeed"),
                    matches!(self.mode, Mode::Idle) && !self.ui.active(),
                );
                if self.console.visible() {
                    let (w, h) = r.screen_size();
                    hud_quads.extend(self.console.build(w, h, elapsed.as_millis() as u64));
                }
                r.set_hud_quads(&self.fs, hud_quads);
                // The scene lives in the online phase, so pull it back
                // out for the overlay after the mode's mutable borrows end.
                let scene = match &self.mode {
                    Mode::Online {
                        phase: Phase::Live(live),
                        ..
                    } => Some(&live.scene),
                    _ => None,
                };
                self.hud_stats.frame(
                    dt,
                    scene.map_or(0, |s| s.stats.anim_restarts),
                    self.interp_misses,
                );
                if self.debug_overlay {
                    frame.hud_lines = hud_lines(
                        &self.mode,
                        scene,
                        &self.hud_stats,
                        (self.build_ms, self.render_ms, self.fx_ms),
                        (self.ev_seen, self.ev_unknown),
                        self.fx.counts(),
                        (
                            r.hud_quad_count(),
                            self.hud_ms,
                            self.hud.as_ref().map_or(0, |h| h.unknown),
                        ),
                        self.audio.stats(),
                        r,
                    );
                }
                let t0 = Instant::now();
                r.render(frame, vm);
                self.render_ms = t0.elapsed().as_secs_f32() * 1000.0;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if !self.grabbed {
                return;
            }
            let (dx, dy) = (dx as f32, dy as f32);
            let look = play::input::MouseLook {
                sensitivity: self.shell.cvar_f32("sensitivity"),
                m_yaw: self.shell.cvar_f32("m_yaw"),
                m_pitch: self.shell.cvar_f32("m_pitch"),
            };
            match &mut self.mode {
                Mode::Idle => {}
                Mode::Fly(cam) => cam.look(look.degrees(dx, dy, 1.0, false)),
                Mode::Online { input, view, .. } => {
                    let (zoom, mounted) = self.look_zoom;
                    input.mouse(look.degrees(dx, dy, zoom, mounted));
                    view.mouse(dx, dy);
                }
                Mode::Walk {
                    ps, mouse_delta, ..
                } => {
                    // the same turn and pitch clamp as FlyCamera::look
                    let [pitch, yaw] = look.degrees(dx, dy, 1.0, false);
                    ps.yaw += yaw.to_radians();
                    ps.pitch =
                        (ps.pitch - pitch.to_radians()).clamp(camera::PITCH_MIN, camera::PITCH_MAX);
                    // raw counts; the sway spring scales them
                    mouse_delta.0 += dx;
                    mouse_delta.1 += dy;
                }
            }
        }
    }
}

/// `cg_fov` as cgame reads it, clamped to 80..160 (0x30032e20).
fn cg_fov(shell: &console::shell::Shell) -> f32 {
    shell.cvar_f32("cg_fov").clamp(weapon::CG_FOV, 160.0)
}

/// The systeminfo's `sv_cheats`, as `CL_SystemInfoChanged` reads it.
fn sv_cheats(systeminfo: &str) -> bool {
    net::info_value_for_key(systeminfo, "sv_cheats") == Some("1")
}

/// `--probe-say`'s `SECS:COMMAND`.
fn parse_probe_say(s: &str) -> Result<(f32, String), String> {
    let (secs, cmd) = s.split_once(':').ok_or("expected SECS:COMMAND")?;
    let secs = secs.trim().parse().map_err(|e| format!("{secs}: {e}"))?;
    Ok((secs, cmd.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pk3(dir: &std::path::Path, file: &str, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(dir.join(file)).unwrap());
        for (name, content) in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(content).unwrap();
        }
        z.finish().unwrap();
    }

    /// The state `reopen_fs` rebuilds, as it stands when a download lands:
    /// loaded off the old paks, the aliases filtered for the current map.
    struct Reopened {
        localized: vcod_common::localize::Localized,
        menus: hud::menu::MenuCache,
        hud: Option<hud::Hud>,
        audio: audio::AudioSystem,
        fx: fx::sim::FxSystem,
        quick_chat: quick_chat::QuickChat,
    }

    impl Reopened {
        fn load(fs: &Pk3Fs) -> Reopened {
            let mut audio = audio::AudioSystem::new(
                fs,
                audio::AudioOpts {
                    enabled: false,
                    volume: 1.0,
                },
            );
            audio.on_gamestate("mp_mod");
            Reopened {
                localized: vcod_common::localize::Localized::load(fs),
                menus: hud::menu::MenuCache::default(),
                hud: hud::Hud::new(fs).ok(),
                audio,
                fx: fx::sim::FxSystem::new(),
                quick_chat: quick_chat::QuickChat::new(1),
            }
        }

        fn reopen(&mut self, dir: &std::path::Path, fs: &mut Pk3Fs) {
            reopen_fs(
                dir,
                None,
                None,
                fs,
                &mut self.localized,
                &mut self.menus,
                &mut self.hud,
                &mut self.audio,
                &mut self.fx,
                &mut self.quick_chat,
            )
            .unwrap();
        }
    }

    /// A downloaded pak can carry the menus and strings the paks before it
    /// lacked; a miss cached against the old search path must not outlive it.
    #[test]
    fn reopen_reads_menus_and_strings_off_the_new_paks() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("readme.txt", b"")]);
        let mut fs = Pk3Fs::open(dir.path()).unwrap();
        let mut s = Reopened::load(&fs);
        assert!(s.menus.get(&fs, "team_mod").is_none());

        make_pk3(
            dir.path(),
            "zzz_mod.pk3",
            &[
                (
                    "ui_mp/scriptmenus/team_mod.menu",
                    br#"{ menuDef { name "team_mod"
      itemDef { name "a" visible 1 text "@MODMENU_ALLIES" action { scriptMenuResponse "allies"; } }
    } }"#,
                ),
                (
                    "localizedstrings/english/modmenu.str",
                    b"REFERENCE ALLIES\nLANG_ENGLISH \"Allies\"\n",
                ),
            ],
        );
        s.reopen(dir.path(), &mut fs);
        let menu = s.menus.get(&fs, "team_mod").expect("menu from the new pak");
        let v = hud::menu::view(menu, &s.localized, |_| None);
        assert_eq!(v.rows[0].label, "Allies");
    }

    /// The fonts, the sound aliases, the effect files and the impact table
    /// are read off the search path too, and a miss there must not outlive
    /// it either.
    #[test]
    fn reopen_reads_fonts_aliases_and_effects_off_the_new_paks() {
        use audio::cues::{Cue, Source};
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("readme.txt", b"")]);
        let mut fs = Pk3Fs::open(dir.path()).unwrap();
        let mut s = Reopened::load(&fs);
        assert!(s.hud.is_none(), "no fonts in the old paks");
        let shout = || Cue {
            alias: "mod_shout".to_string(),
            source: Source::Point(Vec3::ZERO),
            delay_s: 0.0,
        };
        s.audio.play(&fs, shout());
        assert_eq!(s.audio.stats().misses, 1);
        let at = fx::sim::SpawnAt::Point { pos: Vec3::ZERO };
        assert!(s.fx.spawn(&fs, "fx/mod/shout.efx", at, 0.0).is_empty());

        // `parse_font_dat` takes any file of the right length.
        let font = vec![0u8; 20552];
        make_pk3(
            dir.path(),
            "zzz_mod.pk3",
            &[
                ("fonts/fontImage_12.dat", &font),
                ("fonts/fontImage_16.dat", &font),
                ("fonts/fontImage_18.dat", &font),
                ("fonts/fontImage_24.dat", &font),
                ("fonts/fontImage_30.dat", &font),
                ("fonts/fontImage_32.dat", &font),
                (
                    "soundaliases/mod.csv",
                    b"name,file\nmod_shout,mod/shout.wav\n",
                ),
                (
                    "fx/mod/shout.efx",
                    b"Sound\n{\n\tsounds\n\t[\n\t\tmod_shout\n\t]\n}\n",
                ),
                (
                    "fx/iw_impacts.csv",
                    b"bullet_small_normal,concrete,fx/mod/hit.efx\n",
                ),
            ],
        );
        s.reopen(dir.path(), &mut fs);
        assert!(s.hud.is_some(), "fonts from the new pak");
        s.audio.play(&fs, shout());
        assert_eq!(s.audio.stats().misses, 1, "alias from the new pak");
        let sounds = s.fx.spawn(&fs, "fx/mod/shout.efx", at, 0.0);
        assert_eq!(sounds.len(), 1, "effect from the new pak");
        assert_eq!(sounds[0].alias, "mod_shout");

        let hit = net::events::GameEvent {
            event: fx::registry::EV_BULLET_HIT_SMALL,
            parm: 0,
            entity_num: 40,
            client_num: -1,
            weapon: 0,
            surf_type: 5, // concrete
            pos: [0.0; 3],
            dir: [0.0; 3],
            other_entity_num: 40,
            attacker_entity_num: -1,
        };
        let ctx = fx::registry::ResolveCtx {
            muzzles: &HashMap::new(),
            weapon_flash: &HashMap::new(),
            view_flash: None,
            view_body: None,
        };
        match fx::registry::resolve(&hit, &ctx).as_slice() {
            [fx::registry::Resolved::Spawn { path, .. }] => assert_eq!(path, "fx/mod/hit.efx"),
            other => panic!("impact table from the new pak: {other:?}"),
        }
    }

    #[test]
    fn own_view_adds_delta_angles_and_flips_pitch() {
        // 90 degrees of yaw from delta alone, 45 more from the mouse; wire
        // pitch 10 degrees down reads as the camera looking down.
        let quarter = 16384;
        let (yaw, pitch, _) = own_view([1820, quarter / 2, 0], [0, quarter, 0]);
        assert!(
            (yaw.to_degrees() - 135.0).abs() < 0.01,
            "yaw {}",
            yaw.to_degrees()
        );
        assert!(
            (pitch.to_degrees() + 10.0).abs() < 0.01,
            "pitch {}",
            pitch.to_degrees()
        );
        // A delta sent as the unsigned 16-bit value wraps like a signed one.
        let (yaw, _, _) = own_view([0, 0, 0], [0, 65536 - quarter, 0]);
        assert!((yaw.to_degrees() + 90.0).abs() < 0.01);
        // Past straight down draws at the clamp, either way.
        for raw_pitch in [quarter - 100, 20_000, -20_000] {
            let (_, pitch, _) = own_view([raw_pitch, 0, 0], [0; 3]);
            assert!(
                (pitch.to_degrees().abs() - 87.89).abs() < 0.01,
                "pitch {}",
                pitch.to_degrees()
            );
        }
    }

    /// The view kick's roll rides cmd.angles[2] into the drawn roll, with
    /// the wire's sign.
    #[test]
    fn own_view_draws_the_kick_roll() {
        let mut input = play::input::PlayInput::default();
        input.kick = [0.0, 4.0, -2.0];
        let (_, _, roll) = own_view(input.cmd_angles(), [0, 0, 0]);
        assert!(
            (roll.to_degrees() + 2.0).abs() < 0.01,
            "roll {}",
            roll.to_degrees()
        );
    }

    #[test]
    fn view_muzzle_offsets_20_forward_4_right_3_down() {
        let cam_pos = Vec3::new(100.0, 200.0, 300.0);
        let forward = Vec3::X;
        let right = Vec3::Y;
        let up = Vec3::Z;
        let (pos, dir) = view_muzzle(cam_pos, forward, right, up);
        assert_eq!(pos, Vec3::new(120.0, 204.0, 297.0));
        assert_eq!(dir, forward);
    }
}
