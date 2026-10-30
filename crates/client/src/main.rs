mod audio;
mod blob_shadow;
mod bloom;
mod camera;
mod clock_probe;
mod cod2_client;
mod cod2_front;
mod cod2_glow;
mod cod2_hud;
mod cod2_light;
mod cod2_names;
mod cod2_player_hud;
mod cod2_players;
mod cod2_scene;
mod cod2_viewmodel;
mod cod2_world_hud;
mod cod4_audio;
mod cod4_client;
mod cod4_crosshair;
mod cod4_damage;
mod cod4_dof;
mod cod4_fly;
mod cod4_front;
mod cod4_fx;
mod cod4_hud;
mod cod4_loading;
mod cod4_look;
mod cod4_menu;
mod cod4_radar;
mod cod4_scoreboard;
mod cod4_spotshadow;
mod cod4_sunshadow;
mod cod4_vehicles;
mod cod4_viewmodel;
mod cod4_voice;
mod cod4_waypoints;
mod console;
mod corona;
mod dlight;
mod edge_probe;
mod entities;
mod entity_light;
mod entity_material;
mod flame_probe;
mod frame_digest;
mod frame_prof;
mod frontend;
mod fx;
mod gamma;
mod head_icon;
mod hud;
mod hud_text;
mod legs_probe;
mod lightmap_blocks;
mod loading;
mod marks;
mod play;
mod post;
mod probe;
mod quick_chat;
mod quit;
mod renderer;
mod scene_copy;
mod screen_blend;
mod session;
mod shadows;
mod shock_probe;
mod sky;
mod soft_particles;
mod ssao;
mod sun_flare;
mod sun_shafts;
mod texture_settings;
mod turret;
mod vehicle_probe;
mod video;
mod viewmodel;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use glam::Vec3;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use vcod_common::asset_source::{AssetSource, RawFile};
use vcod_common::text::atoi_msvc;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use vcod_common::mapworld::{self, MapWorld};
use vcod_common::movetrace::MoveWorld;
use vcod_common::pk3::Pk3Fs;
use vcod_common::{bsp, collision, mesh, model, net, pmove, props, weapon};

use camera::{FlyCamera, InputState};
use renderer::{DynamicModelInstance, Renderer};

/// Window title and the version every surface names: the main menu, the
/// console, F3, the log and a panic report.
const TITLE: &str = concat!("vcod ", env!("VCOD_VERSION"));

/// The GPU backend the renderer picked, once it has one.
static BACKEND: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// [`TITLE`] with the GPU backend, for the window title and the main menu.
fn version_tag() -> String {
    match BACKEND.get() {
        Some(b) => format!("{TITLE} ({b})"),
        None => TITLE.to_string(),
    }
}

#[derive(Parser)]
#[command(about = "Call of Duty (2003) map viewer and client", version = env!("VCOD_VERSION"))]
struct Args {
    /// Map name, e.g. mp_pavlov
    map: Option<String>,
    /// List all maps found in the pk3 search path
    #[arg(long)]
    list: bool,
    /// Game install (holds main/). Defaults to $COD_DIR, else the executable's directory.
    #[arg(long, default_value_os_t = vcod_common::game_dir::default_game_dir())]
    game_dir: std::path::PathBuf,
    /// Pk3 subdirectory: main for CoD1, uo for United Offensive
    #[arg(long, default_value = "main")]
    mod_dir: String,
    /// Which game the install is: cod1, cod2 or cod4. Defaults to what the
    /// game dir holds. cod2 runs fly mode (`vcod <map>`), spectates a server
    /// (`--connect`), probes one headless (`--net-probe`) and, with none of
    /// those, opens its main menu.
    #[arg(long)]
    game: Option<vcod_common::game::Game>,
    /// Load only the install's stock retail paks (the patch's numbered paks,
    /// their `localized_*` ones, UO's map pack) and ignore every other pk3:
    /// third-party maps, `zzz_*` mods, downloads. Downloads nothing, lists no
    /// mods, and Start New Server starts a `--vanilla` server. `vc_vanilla`
    /// reads 1.
    #[arg(long)]
    vanilla: bool,
    /// CoD4: the mod a map loads under, `mods/<name>`: its `mod.ff` and
    /// iwds over the stock ones. A joined server's own `fs_game` wins.
    #[arg(long)]
    fs_game: Option<String>,
    /// Spawn as a soldier and walk (first person) instead of flying
    #[arg(long)]
    walk: bool,
    /// Start with the debug overlay visible (F3 toggles it at runtime)
    #[arg(long)]
    debug_overlay: bool,
    /// Turns on every beyond-retail graphics extra for this run (the `vc_*`
    /// cvars in `RTX_ON`). No ray is traced. The config keeps your own
    /// settings.
    #[arg(long)]
    rtx_on: bool,
    /// Headless: connect to a CoD server, dump the gamestate, then exit
    #[arg(long)]
    net_probe: Option<String>,
    /// Headless, CoD2: play this `.dm_1` demo through the client's demo
    /// path and print what it parsed and drew, a line a second
    #[arg(long)]
    probe_demo: Option<std::path::PathBuf>,
    /// With --probe-demo: play it as `timedemo` does, 50 ms of demo a frame
    #[arg(long)]
    probe_timedemo: bool,
    /// Connect to a CoD server (ip:port) and join it through the stock team
    /// and weapon menus (CoD4: team and class); spectator is one of the team
    /// menu's choices
    #[arg(long)]
    connect: Option<String>,
    /// CoD4: watch a datagram capture (`cod4_probe`'s format, as the
    /// committed `crates/cod4/tests/fixtures/net/play-1.7-*.txt`) on its
    /// own clock, as its client saw it
    #[arg(long)]
    capture: Option<std::path::PathBuf>,
    /// Answer the stock team menu with this after --connect or the console's
    /// `connect`
    #[arg(long)]
    team: Option<String>,
    /// Answer the stock weapon menu with this weapon file name (e.g. m1carbine_mp)
    #[arg(long)]
    weapon: Option<String>,
    /// CoD4: answer the class menu with this response (e.g. assault_mp,0)
    #[arg(long)]
    class: Option<String>,
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
    /// from the step before cannot be recorded under a firing label. Each step
    /// also logs every cmd it sent (`!cmds`, its view and movement included)
    /// and each snapshot its
    /// commandTime, which `playerstate_combat_ab` replays frame for frame. The
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
    /// index, owner, ground, both trajectories and its event ring;
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
    /// rungs and at the foot (`tests/ladder_yawcap_ab.rs`). Retail
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
    probe_vehicle: Option<vehicle_probe::VehicleRole>,
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
    probe_uo_weapon: Option<vehicle_probe::WeaponScript>,
    /// With `--net-probe`: stay a quiet spectator and print a `CLOCK` line a
    /// second, the client clock's lag behind the newest snapshot, how often
    /// it ran past it and how evenly it stepped, beside the same for the
    /// clock it replaced (`OLD`). `--probe-cmd-ms` is the frame time;
    /// `VCOD_NETSIM` adds a bad link. Writes no fixture.
    #[arg(long)]
    probe_clock: bool,
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
    probe_edge: Option<edge_probe::EdgeMode>,
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
    save_slope: bool,
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
    /// with the view swung off it. Tag the fixture `moveshot-<part>-1.5` or
    /// `-uo`; moveshot_ab.rs replays it (cod11-combat.md 2.1).
    #[arg(long, value_enum, value_name = "PART")]
    probe_moveshot: Option<probe::MoveShot>,
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
    /// measures a dead speaker. `{sid}` in COMMAND becomes the current
    /// serverId, so `12:mr {sid} 0 spectator` answers a menu. Every `h`/`i`
    /// chat line that comes back is printed with its control bytes escaped.
    /// Writes no fixture.
    #[arg(long, value_name = "SECS:COMMAND", value_parser = parse_probe_say)]
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
    /// Run without sound (also what happens when no output device opens).
    #[arg(long)]
    no_audio: bool,
    /// `WxH`: a window of that size for this run, whatever `r_mode` and
    /// `r_fullscreen` say, a `vid_restart` included. Sets no cvar, so the
    /// saved config keeps its mode. `tools/cod4_perf.sh` passes its SIZE.
    #[arg(long, value_name = "WxH", value_parser = parse_window_size)]
    window: Option<(u32, u32)>,
    /// Master volume, 0.0 to 1.0: sets `mss_volume`, which the options
    /// screen's slider also sets and the config keeps (default 0.8).
    #[arg(long)]
    volume: Option<f32>,
    /// A console line to run once the window is up, repeatable, in order:
    /// `--exec "set nextdemo quit" --exec "set timedemo 1" --exec "demo x"`
    /// plays a demo as fast as it draws and quits at its end.
    #[arg(long)]
    exec: Vec<String>,
    /// As --exec, run once after the first frame that draws a game:
    /// `--connect ADDR --exec-in-game "record x"` records from the join.
    #[arg(long)]
    exec_in_game: Vec<String>,
    /// `"SECS LINE"`, repeatable, in order: a queue of console lines, each
    /// run SECS seconds after the one before it ran (the first after the
    /// --exec-in-game lines), noted in `VCOD_FRAME_PROFILE` as `# exec`
    /// lines. `tools/cod4_perf.sh`'s sweep toggles one setting a pass
    /// with it (docs/research/cod4-client-perf.md 25).
    #[arg(long)]
    exec_after: Vec<String>,
}

/// The map every mode reads: the renderer builds from it, entities resolve
/// submodels in it. `None` while `--connect` is between maps.
struct World {
    bsp: MapWorld,
    /// The fog a load with no server draws: CoD2 fly mode's map script
    /// `setExpFog` (`vcod_cod2::world::script_fog`).
    fog: Option<net::FogParams>,
    /// A CoD4 map's own materials (`cod4_fly`), which the renderer draws
    /// with in place of the game's library.
    materials: Option<vcod_common::material::MaterialLib>,
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
    /// `ET_FX` replay timers (`fx::looped`).
    looped_fx: fx::looped::LoopedFx,
    /// UO wheel and tread dust marks (`fx::vehicle_trails`).
    vehicle_trails: fx::vehicle_trails::VehicleTrails,
    /// Last frame's `entity_pos`, what prediction clips (docs/research/cod11-player-clip.md).
    drawn_pos: HashMap<u32, Vec3>,
    /// Last frame's interpolated UO vehicle angles: a seat's prediction runs
    /// before this frame's entities are lerped (uo cgame 0x30036070 ahead of
    /// 0x3001f810), so it reads the centity's `lerpAngles` of the frame before.
    drawn_vehicle_angles: HashMap<u32, [f32; 3]>,
    /// A UO four-wheel's turn carried onto its riders' view.
    seat_turn: play::seat::FourWheelTurn,
    /// A UO gunner's eye lag.
    gunner_sway: play::seat::GunnerSway,
    /// UO's turret view sway, on state of its own (0x300dc9c0..0x300dca0c).
    turret_sway: play::seat::GunnerSway,
    /// Last frame's predicted sight fraction in a UO seat, which decides
    /// whether the ridden body draws there (`play::seat::own_body_drawn`).
    seat_frac: f32,
}

/// The live phase off `bsp`, with the weapons configstring 7 (`weapons_cs`)
/// names read off `fs`.
fn live_phase(fs: &Pk3Fs, bsp: &MapWorld, weapons_cs: &str) -> Phase {
    Phase::Live(Box::new(LivePhase::new(
        client_world(fs, bsp),
        live_weapons(fs, weapons_cs),
    )))
}

/// The client's collision map off `bsp`, what `CM_LoadMap` (CoDMP.exe
/// 0x419c90) loads for the cgame.
fn client_world(fs: &Pk3Fs, bsp: &MapWorld) -> collision::CollisionWorld {
    let world = collision::CollisionWorld::build(bsp, &props::collision_tris(fs, &bsp.entities));
    // Predict as the retail Windows client does, not as the server.
    world.set_arith(vcod_common::patch::Arith::for_client());
    // A brush model clips only through its snapshot entity
    // (`pmove::movers::SnapshotMovers::place`), as retail's cgame meets it.
    for model in 1..world.model_count() {
        world.set_model_linked(model, false);
    }
    world
}

/// Read once per cgame: it parses configstring 7 in its weapon setup
/// (0x3000ef10) and `CG_ConfigStringModified` has no case for it
/// (docs/research/cod11-hud-protocol.md, after the configstring table).
fn live_weapons(fs: &Pk3Fs, weapons_cs: &str) -> Vec<Option<weapon::WeaponDef>> {
    vcod_common::weapon_table::from_configstring(fs, weapons_cs)
}

impl LivePhase {
    fn new(world: collision::CollisionWorld, weapons: Vec<Option<weapon::WeaponDef>>) -> LivePhase {
        LivePhase {
            world,
            weapons,
            scene: entities::EntityScene::new(),
            events: net::events::EventTracker::new(),
            predicted_events: play::events::PredictedEvents::default(),
            clock: play::clock::ServerClock::default(),
            last_loop_snap: None,
            looped_fx: fx::looped::LoopedFx::default(),
            vehicle_trails: fx::vehicle_trails::VehicleTrails::default(),
            drawn_pos: HashMap::new(),
            drawn_vehicle_angles: HashMap::new(),
            seat_turn: play::seat::FourWheelTurn::default(),
            gunner_sway: play::seat::GunnerSway::default(),
            turret_sway: play::seat::GunnerSway::default(),
            seat_frac: 0.0,
        }
    }

    /// An in-game `vid_restart`'s fresh cgame over the engine's state:
    /// the collision map (`world`) and `cl.serverTime` (`clock`) stay, the
    /// rest starts over and configstring 7 is parsed again as it stands
    /// (docs/research/cod11-front-end.md section 17, "The cgame restart").
    fn restart_cgame(self, weapons: Vec<Option<weapon::WeaponDef>>) -> LivePhase {
        LivePhase {
            clock: self.clock,
            ..LivePhase::new(self.world, weapons)
        }
    }
}

enum Mode {
    /// No map and no server: the main menu, or the full-screen console when
    /// no menu is up, as retail's is when disconnected.
    Idle,
    /// `vcod <map>`: a free camera over a CoD1 or CoD2 map, no server.
    Fly(FlyCamera),
    /// A CoD2 session, online or a demo (`App::session`'s `Cod2Client`):
    /// the camera follows its frames, or flies until the first one.
    Cod2(FlyCamera),
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
    },
    Walk {
        /// Boxed to keep the variants a similar size.
        world: Box<collision::CollisionWorld>,
        /// Boxed to keep the variants a similar size.
        ps: Box<pmove::PlayerState>,
        input: pmove::PmInput,
        keys: WalkKeys,
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
        /// Press edges latched until the next redraw; `ads_held`/`fire_held` are level.
        fire_edge: bool,
        fire_held: bool,
        reload_edge: bool,
        ads_held: bool,
        /// The gun's sway, which a scope overlay is centred on.
        gun_aim: hud::scope::GunAim,
    },
}

impl Mode {
    /// `vid_restart` leaves the offline viewer's sounds playing; a game
    /// (CoD1, or CoD2 in docs/research/cod2-sound.md section 9) stops them.
    fn keeps_sounds_on_vid_restart(&self) -> bool {
        matches!(self, Mode::Fly(_) | Mode::Walk { .. })
    }
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

/// The free camera's frame once its view is set, `Mode::Fly`'s and
/// `Mode::Cod2`'s alike: the listener, the effects' step and draw, CoD2's
/// clouds and world HUD, then the frame.
struct ViewTail<'a> {
    fs: &'a Pk3Fs,
    shell: &'a console::shell::Shell,
    audio: &'a mut audio::AudioSystem,
    fx: &'a mut fx::sim::FxSystem,
    session: &'a session::Session,
    r: &'a mut Renderer,
    time: f32,
    dt: f32,
    aspect: f32,
    cull: renderer::CullMode,
}

impl ViewTail<'_> {
    /// `view` is the draw's angles, fov and view weapon, `fly_fov` the
    /// camera's own; `fx_ms` takes the effects' time.
    fn run(
        self,
        cam: &FlyCamera,
        view: cod2_client::DrawView,
        fly_fov: f32,
        hud_quads: &mut Vec<hud::HudQuad>,
        fx_ms: &mut f32,
    ) -> (renderer::Frame, Option<renderer::VmDraw>) {
        let ViewTail {
            fs,
            shell,
            audio,
            fx,
            session,
            r,
            time,
            dt,
            aspect,
            cull,
        } = self;
        let cod2_client::DrawView {
            yaw: draw_yaw,
            pitch: draw_pitch,
            roll: draw_roll,
            fov: world_fov,
            vm: cod2_vm,
        } = view;
        let (cam_forward, cam_right, cam_up) = camera::basis(draw_yaw, draw_pitch);
        // u32::MAX: no entity pinned to the camera.
        audio.set_listener(cam.pos, cam_forward, cam_right, cam_up, u32::MAX);
        // no CollisionWorld in fly mode, so usePhysics fx don't collide
        let fx_t0 = Instant::now();
        let mut prof = frame_prof::span(frame_prof::Stage::FxStep);
        fx.step(dt, time, None);
        if session.live() {
            // Delayed `playfx` and the hits the step raised.
            fx.spawn_chained(fs, time);
        }
        prof.next(frame_prof::Stage::Sound);
        audio.step(&session.entity_positions(), &session.entity_angles(), None);
        prof.next(frame_prof::Stage::FxDraw);
        // `fx_draw 0` runs the effects and draws none of them
        let draw_fx = shell.cvar_f32("fx_draw") != 0.0;
        let quads = match draw_fx {
            true => fx.build_quads(cam.pos, cam_right, cam_up, time),
            false => Vec::new(),
        };
        r.set_fx_quads(fs, quads, time);
        // CoD2's snow (fx::cod2_cloud), tested against the map's roofs
        let clouds = match draw_fx {
            true => fx.clouds(cam.pos, cam_right, cam_up, time),
            false => Default::default(),
        };
        r.set_fx_clouds(fs, session.map_name(), &clouds, time);
        r.set_marks(
            fs,
            &fx.take_marks(time),
            &[],
            (time * 1000.0) as i32,
            time,
            shell.cvar_f32("cg_marks") != 0.0,
        );
        let mut lights = fx.lights(cam.pos, time);
        // CoD4's `r_dlightLimit` (0, 2 or 4 in the menu, at most 4: iw3mp.exe
        // 0x62b34d); vcod keeps the nearest, retail the most important.
        if let Some(limit) = shell.cvar("r_dlightLimit") {
            lights.truncate(limit.trim().parse::<usize>().unwrap_or(4).min(4));
        }
        r.set_fx_lights(&lights);
        drop(prof);
        *fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;
        let view_proj =
            if (draw_yaw, draw_pitch, draw_roll, world_fov) == (cam.yaw, cam.pitch, 0.0, fly_fov) {
                cam.view_proj(fly_fov, aspect)
            } else {
                camera::view_proj_from(cam.pos, draw_yaw, draw_pitch, draw_roll, world_fov, aspect)
            };
        // CoD2's waypoints and head icons, under the 2D HUD.
        session.world_hud(
            hud_quads,
            view_proj,
            (draw_yaw, draw_pitch, draw_roll),
            r.screen_size(),
        );
        (
            renderer::Frame {
                view_proj,
                eye: cam.pos,
                fwd: cam_forward,
                up: Vec3::Z,
                time,
                fov_x: world_fov,
                cull,
                hud_lines: Vec::new(),
            },
            cod2_vm,
        )
    }
}

/// F3 overlay text, top line first. A free function because the caller holds
/// the renderer borrowed out of `App`.
#[allow(clippy::too_many_arguments)]
fn hud_lines(
    mode: &Mode,
    // `Mode::Cod2`'s camera tag: the game the session plays.
    cam_tag: &str,
    // The online entity scene; fly/walk have none.
    scene: Option<&entities::EntityScene>,
    stats: &hud_text::HudStats,
    // (build_instances, render, fx step+build_quads) ms
    cpu_ms: (f32, f32, f32),
    // (events drained, of those unrecognized)
    ev_counts: (u64, u64),
    // (particles, pending marks, lights)
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
        version_tag(),
        format!(
            "fps {fps:5.1}  ({:5.2} ms, worst {:5.1})",
            stats.dt_smooth * 1000.0,
            stats.worst_ms
        ),
        format!("cpu ms: build {build_ms:5.2}  render {render_ms:5.2}"),
    ];
    let (fx_particles, _, fx_lights) = fx_counts;
    let marks = r.mark_count();
    lines.push(format!(
        "fx p{fx_particles} m{marks} l{fx_lights} {fx_ms:.2}ms"
    ));
    let (draws, insts, bones) = r.debug_counts();
    let lod = r.lod_counts();
    lines.push(format!(
        "draw: world {draws}  inst {insts}  bones {bones}  lod {}/{}/{} gone {} tris {}",
        lod.per_lod[0], lod.per_lod[1], lod.per_lod[2], lod.gone, lod.tris
    ));
    lines.push(r.ssao_line());
    lines.push(r.sun_shadow_line());
    lines.push(r.spot_shadow_line());
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
    let (stage_draws, dlight_draws, dropped, warns) = r.shader_stats();
    lines.push(format!(
        "shader: sky {}  stages {stage_draws}  dlight {dlight_draws}  dropped {dropped}  warns {warns}",
        r.sky_name().unwrap_or("none")
    ));
    lines.push(format!("fog: {}", r.fog_debug()));
    if let Some(line) = r.shadow_debug() {
        lines.push(line);
    }
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
        Mode::Cod2(cam) => lines.push(cam_line(cam_tag, cam.pos, cam.yaw, cam.pitch)),
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
    let started = Instant::now();
    console::log::init();
    frame_digest::init();
    frame_prof::init();
    let args = Args::parse();
    log::info!("{TITLE}{}", if args.vanilla { " (vanilla)" } else { "" });
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("{TITLE} panicked");
        default_panic(info);
    }));
    quit::install();
    use vcod_common::game::Game;
    let game = Game::resolve(args.game, &args.game_dir);
    if let Some(addr) = args.connect.as_ref().filter(|_| args.net_probe.is_none()) {
        vcod_common::laps::begin_load_at(format!("{game:?} start --connect {addr}"), started);
    }
    // Where 1.1's `main/codkey` sits (`net::cdkey`).
    net::cdkey::set_install(&args.game_dir);
    // Where a CoD4 join reads and writes its profile's stats.
    play::cod4_profile::set_context(&args.game_dir, "");
    // CoD2 runs in fly mode only: a map with no walk, server, probe or list.
    let cod2_fly = game == Game::Cod2
        && args.map.is_some()
        && !args.walk
        && args.connect.is_none()
        && args.net_probe.is_none()
        && !args.list;
    // CoD4 flies over a stock map's zone (cod4_fly), or watches a capture
    // or a server (play::cod4) over the map its gamestate names.
    let cod4_watch = game == Game::Cod4
        && (args.capture.is_some() || args.connect.is_some())
        && !args.walk
        && args.net_probe.is_none()
        && !args.list;
    let cod4_fly = game == Game::Cod4
        && args.map.is_some()
        && !args.walk
        && !cod4_watch
        && args.net_probe.is_none()
        && !args.list;
    // `--connect` to a CoD2 server spectates or plays (play::cod2), drawn
    // in fly mode.
    let cod2_connect = game == Game::Cod2
        && args.connect.is_some()
        && args.net_probe.is_none()
        && !args.walk
        && !args.list;
    if game == Game::Cod2
        && let Some(path) = &args.probe_demo
    {
        vcod_cod2::model::register();
        let dir = args.game_dir.join(&args.mod_dir);
        let fs = vcod_common::asset_source::open_game_fs(game, &dir, None)
            .with_context(|| format!("cannot open game data in {}", dir.display()))?;
        return play::cod2::demo_probe(path, &fs, args.probe_timedemo);
    }
    if game == Game::Cod4
        && let Some(addr) = &args.net_probe
    {
        let addr = std::net::ToSocketAddrs::to_socket_addrs(addr.as_str())?
            .next()
            .with_context(|| format!("cannot resolve {addr}"))?;
        let cp = vcod_cod4::net::conn::Cp::Iwds(args.game_dir.join("main"));
        // Headless, a team that plays takes a class even with none named.
        let team = args.team.clone().or(args.probe_team.clone());
        let class = args.class.clone().or_else(|| {
            team.as_deref()
                .filter(|t| *t != "spectator")
                .map(|_| play::cod4_menus::DEFAULT_CLASS.to_string())
        });
        let join = play::cod4_menus::Join { team, class };
        return play::cod4::probe(
            addr,
            args.probe_secs,
            "vcodprobe",
            cp,
            join,
            Some(&args.game_dir),
        );
    }
    if game == Game::Cod2
        && let Some(addr) = &args.net_probe
    {
        vcod_cod2::model::register();
        let dir = args.game_dir.join(&args.mod_dir);
        let fs = vcod_common::asset_source::open_game_fs(game, &dir, None)
            .with_context(|| format!("cannot open game data in {}", dir.display()))?;
        let join = args.probe_team.clone().map(|team| play::cod2::Join {
            team,
            weapon: args.probe_weapon.clone(),
        });
        return play::cod2::probe(addr, args.probe_secs, &fs, join);
    }
    // Bare `vcod --game cod2` opens CoD2's main menu (frontend::cod2).
    let cod2_menu = game == Game::Cod2
        && args.map.is_none()
        && args.connect.is_none()
        && args.net_probe.is_none()
        && !args.walk
        && !args.list;
    let cod2 = cod2_fly || cod2_connect || cod2_menu;
    // Bare `vcod` on a CoD4 install opens 1.7's main menu (cod4_front).
    let cod4_menu = game == Game::Cod4
        && args.map.is_none()
        && !cod4_watch
        && args.net_probe.is_none()
        && !args.walk
        && !args.list;
    if !cod2 && !cod4_fly && !cod4_watch && !cod4_menu {
        game.ensure_supported()?;
    }

    let dir = args.game_dir.join(&args.mod_dir);

    if let Some(addr) = &args.net_probe
        && args.probe_clock
    {
        return clock_probe::run(addr, args.probe_secs, args.probe_cmd_ms);
    }
    if let (Some(addr), Some(mode)) = (&args.net_probe, args.probe_edge) {
        return edge_probe::run(addr, mode, args.probe_secs, args.probe_cmd_ms);
    }
    if let Some(addr) = &args.net_probe {
        // Sound cue resolution needs the game data, the wire-level prints do
        // not, so a failed open only costs the audio line.
        let fs = match Pk3Fs::open_search(&dir, None, None, args.vanilla) {
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
                slope_every_state: args.slope_every_state,
                slope_from_spawn: args.slope_from_spawn,
                prone: args.probe_prone,
                moves: args.probe_moves,
                turns: args.probe_turns,
                sprint: args.probe_sprint,
                autofire: args.probe_autofire,
                talk: args.probe_talk,
                moveshot: args.probe_moveshot,
                uofoot: args.probe_uofoot,
                uoturret: args.probe_uoturret,
                crouchjump: args.probe_crouchjump,
                crouchjump_stand: args.probe_crouchjump_stand,
                patchwalk: args.probe_patchwalk,
                combat: args.save_combat,
                ads: args.save_ads,
                grenade: args.save_grenade,
                sway: args.probe_sway,
                lean: args.save_lean,
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
                legs: args.save_legs,
                shock: args.save_shock,
                bump_target: args.probe_bump_target,
                follow: args.probe_follow,
                killcam: args.probe_killcam,
                killcam_skip_ms: args.probe_killcam_skip_ms,
                kill_cmds: args.probe_kill_cmds.clone(),
                fall: args.probe_fall,
                fall_walk: args.probe_fall_walk,
                fall_prone: args.probe_fall_prone,
                pitch_flip: args.probe_pitch_flip,
                yaw: args.probe_yaw,
                ride: args.probe_ride,
                items: args.probe_items,
                compass: args.probe_compass,
                flame: args.probe_flame,
                ladder: args.probe_ladder,
                save_ladder: args.save_ladder.clone(),
                weapsel: args.probe_weapsel,
                vehicle: args.probe_vehicle,
                uo_weapon: args.probe_uo_weapon,
                scripted: args.save_scripted.clone(),
                download: args.probe_download.clone(),
                rate: args.probe_rate.clone(),
                snaps: args.probe_snaps.clone(),
                name: args.probe_name.clone(),
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

    let mut fs = if cod4_fly || cod4_watch || cod4_menu {
        cod4_fly::open_fs(&args.game_dir, args.fs_game.as_deref(), args.map.as_deref())
    } else if cod2 {
        vcod_cod2::model::register();
        vcod_common::asset_source::open_game_fs(game, &dir, None)
    } else {
        Pk3Fs::open_search(&dir, None, None, args.vanilla)
    }
    .with_context(|| format!("cannot open game data in {}", dir.display()))?;

    if args.list {
        for name in fs.find_maps() {
            println!("{name}");
        }
        return Ok(());
    }

    // The config's binds and cvars, or the defaults when there is none yet.
    // On CoD2 the active profile's (`players/active.txt`) holds it, seeded
    // from the mod directory's until the profile has one of its own
    // (cod2-fonts-menus.md 4.1).
    let profile = cod2
        .then(|| frontend::profiles::active_profile(&dir.join("players")))
        .flatten();
    // CoD4's sits in the active profile (`players/profiles/active.txt`),
    // seeded from the `config_mp.cfg` 1.7 wrote there until vcod has
    // written its own (cod4-front-end.md 3).
    let cod4_profile = (game == Game::Cod4)
        .then(|| play::cod4_menus::front::active_profile(&args.game_dir))
        .flatten()
        .and_then(|p| play::cod4_menus::front::profile_dir(&args.game_dir, &p));
    let config_path = match (&profile, &cod4_profile) {
        (Some(p), _) => dir.join("players").join(p).join(CONFIG_FILE),
        (None, Some(d)) => d.join(CONFIG_FILE),
        (None, None) => dir.join(CONFIG_FILE),
    };
    let mut shell = console::shell::Shell::new();
    let retail_config = cod4_profile
        .as_ref()
        .map(|d| d.join("config_mp.cfg"))
        .filter(|p| p.exists());
    let read_path = if config_path.exists() {
        config_path.clone()
    } else if let Some(p) = retail_config {
        p
    } else {
        dir.join(CONFIG_FILE)
    };
    let config_text = std::fs::read_to_string(&read_path).ok();
    if let Some(text) = &config_text {
        for effect in shell.exec_config(text) {
            if let console::shell::Effect::Print(line) = effect {
                log::warn!("{CONFIG_FILE}: {line}");
            }
        }
    }
    // UO writes its own `uoconfig_mp.cfg` from a `default_mp.cfg` that binds
    // ALT to `+sprint`; a UO config vcod wrote before it had the sprint
    // has no such line, so it gets the default where ALT is free
    // (docs/research/coduo-movement.md, 8).
    if net::protocol::install_protocol(&fs).is_uo() && shell.bind_if_free("ALT", "+sprint") {
        log::info!("{CONFIG_FILE}: bound ALT to +sprint, United Offensive's default");
    }
    if fs.vanilla() {
        shell.set_rom("vc_vanilla", "1");
    }
    shell.register_aa_samples();
    shell.register_depth_prepass(game != Game::Cod1);
    // CoD4's stock keys are CoD2's where vcod has them (frag, smoke,
    // `toggleads`, `+melee_breath`).
    if game == Game::Cod4 {
        shell.register_cod4_options();
        let n = shell.use_cod4_defaults(config_text.is_some());
        log::debug!("{CONFIG_FILE}: {n} keys take CoD2's and CoD4's default binds for CoD4");
    }
    if game == Game::Cod2 {
        shell.use_cod2_cvars();
        if let Some(p) = &profile {
            shell.execute(&format!("set com_playerProfile \"{p}\""));
        }
        let n = shell.use_cod2_binds();
        log::debug!("{CONFIG_FILE}: {n} keys take CoD2's default binds");
    }
    // `cl_language` registers after the config ran, taking its value.
    shell.apply_latched();
    // vcod's old defaults a config still holds go (cod11-console.md 8).
    let (reset, upgraded) = shell.upgrade_config(game);
    for name in &reset {
        log::info!(
            "{CONFIG_FILE}: {name} was an old vcod default, now {:?}",
            shell.cvar(name).unwrap_or("")
        );
    }
    if upgraded
        && config_path.exists()
        && let Err(e) = std::fs::write(&config_path, shell.config_text())
    {
        log::warn!("cannot write {}: {e}", config_path.display());
    }
    if let Some(v) = args.volume {
        shell.execute(&format!("set mss_volume {v}"));
    }
    if args.rtx_on {
        for (name, value) in RTX_ON {
            shell.set_for_run(name, value);
        }
    }

    let mut cod2_fs_game = None;
    let cod2_session = match &args.connect {
        Some(addr) if cod2_connect => {
            let t0 = Instant::now();
            let mut s = play::cod2::Cod2Session::connect(
                addr,
                &cod2_front::connect_opts(&shell, &fs),
                std::time::Duration::from_secs(15),
            )?;
            // The gamestate's `fs_game` and downloads, before the map loads.
            if s.take_reopen(None) {
                cod2_fs_game = s.fs_game().map(str::to_string);
                if let Some(g) = &cod2_fs_game {
                    shell.force("fs_game", g);
                }
                fs = play::cod2::open_search(&args.game_dir, cod2_fs_game.as_deref())?;
            }
            s.join = args.team.clone().map(|team| play::cod2::Join {
                team,
                weapon: args.weapon.clone(),
            });
            vcod_common::laps::span(t0, || "cod2 handshake to the gamestate".into());
            Some(Box::new(s))
        }
        _ => None,
    };
    // A live CoD4 join runs beside the window, which draws the connect
    // screen meanwhile (`cod4_loading`).
    let mut cod4_loading = None;
    let cod4_session = match (&args.capture, &args.connect) {
        _ if !cod4_watch => None,
        (Some(path), _) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            Some(play::cod4::Cod4Session::open_capture(&text)?)
        }
        (None, Some(addr)) => {
            let server = addr.clone();
            let addr = std::net::ToSocketAddrs::to_socket_addrs(addr.as_str())?
                .next()
                .with_context(|| format!("cannot resolve {addr}"))?;
            let name = shell.cvar("name").unwrap_or("vcod").to_string();
            // `cl_allowDownload` and `cl_wwwDownload`, on by default as
            // 1.7's (cod4-handshake.md 11).
            let allow = shell.cvar("cl_allowDownload").is_none_or(|v| v != "0");
            let www = shell.cvar("cl_wwwDownload").is_none_or(|v| v != "0");
            cod4_loading = Some(cod4_loading::Loader::start(cod4_loading::JoinArgs {
                addr,
                server,
                name,
                password: shell.cvar("password").unwrap_or("").to_string(),
                join: play::cod4_menus::Join {
                    team: args.team.clone(),
                    class: args.class.clone(),
                },
                game_dir: args.game_dir.clone(),
                allow_download: allow,
                www_download: www,
            }));
            None
        }
        (None, None) => None,
    };
    let net_client = match &args.connect {
        Some(_) if cod2_connect || cod4_watch => None,
        Some(addr) => {
            let mut net = net::NetClient::connect_from(addr, net::protocol::install_protocol(&fs))
                .with_context(|| format!("cannot open a socket to {addr}"))?;
            net.set_userinfo(shell.userinfo());
            Some(net)
        }
        None => None,
    };

    // Fly and walk need their map up front; online learns it from the
    // server inside the loop and loads through the same path as a map change.
    // With neither, the window opens on the console.
    // A CoD2 server names its map in the gamestate the session waited for.
    let map_arg = match &cod2_session {
        _ if let Some(s) = &cod4_session => {
            Some(s.map_name().context("cod4: no mapname in serverinfo")?)
        }
        Some(s) => Some(
            s.map_name()
                .context("cod2: no mapname in serverinfo")?
                .to_string(),
        ),
        None => args.map.clone(),
    };
    // The session is read on its own thread until the window's first frame
    // takes it back (play::cod2::Pump): the load can outlast `sv_timeout`.
    let cod2_pump = cod2_session.map(|s| s.pump());
    let mut cod4_map = None;
    let local = if (cod4_fly || cod4_watch)
        && let Some(map) = map_arg.as_deref()
    {
        let ents = cod4_session
            .as_ref()
            .map(|s| s.entity_assets())
            .unwrap_or_default();
        // A joined server's `fs_game` (configstring 1) over `--fs-game`:
        // its mod's zone and iwds load before the map (cod4-fastfile.md 13).
        let fs_game = match &cod4_session {
            Some(s) => s.fs_game(),
            None => args.fs_game.clone(),
        };
        // Under a mod the level's `usermaps/<map>/` iwds join it too.
        if fs_game != args.fs_game || fs_game.is_some() {
            fs = cod4_fly::open_fs(&args.game_dir, fs_game.as_deref(), Some(map))?;
        }
        let mut c = cod4_fly::load(&args.game_dir, fs_game.as_deref(), map, &ents)
            .with_context(|| format!("cannot load CoD4 map '{map}'"))?;
        for (path, wav) in std::mem::take(&mut c.sound_files) {
            fs.overlay(&path, wav);
        }
        let bsp = std::mem::take(&mut cod4_map.insert(c).bsp);
        Some((map.to_string(), bsp))
    } else if net_client.is_none()
        && let Some(map) = map_arg.as_deref()
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
        let t0 = Instant::now();
        let data = fs
            .read(&path)
            .with_context(|| format!("cannot read {path}"))?;
        let bsp = if cod2 {
            vcod_cod2::world::from_bytes(&data)
        } else {
            mapworld::load(&data)
        }
        .with_context(|| format!("cannot parse {path}"))?;
        vcod_common::laps::span(t0, || format!("{map}: read and parse"));
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
    let audio_opts = audio::AudioOpts {
        enabled: !args.no_audio,
        volume: shell.cvar_f32("mss_volume"),
        rate: audio::bank::mixer_rate(shell.cvar_f32("mss_khz") as i32),
    };
    // CoD4's aliases come off its zones (`cod4_audio`) once the session
    // runs; its search path has no `soundaliases/`.
    let mut audio = match game {
        Game::Cod4 => audio::AudioSystem::with_aliases(&fs, audio_opts, Default::default()),
        _ => audio::AudioSystem::new(&fs, audio_opts),
    };
    if !audio.enabled() {
        println!("audio: silent (no output device, or --no-audio)");
    }

    // Fly mode over a CoD4 map: its effects, sounds, fog and vision set
    // without a session.
    let cod4_fog = cod4_map
        .as_ref()
        .and_then(|c| c.look.fly_fog)
        .map(|f| f.params());
    let cod4_fly = match cod4_session {
        None => cod4_map.as_mut().and_then(|c| {
            let look = std::mem::take(&mut c.look);
            Some(cod4_fly::FlyAv::new(c.av.take()?, c.ambient.take(), look))
        }),
        Some(_) => None,
    };
    let cod4_ents = cod4_map
        .as_mut()
        .map(|c| {
            (
                std::mem::take(&mut c.models),
                std::mem::take(&mut c.weapons),
                c.bodies.take(),
                c.av.take(),
                c.world.take(),
                std::mem::take(&mut c.look),
                std::mem::take(&mut c.parts),
            )
        })
        .unwrap_or_default();
    let (mode, world, title) = if let Some(net) = net_client {
        (
            online_mode(net, args.team.clone(), args.weapon.clone()),
            None,
            format!("{} — connecting", version_tag()),
        )
    } else if let Some((map, bsp)) = local {
        audio.on_gamestate(&map);
        // CoD2 runs no script in fly mode: the map's own fog call stands in
        let fog = if cod2_fly {
            fs.load::<RawFile>(&format!("maps/mp/{map}.gsc"))
                .ok()
                .and_then(|g| vcod_cod2::world::script_fog(&g.text()))
        } else {
            cod4_fog
        };
        if let Some(f) = fog {
            log::info!("{map}: script fog {f:?}");
        }
        // No server to send configstring 3; every stock MP map ships an
        // `ambient_<map>` alias (iw_sound.csv), unknown names just stay silent.
        // CoD2's are named by the map script's `ambientPlay`.
        let ambient = match cod2_fly {
            true => fs
                .load::<RawFile>(&format!("maps/mp/{map}.gsc"))
                .ok()
                .and_then(|g| play::cod2_sound::script_ambient(&g.text()))
                .unwrap_or_default(),
            false => format!("ambient_{map}"),
        };
        audio.set_ambient(&fs, Some(&ambient));
        let mode = if args.walk {
            walk_mode(&map, &bsp, &fs, view_weapon, rigs)?
        } else {
            let spawn = match &cod4_map {
                Some(c) => Some(c.spawn),
                None => bsp::find_spawn(&bsp.entities),
            };
            let cam = match spawn {
                Some((origin, yaw)) => FlyCamera::new(Vec3::from(origin) + Vec3::Z * 60.0, yaw),
                None => {
                    let (min, max) = mesh::map_bounds(&bsp);
                    let center = (Vec3::from(min) + Vec3::from(max)) * 0.5;
                    log::warn!("no player spawn found, starting at the center of the map");
                    FlyCamera::new(center, 0.0)
                }
            };
            // `--connect` to a CoD2 server: the session is on its pump. A
            // watched CoD4 session drives the camera the same way.
            match cod2_pump.is_some() || cod4_session.is_some() {
                true => Mode::Cod2(cam),
                false => Mode::Fly(cam),
            }
        };
        (
            mode,
            Some(World {
                bsp,
                fog,
                materials: cod4_map.take().map(|c| c.materials),
            }),
            format!("{} — {map}", version_tag()),
        )
    } else {
        (Mode::Idle, None, version_tag())
    };

    // CoD4 plays its zones' effects (`cod4_fx`); its search path has no
    // `fx/*.csv`.
    if game != Game::Cod4 {
        fx::registry::init(&fs);
    }

    let console = if game == Game::Cod4 {
        let font = fs.dirs().first().and_then(|d| cod4_hud::console_font(d));
        if font.is_none() {
            log::warn!("console: the zones lack fonts/consoleFont, drawing no text");
        }
        console::Console::with_font(font)
    } else {
        console::Console::new(&fs)
    };
    let language = shell.cvar("cl_language").unwrap_or("").to_string();
    // Retail keeps the favourites beside CoDMP.exe; vcod shares the file.
    let ui = match game {
        Game::Cod4 => frontend::Ui::without_menus(&fs),
        _ => new_ui(&fs, &args.game_dir, &args.mod_dir, &language),
    };
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        title,
        world,
        game,
        fs,
        game_dir: args.game_dir.clone(),
        mod_dir: args.mod_dir.clone(),
        fs_game: cod2_fs_game,
        fs_pure: None,
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
        flame: fx::flame::FlameSystem::default(),
        client_bullets: fx::client_bullet::ClientBullets::default(),
        start: Instant::now(),
        last_frame: Instant::now(),
        frame_ms: 0,
        debug_overlay: args.debug_overlay,
        cull_mode: renderer::CullMode::On,
        hud_stats: hud_text::HudStats::new(),
        interp_misses: 0,
        ev_seen: 0,
        ev_unknown: 0,
        foreign_events: HashSet::new(),
        build_ms: 0.0,
        render_ms: 0.0,
        fx_ms: 0.0,
        look_zoom: (1.0, false),
        modifiers: Default::default(),
        mouse: Default::default(),
        ignore_hw_gamma: false,
        textures: texture_settings::TextureSettings::default(),
        texture_mode_seen: String::new(),
        hud,
        hud_ms: 0.0,
        localized,
        audio,
        quick_chat: quick_chat::QuickChat::new(0x51ee),
        ui,
        ui_pending: Vec::new(),
        cod4_fly,
        cod4_loading,
        window_size: args.window,
        exec: args.exec.clone(),
        exec_in_game: args.exec_in_game.clone(),
        exec_after: exec_after_queue(&args.exec_after),
        exec_after_last: None,
        soak: play::cod1_soak::Soak::from_env(),
        menu_mouse: false,
        os_cursor: true,
        captured: false,
        server_process: None,
        cod4_parked: None,
        rcon: Default::default(),
        cod4_rcon: Default::default(),
        timedemo: Default::default(),
        exec_depth: 0,
        error: None,
        session: match (cod2, cod4_session) {
            (_, Some(s)) => session::Session::Cod4(Box::new(cod4_client::Cod4Client::new(
                s,
                cod4_ents.0,
                cod4_ents.6,
                cod4_ents.1,
                cod4_ents.2,
                cod4_ents.3,
                cod4_ents.4,
                cod4_ents.5,
            ))),
            (true, None) => {
                session::Session::Cod2(Box::new(cod2_client::Cod2Client::new(cod2_pump)))
            }
            (false, None) if cod4_menu => session::Session::Cod4Menu(Box::new(
                cod4_front::Cod4Front::new(args.game_dir.clone()),
            )),
            (false, None) => session::Session::Cod1,
        },
        wheel: console::keys::WheelNotches::default(),
    };
    if matches!(app.mode, Mode::Idle) && app.cod4_loading.is_none() {
        app.enter_menu(None);
    }
    let ran = event_loop.run_app(&mut app);
    frame_prof::flush();
    if let Some(soak) = &mut app.soak {
        soak.flush();
    }
    ran?;
    // Retail's quit goes through `CL_Disconnect`. Without the `disconnect`
    // the server holds the slot, and anything it owns, until `sv_timeout`.
    if let Mode::Online { net, .. } = &mut app.mode {
        net.disconnect();
    }
    app.session.shutdown();
    match app.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// vcod's config, beside retail's `config_mp.cfg` in the mod directory and
/// written the same way (`Shell::config_text`).
pub(crate) const CONFIG_FILE: &str = "vcod_mp.cfg";

/// What `--rtx-on` sets: each vcod-only graphics extra at its on value.
const RTX_ON: [(&str, &str); 10] = [
    ("vc_bicubic", "1"),
    ("vc_bloom", "1"),
    ("vc_shadows", "1"),
    ("vc_ssao", "1"),
    ("vc_sunShafts", "1"),
    ("vc_softParticles", "1"),
    ("vc_hdr", "1"),
    ("vc_grade", "1"),
    ("vc_vignette", "1"),
    ("vc_grain", "1"),
];

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

/// What the next renderer loads and samples images with: the latched
/// The cvars the samplers follow, as one key: `r_textureMode` and CoD4's
/// mip mode and anisotropy pair.
fn texture_mode_key(shell: &console::shell::Shell) -> String {
    [
        "r_textureMode",
        "r_texFilterMipMode",
        "r_texFilterAnisoMin",
        "r_texFilterAnisoMax",
    ]
    .map(|n| shell.cvar(n).unwrap_or_default())
    .join("|")
}

/// `filter` under CoD4's `r_texFilterMipMode` (iw3mp.exe 0x62a005), which
/// forces the mip blend over `r_textureMode`, and the anisotropy pair
/// (`TextureFilter::cod4_aniso`); unchanged off CoD4.
fn cod4_filter(
    shell: &console::shell::Shell,
    filter: texture_settings::TextureFilter,
) -> texture_settings::TextureFilter {
    let Some(mip_mode) = shell.cvar("r_texFilterMipMode") else {
        return filter;
    };
    let forced = match mip_mode {
        m if m.eq_ignore_ascii_case("Force Bilinear") => "GL_LINEAR_MIPMAP_NEAREST",
        m if m.eq_ignore_ascii_case("Force Trilinear") => "GL_LINEAR_MIPMAP_LINEAR",
        _ => "",
    };
    let filter = texture_settings::TextureFilter::by_name(forced).unwrap_or(filter);
    let aniso = |n| atoi_msvc(shell.cvar(n).unwrap_or("1"));
    filter.cod4_aniso(aniso("r_texFilterAnisoMin"), aniso("r_texFilterAnisoMax"))
}

/// `r_picmip`, `r_picmip2`, `r_texturebits`, `r_simpleMipMaps` and
/// `r_weightMipMaps`, and `r_textureMode`, whose unknown names keep `prev`;
/// on CoD4 its picmip dvars and [`cod4_filter`].
fn texture_cvars(
    shell: &console::shell::Shell,
    prev: texture_settings::TextureFilter,
) -> texture_settings::TextureSettings {
    let mut t = texture_settings::TextureSettings::from_cvars(
        [
            "r_picmip",
            "r_picmip2",
            "r_texturebits",
            "r_simpleMipMaps",
            "r_weightMipMaps",
        ]
        .map(|n| shell.cvar_f32(n)),
        shell.cvar("r_textureMode").unwrap_or_default(),
        prev,
    );
    // CoD4's picmip dvars, mip mode and anisotropy (cod4-front-end.md 9)
    if shell.cvar("r_picmip_bump").is_some() {
        t.set_cod4_picmip(
            ["r_picmip", "r_picmip_bump", "r_picmip_spec"].map(|n| shell.cvar_f32(n)),
        );
        t.filter = cod4_filter(shell, t.filter);
    }
    t
}

/// `vc_hdr` as latched: the scene format the next renderer builds with.
fn hdr_cvar(shell: &console::shell::Shell) -> bool {
    shell.cvar_f32("vc_hdr") != 0.0
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

/// `Com_Frame`'s shortest frame in ms (CoDMP.exe 0x438040): `1000 /
/// com_maxfps` when it is above 0, else 1. Nothing lifts it, `timedemo`
/// included; the `dedicated` test never holds on a client.
fn frame_min_ms(maxfps: i32) -> u64 {
    if maxfps > 0 {
        (1000 / maxfps) as u64
    } else {
        1
    }
}

/// `--exec-after`'s `"SECS LINE"` arguments as a queue; one whose SECS
/// does not parse runs at once.
fn exec_after_queue(args: &[String]) -> std::collections::VecDeque<(f32, String)> {
    args.iter()
        .map(|a| {
            let (secs, line) = a.trim().split_once(' ').unwrap_or((a.as_str(), ""));
            (secs.parse().unwrap_or(0.0), line.trim().to_string())
        })
        .collect()
}

/// A switch cvar, on unless set to 0 (on when unregistered: the CoD4
/// Options dvars exist only on a CoD4 install).
/// The CoD4 renderer's dvars, read every frame.
pub(crate) fn cod4_options(shell: &console::shell::Shell) -> renderer::Cod4Options {
    renderer::Cod4Options {
        // `R_UsingGlow` wants both (cod4-world.md 11.4)
        glow: cvar_on(shell, "r_glow") && cvar_on(shell, "r_glow_allowed"),
        specular: cvar_on(shell, "r_specular"),
        water: cvar_on(shell, "r_drawWater"),
        zfeather: cvar_on(shell, "r_zFeather"),
        dof: cvar_on(shell, "r_dof_enable"),
        distortion: cvar_on(shell, "r_distortion"),
        fog: cvar_on(shell, "r_fog"),
        normal_maps: cvar_on(shell, "r_vc_normalMaps"),
        film: cvar_on(shell, "r_vc_film"),
    }
}

fn cvar_on(shell: &console::shell::Shell, name: &str) -> bool {
    shell
        .cvar(name)
        .is_none_or(|v| v.trim().parse::<f32>() != Ok(0.0))
}

/// `--window`'s `WxH`.
fn parse_window_size(s: &str) -> Result<(u32, u32), String> {
    video::parse_size(s)
        .filter(|&(w, h)| w > 0 && h > 0)
        .ok_or_else(|| format!("{s:?} is not WxH"))
}

/// The video mode `--window` (`window`) or else the cvars ask for
/// ([`video::video_mode`]).
fn video_mode_or(window: Option<(u32, u32)>, shell: &console::shell::Shell) -> video::VideoMode {
    match window {
        Some(size) => video::VideoMode {
            size: Some(size),
            fullscreen: video::Fullscreen::Windowed,
            refresh_hz: None,
        },
        None => video_mode(shell),
    }
}

/// The video mode the cvars ask for ([`video::video_mode`]).
fn video_mode(shell: &console::shell::Shell) -> video::VideoMode {
    let int = |name| shell.cvar_f32(name) as i32;
    let mut mode = video::video_mode(
        shell.cvar("r_mode").unwrap_or("-1"),
        int("r_fullscreen"),
        (int("r_customwidth"), int("r_customheight")),
    );
    // `%i Hz` (`set_cod2_mode_lists`)
    mode.refresh_hz = shell
        .cvar("r_displayRefresh")
        .and_then(|r| r.trim().strip_suffix("Hz")?.trim().parse().ok());
    mode
}

/// CoD2's `r_mode` and `r_displayRefresh` as enum dvars over the primary
/// display's modes ([`video::cod2_mode_lists`]), what the graphics menu's
/// `dvarEnumList` items step through.
fn set_cod2_mode_lists(
    shell: &mut console::shell::Shell,
    monitor: Option<winit::monitor::MonitorHandle>,
) {
    let modes: Vec<_> = monitor
        .map(|m| {
            m.video_modes()
                .map(|v| video::MonitorMode {
                    size: v.size().into(),
                    refresh_mhz: v.refresh_rate_millihertz(),
                    bit_depth: v.bit_depth(),
                })
                .collect()
        })
        .unwrap_or_default();
    let (sizes, rates) = video::cod2_mode_lists(&modes);
    log::debug!(
        "cod2: {} video modes, {} refresh rates",
        sizes.len(),
        rates.len()
    );
    shell.set_enum_domain("r_mode", sizes);
    if shell.cvar("r_displayRefresh").is_none() {
        shell.execute(&format!(
            "seta r_displayRefresh \"{}\"",
            rates.first().map_or("", |r| r)
        ));
    }
    shell.set_enum_domain("r_displayRefresh", rates);
}

/// Puts `window` in `mode`: windowed at its size, borderless over the
/// monitor it is on, or that monitor switched to its mode closest to the
/// size (the monitor's own size when `r_mode` names none). Wayland has no
/// exclusive fullscreen, so there it is borderless.
fn apply_video_mode(window: &Window, mode: video::VideoMode) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Fullscreen;
    let monitor = window
        .current_monitor()
        .or_else(|| window.primary_monitor());
    let wayland = window
        .window_handle()
        .is_ok_and(|h| matches!(h.as_raw(), RawWindowHandle::Wayland(_)));
    let exclusive = || {
        let monitor = monitor.as_ref()?;
        let want = mode.size.unwrap_or_else(|| monitor.size().into());
        let modes: Vec<_> = monitor.video_modes().collect();
        let infos: Vec<_> = modes
            .iter()
            .map(|m| video::MonitorMode {
                size: m.size().into(),
                refresh_mhz: m.refresh_rate_millihertz(),
                bit_depth: m.bit_depth(),
            })
            .collect();
        let pick = modes
            .get(video::closest_mode_at(want, mode.refresh_hz, &infos)?)?
            .clone();
        log::info!(
            "exclusive fullscreen {}x{} at {} Hz",
            pick.size().width,
            pick.size().height,
            pick.refresh_rate_millihertz() as f32 / 1000.0
        );
        Some(Fullscreen::Exclusive(pick))
    };
    match mode.fullscreen {
        video::Fullscreen::Windowed => {
            window.set_fullscreen(None);
            if let Some((w, h)) = mode.size {
                let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
            }
        }
        video::Fullscreen::Exclusive if !wayland => {
            let full = exclusive().unwrap_or(Fullscreen::Borderless(monitor));
            window.set_fullscreen(Some(full));
        }
        video::Fullscreen::Borderless | video::Fullscreen::Exclusive => {
            window.set_fullscreen(Some(Fullscreen::Borderless(monitor)));
        }
    }
}

/// One key on an open chat field: `None` while it stays open, `Some(None)`
/// when Escape (or Enter on an empty line) closes it, `Some(Some(line))`
/// when Enter sends `line`. The field is 256 bytes and a quote would end
/// the argument, so neither gets in.
fn chat_field_key(
    field: &mut hud::ChatField,
    code: KeyCode,
    text: Option<&str>,
) -> Option<Option<String>> {
    match code {
        KeyCode::Escape => return Some(None),
        KeyCode::Enter | KeyCode::NumpadEnter => {
            return Some(Some(std::mem::take(&mut field.text)).filter(|l| !l.is_empty()));
        }
        KeyCode::Backspace => {
            field.text.pop();
        }
        _ => {
            for c in text.unwrap_or("").chars() {
                if (' '..='~').contains(&c) && c != '"' && field.text.len() < 255 {
                    field.text.push(c);
                }
            }
        }
    }
    None
}

/// `record [name]` on `net` (CoDMP.exe 0x40e540): the file in `dir`, its
/// refusals and progress printed as retail prints them.
fn start_record(
    net: &mut net::NetClient<net::UdpTransport>,
    dir: &std::path::Path,
    name: Option<&str>,
) {
    let (path, name) = play::demos::record_path(dir, name, net.protocol());
    let file = std::fs::create_dir_all(dir).and_then(|_| std::fs::File::create(&path));
    // Refused before the file is made, as retail checks first.
    if net.is_recording() {
        console::log::print("Already recording.");
        return;
    }
    let file = match file {
        Ok(f) => f,
        Err(e) => {
            console::log::print(&format!("ERROR: couldn't open {}: {e}", path.display()));
            return;
        }
    };
    match net.start_recording(Box::new(std::io::BufWriter::new(file)), &name) {
        Ok(()) => console::log::print(&format!("recording to {}.", path.display())),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            console::log::print(&format!("{e}"));
        }
    }
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
    }
}

/// The camera's (yaw, pitch, roll) in radians for our own playerstate: the
/// raw cmd angles plus `delta_angles`, which is the view the server builds
/// (docs/protocol-1.1.md, "View angles"). Wire pitch is down-positive, the
/// camera's up-positive; roll keeps the wire's sign (positive tilts the head
/// right). Pitch is drawn clamped at retail's limit; the cmd still carries
/// the raw angle.
fn own_view(raw: [i32; 3], delta: [i32; 3]) -> (f32, f32, f32) {
    own_view_at(raw, delta, PITCH_CLAMP_SHORT)
}

/// [`own_view`] with the pitch held at `clamp` short units.
fn own_view_at(raw: [i32; 3], delta: [i32; 3], clamp: i16) -> (f32, f32, f32) {
    let short = |i: usize| raw[i].wrapping_add(delta[i]) as i16;
    let deg = |s: i16| s as f32 * 360.0 / 65536.0;
    let pitch = short(0).clamp(-clamp, clamp);
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

/// UO's in a vehicle seat (coduo-vehicles.md 3).
const SEAT_PITCH_CLAMP: i16 = 12000;

/// Reads every effect the level can play off its events, with their
/// sprite images and mark materials, and uploads the models it registers,
/// at the map load: not in the frame that first plays or draws one
/// (cod11-client-soak.md).
fn prewarm_level(
    fx: &mut fx::sim::FxSystem,
    r: &mut renderer::Renderer,
    fs: &Pk3Fs,
    net: &net::NetClient<net::UdpTransport>,
    live: &mut LivePhase,
) {
    let t0 = Instant::now();
    let mut laps = vcod_common::laps::Laps::new("cod1 level prewarm");
    let p = net.protocol();
    let models = live.scene.prewarm(r, fs, net.configstrings(), p);
    laps.lap("models");
    let paths = fx::registry::level_effects(net.configstrings(), &p.cs, &live.weapons, p.is_uo());
    let (mut marks, mut sprites, fx_models) = fx.preload_cod1(fs, paths.iter().map(String::as_str));
    laps.lap("effects");
    for m in &fx_models {
        live.scene.fx_model(r, fs, m);
    }
    laps.lap("effect models");
    sprites.insert(fx::sim::TRACER_SHADER.to_string());
    // The blob shadows and the coronas draw through the mark pass.
    marks.insert(blob_shadow::SHADOW_SHADER.to_string());
    marks.insert(corona::SHADER.to_string());
    r.prewarm_marks(fs, marks.iter().map(String::as_str));
    laps.lap("marks");
    r.prewarm_fx(fs, sprites.iter().map(String::as_str));
    laps.lap("sprites");
    laps.finish();
    log::info!(
        "level prewarm: {models} models, {} effects, {} effect models, {} mark and {} sprite shaders in {:.0} ms",
        paths.len(),
        fx_models.len(),
        marks.len(),
        sprites.len(),
        t0.elapsed().as_secs_f64() * 1000.0
    );
}

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
    vcod_common::laps::span(t0, || "viewmodels".into());
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
    vanilla: bool,
) -> Result<Phase> {
    let map = net::info_value_for_key(net.configstring(0), "mapname")
        .map(str::to_string)
        .context("the server sent no mapname in its serverinfo")?;
    r.unload_world();
    *world = None;
    if let Some(hud) = hud {
        hud.on_gamestate();
    }
    // The server's team colours reach `^8`/`^9` from the first HUD frame;
    // until then they draw white, as a fresh 1.5 or UO client's do.
    hud::font::set_team_colours(net.protocol().team_colour_codes, &|_| None);
    audio.set_protocol(net.protocol());
    audio.on_gamestate(&map);
    fx.clear();
    *title = format!("{} — loading {map}", version_tag());
    if let Some(window) = window {
        window.set_title(title);
    }
    // A demo sets no pak list (`CL_SystemInfoChanged` returns first while
    // one plays, CoDMP.exe 0x415eb0), so nothing is fetched, and a map
    // missing here fails as `CM_LoadMap` does (0x419c90).
    if net.is_demo() {
        return Ok(Phase::Loading {
            loader: loading::MapLoader::without_downloads(map.clone(), couldnt_load(&map)),
        });
    }
    // Same candidate order as the old blocking downloader; `MapLoader` caps it.
    let systeminfo = net.configstring(1).to_string();
    let exists = |rel: &std::path::Path| game_dir.join(rel).exists();
    // The base dir's own base under it (`main/` under `uo/`), then the rest.
    let dir_paths: Vec<std::path::PathBuf> = dirs
        .iter()
        .enumerate()
        .flat_map(|(i, d)| match i {
            0 => vcod_common::pk3::base_dirs(&game_dir.join(d)),
            _ => vec![game_dir.join(d)],
        })
        .collect();
    let mut paks =
        vcod_common::pk3::search_paks(&dir_paths.iter().map(|p| p.as_path()).collect::<Vec<_>>());
    if vanilla {
        let install = net::protocol::install_protocol_of(&paks);
        paks.retain(|p| vcod_common::pk3::is_vanilla_pak(p, &install.transport));
    }
    let local: Vec<i32> = paks.iter().map(|p| p.checksum).collect();
    let have = |c: i32| local.contains(&c);
    let proto = net.protocol();
    // Retail fetches no stock pak; say which ones this install lacks, as a
    // 1.1 install on a 1.5 server does (cod15-protocol-diff.md, "Paks").
    let stock = net::download::missing_stock_paks(&systeminfo, proto, have);
    if !stock.is_empty() {
        log::warn!(
            "The server's patch {} paks are not in this install: {}",
            proto.transport.short_version,
            stock.join(" ")
        );
    }
    if vanilla {
        // Nothing is fetched: a download lands outside the stock paks the
        // search path is limited to.
        let missing = net::download::missing_paks(&systeminfo, proto, dirs, have, exists);
        if !missing.is_empty() {
            log::warn!(
                "--vanilla: not loading the server's own paks: {}",
                missing.join(" ")
            );
        }
        let why = format!(
            "{map} is not in the stock paks, and --vanilla downloads nothing{}",
            if missing.is_empty() {
                String::new()
            } else {
                format!(" (the server uses {})", missing.join(" "))
            }
        );
        return Ok(Phase::Loading {
            loader: loading::MapLoader::without_downloads(map, why),
        });
    }
    let candidates = if allow_download {
        net::download::candidates_for_map(&systeminfo, proto, &map, dirs, have, exists)
            .into_iter()
            .map(|(name, rel)| (format!("{name}.pk3"), game_dir.join(rel)))
            .collect()
    } else {
        // `CL_InitDownloads` with autodownload off: warn, then load what is
        // there (docs/research/cod11-front-end.md, section 16).
        let missing = net::download::missing_paks(&systeminfo, proto, dirs, have, exists);
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

/// `CM_LoadMap`'s drop for a map no pak holds (CoDMP.exe 0x419c90): the
/// localized `Couldn't load maps/mp/<map>.bsp`.
fn couldnt_load(map: &str) -> String {
    format!("EXE_ERR_COULDNT_LOAD\u{15}maps/mp/{map}.bsp")
}

/// The front end off `fs`, with the favourites file and the Mods menu's
/// directory listing under `game_dir`.
fn new_ui(fs: &Pk3Fs, game_dir: &std::path::Path, mod_dir: &str, language: &str) -> frontend::Ui {
    let mut ui =
        frontend::Ui::new(fs, language).with_server_cache(game_dir.join("servercache.dat"));
    if fs.game() == vcod_common::game::Game::Cod2 {
        ui.set_profiles_dir(game_dir.join(mod_dir).join("players"));
    }
    // `--vanilla` loads no mod, so the Mods menu lists none.
    if fs.vanilla() {
        ui
    } else {
        ui.with_mod_root(game_dir.to_path_buf(), mod_dir.to_string())
    }
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

/// `cl_allowDownload`, which a server's systeminfo sets like any other key
/// (`system_info_changed`).
fn allow_download(shell: &console::shell::Shell) -> bool {
    shell.cvar_f32("cl_allowDownload") != 0.0
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
    hud: &mut Option<hud::Hud>,
    audio: &mut audio::AudioSystem,
    fx: &mut fx::sim::FxSystem,
    quick_chat: &mut quick_chat::QuickChat,
) -> Result<()> {
    let cod2 = fs.game() == vcod_common::game::Game::Cod2;
    *fs = match cod2 {
        true => Pk3Fs::open_iwd(base, game)?,
        false => Pk3Fs::open_search(base, game, pure, fs.vanilla())?,
    };
    *localized = vcod_common::localize::Localized::load(fs);
    match hud {
        Some(h) => {
            if let Err(e) = h.reopen(fs) {
                log::warn!("hud: {e}, keeping the fonts already loaded");
            }
        }
        None if !cod2 => *hud = hud::Hud::new(fs).ok(),
        None => {}
    }
    audio.reopen(fs);
    fx.reopen();
    fx::registry::init(fs);
    quick_chat.reopen();
    Ok(())
}

/// `maps/mp/<map>.bsp` off the search path, parsed ([`MapWorld`]'s asset
/// kind): what `CM_LoadMap` (CoDMP.exe 0x419c90) and `R_LoadWorldMap` read.
fn read_map(fs: &dyn AssetSource, map: &str) -> Result<MapWorld> {
    mapworld::find(fs, map).with_context(|| format!("cannot load map '{map}'"))
}

/// The serverinfo's map and its collision map off `fs`: the CPU half of
/// [`App::reload_live_map`].
fn reread_live_map(
    fs: &Pk3Fs,
    configstrings: &[String],
) -> Result<(MapWorld, collision::CollisionWorld)> {
    let serverinfo = configstrings.first().map_or("", String::as_str);
    let map = net::info_value_for_key(serverinfo, "mapname")
        .context("the server sent no mapname in its serverinfo")?;
    let bsp = read_map(fs, map)?;
    let world = client_world(fs, &bsp);
    Ok((bsp, world))
}

/// The CPU half of [`App::restart_cgame`]: a live game's cgame state
/// starts over as `CL_InitCGame` (CoDMP.exe 0x4049c0) loads a fresh cgame
/// (docs/research/cod11-front-end.md section 17, "The cgame restart").
/// False, touching nothing, before the game is live.
#[allow(clippy::too_many_arguments)]
fn restart_live_cgame(
    phase: &mut Phase,
    view: &mut play::view::OnlineView,
    input: &mut play::input::PlayInput,
    predictor: &mut play::predict::Predictor,
    hud: Option<&mut hud::Hud>,
    fx: &mut fx::sim::FxSystem,
    fs: &Pk3Fs,
    configstrings: &[String],
) -> bool {
    if !matches!(phase, Phase::Live(_)) {
        return false;
    }
    let Phase::Live(live) = std::mem::replace(phase, Phase::Connecting) else {
        unreachable!()
    };
    let weapons = live_weapons(fs, configstrings.get(7).map_or("", String::as_str));
    *phase = Phase::Live(Box::new(live.restart_cgame(weapons)));
    fresh_cgame(view, input, predictor, hud, fx);
    true
}

/// What a freshly loaded cgame starts without, past the live phase: the
/// view, the HUD with its chat ring, the effects and the prediction.
/// `CL_InitCGame` (CoDMP.exe 0x4049c0) runs on a `vid_restart` and on every
/// gamestate's `CL_DownloadsComplete` (0x40ffb0), which frees the old
/// cgame first (docs/research/cod11-front-end.md section 17).
fn fresh_cgame(
    view: &mut play::view::OnlineView,
    input: &mut play::input::PlayInput,
    predictor: &mut play::predict::Predictor,
    hud: Option<&mut hud::Hud>,
    fx: &mut fx::sim::FxSystem,
) {
    view.restart_cgame();
    // The fresh cgame's first snapshot runs a load's resets.
    input.new_gamestate();
    predictor.reset();
    if let Some(hud) = hud {
        hud.restart_cgame();
    }
    fx.clear();
}

/// The HUD between servers. Retail's disconnect (`Com_Error`'s
/// `ERR_DISCONNECT` arm, CoDMP.exe 0x4358a0) shuts the cgame down after
/// `CL_Disconnect`, so the chat ring goes with the game, not at the next
/// map load (docs/research/cod11-front-end.md section 17).
fn leave_server_hud(hud: &mut hud::Hud) {
    hud.restart_cgame();
    hud.chat_field = None;
    hud.scoreboard.hide();
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
    let mut laps = vcod_common::laps::Laps::new(format!("cod1 map {map}"));
    let bsp = read_map(fs, map)?;
    laps.lap("bsp");
    // The ambient rides configstring 3 (docs/research/cod11-sound-system.md,
    // section 9); a fresh gamestate may have changed it.
    audio.set_ambient(fs, net::info_value_for_key(net.configstring(3), "n"));
    *title = format!("{} — {map}", version_tag());
    if let Some(window) = window {
        window.set_title(title);
    }
    laps.lap("ambient");
    let phase = live_phase(fs, &bsp, net.configstring(7));
    laps.lap("collision and weapons");
    r.load_world(&bsp, fs)?;
    laps.lap("renderer");
    laps.finish();
    if let Phase::Live(live) = &phase {
        r.set_hud_weapons(Some(&live.weapons));
    }
    *world = Some(World {
        bsp,
        fog: None,
        materials: None,
    });
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
            vehicle_turret_yaw: None,
            cvar: &|_| None,
            bound_key: &|_| None,
            draw: hud::DrawToggles::default(),
            weapon_select: 0,
            own_client: None,
            server_address: None,
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
        fov_x: camera::DEFAULT_FOV_DEG,
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
    bsp: &MapWorld,
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
        view_weapon,
        configstrings,
        weapon_slot: start_slot,
        reserve,
        switch_to: None,
        rigs,
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
    /// The install's game. A CoD4 search path is an iwd one, whose
    /// `Pk3Fs::game` reads CoD2, so a CoD4 check reads this.
    game: vcod_common::game::Game,
    fs: Pk3Fs,
    /// Where downloads land and the pk3 path reopens from.
    game_dir: std::path::PathBuf,
    mod_dir: String,
    /// The `fs_game` the search path was last opened with, layered over
    /// `mod_dir`; the cvar ([`App::fs_game_cvar`]) is what it follows.
    fs_game: Option<String>,
    /// A pure server's `sv_paks`: only paks with these checksums load.
    fs_pure: Option<Vec<i32>>,
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
    viewmodel: Arc<[model::Model]>,
    /// Fly-mode keys; walk keeps its own in `Mode::Walk`.
    input: InputState,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    grabbed: bool,
    fx: fx::sim::FxSystem,
    /// United Offensive's flamethrower chunks.
    flame: fx::flame::FlameSystem,
    /// United Offensive's client-traced vehicle rounds.
    client_bullets: fx::client_bullet::ClientBullets,
    /// Origin of the clock effects are timed against.
    start: Instant,
    last_frame: Instant,
    /// `com_frameTime`'s last frame, ms since `start` (`Com_Frame`'s pace).
    frame_ms: u64,
    debug_overlay: bool,
    cull_mode: renderer::CullMode,
    hud_stats: hud_text::HudStats,
    /// Online frames without a straddling snapshot pair. Cumulative; the
    /// overlay shows the rate.
    interp_misses: u64,
    /// Cumulative events drained, and those with an unrecognized code.
    ev_seen: u64,
    ev_unknown: u64,
    /// Another protocol's events with no 1.1 counterpart, logged once each.
    foreign_events: HashSet<i32>,
    build_ms: f32,
    render_ms: f32,
    fx_ms: f32,
    /// Last frame's fov over `cg_fov` and whether the view rides a mounted
    /// gun: the mouse's sensitivity scale ([`play::input::MouseLook`]).
    look_zoom: (f32, bool),
    /// The keyboard's modifiers, for Alt+Enter.
    modifiers: winit::keyboard::ModifiersState,
    /// The view's mouse motion since the last frame (`CL_MouseEvent`).
    mouse: play::input::MouseAccum,
    /// `r_ignorehwgamma` as of the last window start or `vid_restart`
    /// (retail latches it): textures bake `r_gamma` at each world load and
    /// the frame pass only clamps.
    ignore_hw_gamma: bool,
    /// The texture settings the current renderer was built with.
    textures: texture_settings::TextureSettings,
    /// `r_textureMode` as last seen: a change swaps the renderer's
    /// samplers, or prints `bad filter name` (`GL_TextureMode`, 0x4ea530).
    texture_mode_seen: String,
    hud: Option<hud::Hud>,
    hud_ms: f32,
    /// Menu labels; empty outside `--connect`.
    localized: vcod_common::localize::Localized,
    audio: audio::AudioSystem,
    quick_chat: quick_chat::QuickChat,
    /// The main menu and server browser, up while no game is.
    ui: frontend::Ui,
    /// Menu effects queued where no event loop is at hand; `about_to_wait`
    /// runs them.
    ui_pending: Vec<frontend::UiEffect>,
    /// Fly mode's CoD4 map effects and sounds (`cod4_fly::FlyAv`).
    cod4_fly: Option<cod4_fly::FlyAv>,
    /// A CoD4 join in flight: the connect screen draws while it runs
    /// (`cod4_loading`), and its session and map take over once loaded.
    cod4_loading: Option<cod4_loading::Loader>,
    /// `--window`'s size, over the cvars' video mode ([`video_mode_or`]).
    window_size: Option<(u32, u32)>,
    /// `--exec`'s lines, run at the end of `resumed`.
    exec: Vec<String>,
    /// `--exec-in-game`'s, run after the first frame that draws a game.
    exec_in_game: Vec<String>,
    /// `--exec-after`'s queue: seconds after the last one ran, the line.
    exec_after: std::collections::VecDeque<(f32, String)>,
    /// When the `--exec-in-game` lines or the last `--exec-after` one ran.
    exec_after_last: Option<std::time::Instant>,
    /// `VCOD_SOAK`'s player (`play::cod1_soak`).
    soak: Option<play::cod1_soak::Soak>,
    /// The menus hold the mouse over a game: the cursor shows and moves
    /// over them and the view does not turn ([`Self::sync_menu_mouse`]).
    menu_mouse: bool,
    /// Whether the OS cursor shows over the window ([`Self::sync_os_cursor`]).
    os_cursor: bool,
    /// The OS pointer is locked to the window and its motion arrives as raw
    /// deltas, for the view or the UI cursor ([`Self::sync_capture`]).
    captured: bool,
    /// The `vcod-server` Start New Server launched; dropping it kills it,
    /// so it goes with the client.
    server_process: Option<frontend::create::ServerProcess>,
    /// The CoD4 front end while a CoD4 game is up (`cod4_front`).
    cod4_parked: Option<Box<cod4_front::Cod4Front>>,
    /// `rcon`'s socket while not connected; its `print` replies are polled
    /// each loop.
    rcon: console::rcon::RconSocket,
    /// CoD2's and CoD4's `rcon login` password and `rcon host` address.
    cod4_rcon: console::rcon::Cod4Rcon,
    /// The playing demo's `timedemo` count.
    timedemo: play::demos::TimeDemo,
    /// Nested `exec`s running.
    exec_depth: u8,
    error: Option<anyhow::Error>,
    /// The game's session: CoD2's client, or CoD1's (whose state is
    /// `mode`, `world` and `hud`).
    session: session::Session,
    /// The wheel's turn not yet a whole notch.
    wheel: console::keys::WheelNotches,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: anyhow::Error) {
        self.error = Some(err);
        event_loop.exit();
    }

    /// Gives the mouse to the view (`grabbed`) or takes it back.
    /// `CL_MouseMove` (CoDMP.exe 0x40b240) once a frame: the motion since
    /// the last frame, or with `m_filter` its mean with the last frame's,
    /// turns the view or, online, moves the cmd (`MouseLook::moves`).
    fn mouse_frame(&mut self) {
        let filter = self.shell.cvar_f32("m_filter") as i32 != 0;
        let (dx, dy) = self.mouse.take(filter);
        if !self.grabbed {
            self.mouse.clear();
            return;
        }
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let look = play::input::MouseLook {
            sensitivity: self.shell.cvar_f32("sensitivity"),
            m_yaw: self.shell.cvar_f32("m_yaw"),
            m_pitch: self.shell.cvar_f32("m_pitch"),
            m_forward: self.shell.cvar_f32("m_forward"),
            m_side: self.shell.cvar_f32("m_side"),
            freelook: self.shell.cvar_f32("cl_freelook") as i32 != 0,
        };
        match &mut self.mode {
            Mode::Idle => {}
            Mode::Fly(cam) => cam.look(look.degrees(dx, dy, self.look_zoom.0, false)),
            // The session's zoom and shellshock scale the turn.
            Mode::Cod2(cam) => {
                cam.look(
                    self.session
                        .turn(look.degrees(dx, dy, self.look_zoom.0, false)),
                )
            }
            Mode::Online { join, .. } if join.stray_catcher() => {}
            Mode::Online { input, .. } => {
                let (zoom, mounted) = self.look_zoom;
                let (mlook, strafe) = input.mlook_strafe();
                input.mouse_move(look.moves(dx, dy, zoom, mounted, mlook, strafe));
            }
            Mode::Walk { ps, .. } => {
                // the same turn and pitch clamp as FlyCamera::look
                let [pitch, yaw] = look.degrees(dx, dy, 1.0, false);
                ps.yaw += yaw.to_radians();
                ps.pitch =
                    (ps.pitch - pitch.to_radians()).clamp(camera::PITCH_MIN, camera::PITCH_MAX);
            }
        }
    }

    fn set_grab(&mut self, grabbed: bool) {
        if self.window.is_none() {
            return;
        }
        self.grabbed = grabbed;
        self.sync_capture();
        if grabbed && !self.captured {
            self.grabbed = false;
        }
    }

    /// Captures the OS pointer while the view or the menus take the mouse,
    /// as retail's `IN_Frame` (CoDMP.exe 0x461a80) keeps it captured unless
    /// the console is down in a window; a menu's cursor then moves by the
    /// raw deltas, never by the OS pointer (docs/research/cod11-front-end.md
    /// section 13).
    fn sync_capture(&mut self) {
        let fullscreen = self
            .window
            .as_ref()
            .is_some_and(|w| w.fullscreen().is_some());
        let menus = (self.ui.active() || self.session.cod4_front_active())
            && !self.bypass_mouse()
            && (!self.console.open || fullscreen);
        let want = self.grabbed || menus;
        if want != self.captured
            && let Some(window) = &self.window
        {
            if want {
                if window.set_cursor_grab(CursorGrabMode::Locked).is_ok()
                    || window.set_cursor_grab(CursorGrabMode::Confined).is_ok()
                {
                    self.captured = true;
                } else {
                    static WARNED: std::sync::Once = std::sync::Once::new();
                    WARNED.call_once(|| {
                        log::warn!("this platform does not support grabbing the cursor");
                    });
                }
            } else if window.set_cursor_grab(CursorGrabMode::None).is_ok() {
                self.captured = false;
            }
        }
        self.sync_os_cursor();
    }

    /// The OS cursor shows only while the pointer is free, as
    /// `IN_DeactivateMouse` (CoDMP.exe 0x4616b0) shows it; a menu draws the
    /// UI's own cursor either way (`UI_Refresh`).
    fn sync_os_cursor(&mut self) {
        let show = !self.captured;
        if show != self.os_cursor
            && let Some(window) = &self.window
        {
            window.set_cursor_visible(show);
            self.os_cursor = show;
        }
    }

    /// The press `mp_QuickMessage`'s stray UI catcher swallows; held keys
    /// let go as `_UI_KeyEvent`'s `trap_Key_ClearStates` (0x4000d768) has
    /// them.
    fn eat_stray_key(&mut self) -> bool {
        let Mode::Online { join, .. } = &mut self.mode else {
            return false;
        };
        if !join.take_stray_key() {
            return false;
        }
        self.clear_held_keys();
        true
    }

    /// On grab release or focus loss, so no held key stays latched. Prone
    /// and the `--connect` stance stay, neither is a held key. The scoreboard drops, Tab is held too.
    fn clear_held_keys(&mut self) {
        match &mut self.mode {
            Mode::Idle => {}
            Mode::Fly(_) => self.input = InputState::default(),
            Mode::Cod2(_) => {
                self.input = InputState::default();
                self.session.release_keys();
            }
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
            hud.scoreboard.hide();
        }
        self.session.hide_scoreboard();
    }

    /// The chat field's keys while it is open, online only. Enter sends
    /// `say "<line>"` the way the console's field does (CoDMP.exe 0x40d40b);
    /// Escape drops the line. False when the key is not the field's.
    fn chat_key(&mut self, code: KeyCode, text: Option<&str>) -> bool {
        if let Some(taken) = self.session.chat_key(code, text) {
            return taken;
        }
        let Mode::Online { net, .. } = &mut self.mode else {
            return false;
        };
        let Some(hud) = &mut self.hud else {
            return false;
        };
        let Some(field) = &mut hud.chat_field else {
            return false;
        };
        if let Some(line) = chat_field_key(field, code, text) {
            if let Some(cmd) = line.and_then(|l| play::cod2::chat_command(field.team, &l)) {
                net.send_reliable(&cmd);
            }
            hud.chat_field = None;
        }
        true
    }

    /// Whether keys go to the console: it is down, or there is nothing else.
    /// Keys, mouse buttons and the wheel go through the binds: online,
    /// a CoD2 session included (`Mode::Cod2`,
    /// `play::cod2_input`).
    fn plays_binds(&self) -> bool {
        matches!(self.mode, Mode::Online { .. }) || self.session.live()
    }

    fn console_active(&self) -> bool {
        self.console.open
            || (matches!(self.mode, Mode::Idle)
                && !self.ui.active()
                && !self.session.cod4_front_active()
                && self.cod4_loading.is_none())
    }

    /// Whether the front end takes the mouse and keys: with no game up, or
    /// as the in-game main menu.
    fn menu_active(&self) -> bool {
        !self.console.open && self.ui.active()
    }

    /// `_UI_Init` again: a fresh front end off the search path, with
    /// `ui_menuFiles` set back to its default first (0x4000d324). The
    /// server lists carry over, as retail's survive in `servercache.dat`.
    /// In a live game the cgame starts again too; an error is its drop
    /// ([`load_script_menus`]).
    fn restart_ui(&mut self) -> Result<(), String> {
        self.shell.execute("set ui_menuFiles ui_mp/menus.txt");
        let browser = std::mem::take(&mut self.ui.browser);
        self.ui = new_ui(
            &self.fs,
            &self.game_dir,
            &self.mod_dir,
            self.shell.cvar("cl_language").unwrap_or(""),
        );
        self.ui.set_browser(browser);
        // The cgame starts again with the UI and loads its menus back.
        if let Mode::Online {
            net,
            phase: Phase::Live(_),
            ..
        } = &self.mode
        {
            return load_script_menus(
                &mut self.ui,
                &self.fs,
                &self.shell,
                net.configstrings(),
                &net.protocol().cs,
            );
        }
        Ok(())
    }

    /// `vid_restart`'s UI half: `CL_Vid_Restart_f` (CoDMP.exe 0x40fbe0)
    /// always shuts the UI down and starts it again, so the UI and the
    /// script menus load again (under a latched `cl_language`'s directory)
    /// and come back closed, the main menu aside with no game up.
    fn vid_restart_ui(&mut self) {
        if let Err(e) = self.restart_ui() {
            self.disconnect(Some(e));
            return;
        }
        match &mut self.mode {
            // CoD2's UI comes back closed too (cod2-sound.md section 9).
            Mode::Online { .. } | Mode::Cod2(_) => self.after_menu(),
            Mode::Idle => self.enter_menu(None),
            Mode::Fly(_) | Mode::Walk { .. } => {}
        }
    }

    /// The main menu, or the error popup over it when `error` says why the
    /// game ended. The map set is the `menu` one too while no game is up.
    fn enter_menu(&mut self, error: Option<&str>) {
        self.leave_cod4();
        if self.enter_cod4_menu(error.map(|e| (e, false))) {
            return;
        }
        self.audio.on_gamestate("menu");
        hud::font::set_front_end_colours(&self.fs);
        let mut out = Vec::new();
        match error {
            Some(e) => self.ui.show_error(e, &self.shell, &mut out),
            None => self.ui.open_main(&self.shell, &mut out),
        }
        self.ui_pending.extend(out);
    }

    /// Carries out what a menu script asked for.
    fn ui_effects(&mut self, event_loop: &ActiveEventLoop, effects: Vec<frontend::UiEffect>) {
        use frontend::UiEffect;
        for effect in effects {
            match effect {
                UiEffect::ScriptMenuResponse { menu, response } => {
                    if !self.session.menu_response(&menu, &response)
                        && let Mode::Online { net, .. } = &mut self.mode
                    {
                        let cmd = play::join::script_menu_response(
                            &menu,
                            &response,
                            net.configstrings(),
                            &net.protocol().cs,
                            net.server_id(),
                        );
                        log::debug!("script menu response: {cmd}");
                        net.send_reliable(&cmd);
                    }
                }
                UiEffect::Command(line) => {
                    let effects = self.shell.execute(&line);
                    self.apply(event_loop, effects);
                }
                UiEffect::Sound(alias) => {
                    self.audio.play_menu(&self.fs, &alias);
                }
                UiEffect::FocusSound { item, global } => {
                    // A name the menu set lacks registers as 0 (UI syscall
                    // 0x26), so the item falls back to `itemFocusSound`.
                    let alias = [item, global]
                        .into_iter()
                        .flatten()
                        .find(|a| self.audio.has_menu_alias(a));
                    if let Some(alias) = alias {
                        self.audio.play_menu(&self.fs, &alias);
                    }
                }
                UiEffect::RunMod(game) => self.run_mod(game),
                UiEffect::StartServer(req) => self.start_server(&req),
                UiEffect::LoadProfile(name) => self.load_profile(&name),
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

    /// Start New Server: runs `vcod-server` beside this executable with the
    /// menu's settings, its output in `vcod-server.log` beside the config.
    /// Dedicated No joins it once it answers (`poll_server_join`); LAN and
    /// Internet leave the client in the front end. A server started
    /// earlier goes first.
    fn start_server(&mut self, req: &frontend::create::StartServer) {
        use frontend::create;
        if matches!(self.mode, Mode::Online { .. }) {
            self.disconnect(None);
        }
        self.server_process = None;
        let player_mod = self.fs_game_cvar();
        let shell = &self.shell;
        let launched = (|| {
            let exe = create::server_exe()?;
            let port = create::free_port(28960).context("no free UDP port from 28960")?;
            let value = |n: &str| shell.cvar(n).map(str::to_string);
            let dirs = (
                self.game_dir.as_path(),
                self.mod_dir.as_str(),
                player_mod.as_deref(),
                self.fs.vanilla(),
            );
            let args = create::server_args(req, port, dirs, &value);
            let log = self.config_path.with_file_name("vcod-server.log");
            log::info!("starting {} {}", exe.display(), args.join(" "));
            create::ServerProcess::spawn(&exe, &args, port, log, req.dedicated == 0)
        })();
        match launched {
            Ok(server) => {
                console::log::print(&format!(
                    "vcod-server on port {}, log in {}",
                    server.port,
                    server.log.display()
                ));
                self.server_process = Some(server);
            }
            Err(e) => {
                let mut out = Vec::new();
                self.ui.show_error(&format!("{e:#}"), &self.shell, &mut out);
                self.ui_pending.extend(out);
            }
        }
    }

    /// Dedicated No's wait for its server: connects once it answers
    /// `getinfo`, shows the error popup when it ended or never answered.
    fn poll_server_join(&mut self) {
        use frontend::create::JoinStep;
        let Some(server) = self.server_process.as_mut() else {
            return;
        };
        match server.poll_join(Instant::now()) {
            None | Some(JoinStep::Wait) => {}
            Some(JoinStep::Connect) => {
                let addr = format!("127.0.0.1:{}", server.port);
                self.connect(addr);
            }
            Some(JoinStep::Fail(why)) => {
                let text = format!("{why}, see {}", server.log.display());
                log::error!("{text}");
                self.server_process = None;
                let mut out = Vec::new();
                self.ui.show_error(&text, &self.shell, &mut out);
                self.ui_pending.extend(out);
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
        } else if self.grab_before_console && !(self.ui.active() && !self.bypass_mouse()) {
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
        // A CoD4 board takes its keys' presses (`Scoreboard_HandleInput`).
        if pressed {
            let step = (self.shell.cvar_f32("cg_scoreboardScrollStep") as i32).clamp(1, 8);
            if self.session.cod4_scoreboard_key(key, step) {
                return;
            }
        }
        // While held the board takes the wheel and the page keys
        // (cgame 0x30027180), `cg_scoreboardScrollStep` items a press.
        let board = match (&mut self.hud, &self.mode) {
            (Some(hud), Mode::Online { .. }) => Some(&mut hud.scoreboard),
            _ if self.session.live() => self.session.scoreboard_mut(),
            _ => None,
        };
        if let Some(board) = board
            && board.held()
        {
            let step = self.shell.cvar_f32("cg_scoreboardScrollStep") as i32;
            match key {
                "MWHEELUP" | "PGUP" | "KP_PGUP" => {
                    if pressed {
                        board.scroll_up(step);
                    }
                    return;
                }
                "MWHEELDOWN" | "PGDN" | "KP_PGDN" => {
                    if pressed {
                        board.scroll_down(step);
                    }
                    return;
                }
                _ => {}
            }
        }
        let effects = self.shell.key_event(key, pressed);
        self.apply(event_loop, effects);
    }

    /// Carries out what the console's commands asked for.
    fn apply(&mut self, event_loop: &ActiveEventLoop, effects: Vec<console::shell::Effect>) {
        use console::shell::Effect;
        for effect in effects {
            // A CoD2 session takes the gameplay binds (play::cod2_input).
            if self.session.apply(&effect, &self.shell, self.start) {
                continue;
            }
            match effect {
                Effect::Print(line) => console::log::print(&line),
                Effect::Clear => self.console.clear(),
                Effect::Connect(addr) => self.connect(addr),
                Effect::Reconnect => match self.connect_addr.clone() {
                    Some(addr) => self.connect(addr),
                    None => console::log::print("Not connected to a server."),
                },
                Effect::Record(name) => {
                    let dir = match self.session {
                        session::Session::Cod4(_) => self.cod4_demo_dirs()[0].clone(),
                        _ => self.demo_dirs()[0].clone(),
                    };
                    match (&mut self.mode, self.session.record(&dir, name.as_deref())) {
                        (_, Some(reply)) => console::log::print(&reply),
                        (Mode::Online { net, .. }, _) => start_record(net, &dir, name.as_deref()),
                        _ => console::log::print("You must be in a level to record."),
                    }
                }
                Effect::StopRecord => match (&mut self.mode, self.session.stop_record()) {
                    (_, Some(reply)) => console::log::print(&reply),
                    (Mode::Online { net, .. }, _) if net.is_recording() => {
                        match net.stop_recording() {
                            Ok(_) => console::log::print("Stopped demo."),
                            Err(e) => console::log::print(&format!("ERROR: {e}")),
                        }
                    }
                    _ => console::log::print("Not recording a demo."),
                },
                Effect::Demo { name, timedemo } => {
                    if self.game == vcod_common::game::Game::Cod4 {
                        self.play_cod4_demo(&name, timedemo);
                    } else if self.fs.game() == vcod_common::game::Game::Cod2 {
                        self.play_cod2_demo(&name, timedemo);
                    } else {
                        self.play_demo(&name);
                    }
                }
                // A CoD4 session's `disconnect` (Leave Game's Yes) leaves
                // the server for the main menu.
                Effect::Disconnect if matches!(self.session, session::Session::Cod4(_)) => {
                    self.disconnect(None)
                }
                Effect::Disconnect => {
                    if self.in_game() || self.session.busy() {
                        self.disconnect(None);
                    } else {
                        console::log::print("Not connected to a server.");
                    }
                }
                Effect::Quit => event_loop.exit(),
                Effect::ShellShock(args) => match &mut self.mode {
                    Mode::Online { view, .. } => view.shock_command = Some(args),
                    _ => {
                        if self.session.shellshock(args).is_err() {
                            console::log::print("Unknown command \"cg_shellshock\"")
                        }
                    }
                },
                Effect::Rcon(args) => self.rcon(&args),
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
                    let freelook = self.shell.cvar_f32("cl_freelook") as i32 != 0;
                    if let Mode::Online { input, net, .. } = &mut self.mode {
                        input.key(action, down);
                        if action == play::input::Action::MLook && !down && !freelook {
                            let delta = net
                                .snapshots()
                                .newest()
                                .map_or(0, |s| s.ps.field_i32(net.protocol(), "delta_angles[0]"));
                            input.center_view(delta);
                        }
                    }
                }
                Effect::Impulse(action) => {
                    if let Mode::Online { input, net, .. } = &mut self.mode {
                        // A held board takes both first (cgame 0x30038040).
                        if let (
                            play::input::Action::NextWeapon | play::input::Action::PrevWeapon,
                            Some(hud),
                        ) = (action, &mut self.hud)
                        {
                            let step = self.shell.cvar_f32("cg_scoreboardScrollStep") as i32;
                            if hud
                                .scoreboard
                                .take_cycle(action == play::input::Action::NextWeapon, step)
                            {
                                continue;
                            }
                        }
                        // In a UO seat `weapnext` and `weapprev` ask for the
                        // next or previous seat (uo cgame 0x300474c0,
                        // 0x30047550).
                        if matches!(
                            action,
                            play::input::Action::NextWeapon | play::input::Action::PrevWeapon
                        ) && let Some(s) = net.snapshots().newest()
                            && net.protocol().is_uo()
                            && s.ps.field_i32(net.protocol(), "eFlags")
                                & net::flags::EF_IN_VEHICLE_V22
                                != 0
                        {
                            net.send_reliable(if action == play::input::Action::NextWeapon {
                                "nextvehslot"
                            } else {
                                "prevvehslot"
                            });
                            continue;
                        }
                        let select = matches!(
                            action,
                            play::input::Action::Slot(_)
                                | play::input::Action::NextWeapon
                                | play::input::Action::PrevWeapon
                                | play::input::Action::AltWeapon
                        );
                        // The view and seat gates come before the delay and
                        // the stamp, so a refused bind leaves the name alone.
                        if select && !input.select_allowed(action) {
                            continue;
                        }
                        // `cg_weaponCycleDelay` (cgame 0x30038096): a select
                        // inside the delay from the name's stamp does nothing.
                        let delay = self.shell.cvar_f32("cg_weaponCycleDelay") as i32;
                        if select
                            && let Some(hud) = &self.hud
                            && !hud.weapon_cycle_allowed(delay)
                        {
                            continue;
                        }
                        // `weapalt` stamps only when it selects, and says so
                        // when the gun has no other mode (0x30037f90).
                        if action == play::input::Action::AltWeapon {
                            let now_ms =
                                ((Instant::now() - self.start).as_secs_f32() * 1000.0) as i32;
                            match (input.alt_weapon(), &mut self.hud) {
                                (play::input::AltWeapon::Selected, Some(hud)) => {
                                    hud.weapon_selected()
                                }
                                (play::input::AltWeapon::NoAlternate, Some(hud)) => hud
                                    .game_message(
                                        &self
                                            .localized
                                            .message("CGAME_THIS_WEAPON_HAS_NO_ALTERNATE"),
                                        now_ms,
                                    ),
                                _ => {}
                            }
                            continue;
                        }
                        input.key(action, true);
                        input.key(action, false);
                        if let (true, Some(hud)) = (select, &mut self.hud) {
                            hud.weapon_selected();
                        }
                    }
                }
                // `weapon <name|number>` (0x30038130): no cycle delay, a name
                // looked up in configstring 7 first.
                Effect::Weapon(arg) => {
                    if let Mode::Online { input, net, .. } = &mut self.mode {
                        let names = net.configstrings().get(7).map_or("", String::as_str);
                        let index = |name: &str| {
                            names
                                .split(' ')
                                .position(|n| n.eq_ignore_ascii_case(name))
                                .and_then(|i| u8::try_from(i + 1).ok())
                        };
                        if input.weapon_command(&arg, index)
                            && let Some(hud) = &mut self.hud
                        {
                            hud.weapon_selected();
                        }
                    }
                }
                // The server never pushes scores: send `score` on the down
                // edge unless one went out in the last 2 s, and every 2 s
                // while the board is up (see the redraw tick;
                // docs/research/cod11-hud-protocol.md, sections 3.1 and 4).
                Effect::Scores(down) => {
                    if let (Mode::Online { net, .. }, Some(hud)) = (&mut self.mode, &mut self.hud) {
                        let now = (Instant::now() - self.start).as_secs_f32();
                        if !down {
                            hud.scoreboard.release(now);
                        } else if hud.scoreboard.press(now) {
                            net.send_reliable("score");
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
                    self.session.set_userinfo(&self.shell.userinfo());
                }
                Effect::SaveConfig => {
                    if let Err(e) = std::fs::write(&self.config_path, self.shell.config_text()) {
                        log::warn!("cannot write {}: {e}", self.config_path.display());
                    }
                }
                Effect::Exec(file) => self.exec_file(event_loop, &file),
                // CoD2's; a CoD1 client has no such command.
                Effect::OpenScriptMenu { .. } => {}
                // cgame 0x300140e0: only with the client's own view
                // (`pm_flags` 0x40000); CoDMP.exe 0x4034f9 adds `CA_ACTIVE`.
                // The UI closes every menu, then opens `quickmessage` only
                // if its menus still hold it, which `ui_load` drops.
                Effect::QuickMessage => {
                    if let Mode::Online {
                        net, join, phase, ..
                    } = &mut self.mode
                        && matches!(phase, Phase::Live(_))
                        && net.snapshots().newest().is_some_and(|s| {
                            s.ps.field_i32(net.protocol(), "pm_flags") & play::recoil::PMF_OWN_VIEW
                                != 0
                        })
                    {
                        let mut out = Vec::new();
                        if self.ui.open_quick_message(&self.shell, &mut out) {
                            join.quick_message_opened();
                        } else {
                            join.quick_message_missing();
                        }
                        self.ui_pending.extend(out);
                    }
                }
                Effect::UiLoad => {
                    let list = self.shell.cvar("ui_menuFiles").unwrap_or("").to_string();
                    let mut out = Vec::new();
                    let language = self.shell.cvar("cl_language").unwrap_or("").to_string();
                    self.ui
                        .reload(&self.fs, &list, &language, &self.shell, &mut out);
                    self.ui_pending.extend(out);
                }
                Effect::SndRestart => {
                    // `snd_restart` (CoDMP.exe 0x40fdf0): refused under a
                    // listen server, else Miles shut down and started again,
                    // which re-registers the `mss_*` cvars (0x44fe60), then
                    // `vid_restart` (0x40fe0c calls 0x40fbe0).
                    if self.refuse_restart("Listen server cannot sound restart.") {
                        continue;
                    }
                    self.shell.apply_latched_sound();
                    let rate = audio::bank::mixer_rate(self.shell.cvar_f32("mss_khz") as i32);
                    self.audio.restart(&self.fs, rate);
                    // The streams `audio.restart` brought back outlive
                    // the restart's stop (0x40fea7 restores after 0x40fbe0).
                    self.vid_restart(event_loop, false);
                }
                Effect::VidRestart => self.vid_restart(event_loop, true),
                Effect::ApplyPicmip => self.apply_picmip(event_loop),
            }
        }
    }

    /// `vid_restart` (`CL_Vid_Restart_f`, CoDMP.exe 0x40fbe0): the sounds,
    /// the latched video cvars, the UI, the window and, in a live game, the
    /// cgame ([`Self::restart_cgame`]). `stop_sounds` is false under
    /// `snd_restart`, whose restored streams stand for the stop.
    fn vid_restart(&mut self, event_loop: &ActiveEventLoop, stop_sounds: bool) {
        if self.refuse_vid_restart() {
            return;
        }
        if stop_sounds {
            self.vid_restart_stop_sounds();
        }
        self.shell.apply_latched_video();
        // A Mods pick the listen server refused takes effect now
        // (`FS_Restart` on a modified `fs_game`, 0x40fd66).
        if !matches!(self.mode, Mode::Online { .. }) {
            self.switch_fs_game(self.fs_game_cvar());
        }
        self.vid_restart_ui();
        self.ignore_hw_gamma = ignore_hw_gamma(&self.shell);
        // Retail's `vid_restart` reloads every image; vcod rebuilds
        // only when the format, the MSAA or an image setting moved.
        self.shell.cod4_set_picmip();
        let textures = texture_cvars(&self.shell, self.textures.filter);
        let samples = renderer::aa_samples(self.shell.cvar_f32("r_aaSamples"));
        if self.renderer.as_ref().is_some_and(|r| {
            r.hdr() != hdr_cvar(&self.shell)
                || textures != self.textures
                || samples != renderer::msaa_samples()
        }) {
            self.rebuild_renderer(event_loop);
        }
        if let Some(w) = self.window.clone() {
            apply_video_mode(&w, video_mode_or(self.window_size, &self.shell));
            self.refresh_mode_lists(&w);
        }
        self.restart_cgame();
    }

    /// The mode lists again off the monitor the window is on: before the
    /// first window a Wayland session may name no monitor at all, which
    /// left Video Mode and Refresh Rate empty and greyed in a window.
    fn refresh_mode_lists(&mut self, window: &Window) {
        if !matches!(
            self.fs.game(),
            vcod_common::game::Game::Cod2 | vcod_common::game::Game::Cod4
        ) {
            return;
        }
        if let Some(m) = window
            .current_monitor()
            .or_else(|| window.primary_monitor())
        {
            set_cod2_mode_lists(&mut self.shell, Some(m));
        }
    }

    /// CoD4's `r_applyPicmip` (`R_ApplyPicmip`, iw3mp.exe 0x5f2c80):
    /// `R_SetPicmip` (0x617520), then every material's file-loaded images
    /// at a picmip that moved are reloaded (0x5f2c20), with no UI restart.
    /// vcod reloads by rebuilding the renderer, as a `vid_restart` that
    /// moved an image setting does.
    fn apply_picmip(&mut self, event_loop: &ActiveEventLoop) {
        if self.game != vcod_common::game::Game::Cod4 {
            return;
        }
        self.shell.cod4_set_picmip();
        let textures = texture_cvars(&self.shell, self.textures.filter);
        if self.renderer.is_some() && textures != self.textures {
            self.rebuild_renderer(event_loop);
        }
    }

    /// `CL_Vid_Restart_f`'s `S_StopAllSounds(1)` (CoDMP.exe 0x44fa40), with
    /// a game or without one: the main menu's music and its click go too
    /// (docs/research/cod11-front-end.md section 17, "The cgame restart").
    /// The offline viewer has no retail counterpart and keeps its ambient.
    fn vid_restart_stop_sounds(&mut self) {
        if !self.mode.keeps_sounds_on_vid_restart() {
            self.audio.stop_all();
        }
    }

    /// A renderer for `window` with the current world, submodels and
    /// viewmodel loaded into it. A CoD2 session is read on its own thread
    /// meanwhile: the world's materials take long on a software adapter.
    fn build_renderer(&mut self, window: Arc<Window>) -> Result<Renderer> {
        self.session.park();
        let r = self.build_renderer_now(window);
        self.session.finish_pump();
        r
    }

    fn build_renderer_now(&mut self, window: Arc<Window>) -> Result<Renderer> {
        self.textures = texture_cvars(&self.shell, self.textures.filter);
        self.texture_mode_seen = texture_mode_key(&self.shell);
        let samples = renderer::aa_samples(self.shell.cvar_f32("r_aaSamples"));
        let mut r = Renderer::new(
            window,
            &self.fs,
            hdr_cvar(&self.shell),
            self.textures,
            samples,
        )?;
        r.set_image_gamma(image_gamma(&mut self.shell, self.ignore_hw_gamma));
        if let Some(w) = &self.world {
            if let Some(lib) = &w.materials {
                r.set_material_lib(lib.clone());
            }
            r.load_world(&w.bsp, &self.fs)?;
            if w.fog.is_some() {
                r.set_fog(w.fog, 0.0);
            }
            // Offline there is no snapshot to place the brush models, so
            // every one stands where the map put it.
            if !matches!(self.mode, Mode::Online { .. }) {
                r.set_submodels(
                    &(1..w.bsp.models.len())
                        .map(|m| (m, glam::Mat4::IDENTITY))
                        .collect::<Vec<_>>(),
                );
            }
        }
        if !self.viewmodel.is_empty() {
            r.set_viewmodel(&self.fs, &self.viewmodel);
        }
        Ok(r)
    }

    /// `vid_restart` with a changed `vc_hdr`: every world pipeline targets
    /// the scene format, so the renderer is rebuilt and the map reloaded
    /// into it. The entity scene's model handles index the old renderer, so
    /// a live game starts a fresh one, as a gamestate does.
    fn rebuild_renderer(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        // the old surface must go before the new one is made
        self.renderer = None;
        match self.build_renderer(window) {
            Ok(r) => self.renderer = Some(r),
            Err(e) => return self.fail(event_loop, e),
        }
        if let Mode::Online {
            phase: Phase::Live(live),
            ..
        } = &mut self.mode
        {
            live.scene = entities::EntityScene::new();
            if let Some(r) = &mut self.renderer {
                r.set_hud_weapons(Some(&live.weapons));
            }
        }
        // The CoD2 draw's model handles died with the old renderer.
        self.session.reset_render();
        self.cod4_prewarm();
    }

    /// `r_textureMode` is read every frame (`RE_BeginFrame` tests its
    /// modified flag, 0x4de1db) and applied to every mipmapped image in
    /// place; vcod swaps the renderer's shared samplers. CoD4's
    /// `r_texFilterMipMode`, `r_texFilterAnisoMin` and
    /// `r_texFilterAnisoMax` are read every frame too
    /// (`RB_UpdateBackEndDvarOptions`, iw3mp.exe 0x6154e0).
    fn texture_mode_changed(&mut self) {
        let seen = texture_mode_key(&self.shell);
        if seen == self.texture_mode_seen {
            return;
        }
        self.texture_mode_seen = seen;
        let mode = self.shell.cvar("r_textureMode").unwrap_or_default();
        match texture_settings::TextureFilter::by_name(mode) {
            None => console::log::print("bad filter name"),
            Some(f) => {
                let f = cod4_filter(&self.shell, f);
                self.textures.filter = f;
                if let Some(r) = &mut self.renderer {
                    r.set_texture_filter(f);
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

    /// The drawn `serverTime` (`VCOD_FRAME_DIGEST`'s line,
    /// `--exec-in-game`): `None` with no game drawn.
    fn digest_time(&self) -> Option<i32> {
        if self.session.live() {
            return self.session.frame_time();
        }
        match &self.mode {
            Mode::Online {
                phase: Phase::Live(live),
                ..
            } => live.clock.delta().map(|_| live.clock.server_time()),
            _ => None,
        }
    }

    /// Where demos go and are looked for: the server's or the player's mod
    /// directory, then the base one the config sits in, each's `demos/`.
    fn demo_dirs(&self) -> Vec<std::path::PathBuf> {
        let base = self
            .config_path
            .parent()
            .map_or_else(|| self.game_dir.clone(), std::path::Path::to_path_buf);
        let mut dirs: Vec<_> = self
            .fs_game
            .iter()
            .chain(&self.fs_game_cvar())
            .map(|g| self.game_dir.join(g))
            .chain([base])
            .map(|d| d.join("demos"))
            .collect();
        dirs.dedup();
        dirs
    }

    /// `demo <name>` (CoDMP.exe 0x40eb40): leaves any server, then plays
    /// the file through the online mode with nothing sent
    /// (docs/research/cod11-demos.md).
    fn play_demo(&mut self, name: &str) {
        let Some((path, proto)) = play::demos::find(&self.demo_dirs(), name) else {
            console::log::print(&format!("Couldn't open demos/{name}"));
            return;
        };
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                console::log::print(&format!("Couldn't open {}: {e}", path.display()));
                return;
            }
        };
        if matches!(self.mode, Mode::Online { .. }) {
            self.disconnect(None);
        }
        log::info!("playing {}", path.display());
        let net = net::NetClient::play_demo(net::UdpTransport::none(), Instant::now(), proto, data);
        if self.hud.is_none() {
            self.hud = hud::Hud::new(&self.fs)
                .map_err(|e| log::warn!("hud: {e}, disabling the on-screen HUD"))
                .ok();
            self.localized = vcod_common::localize::Localized::load(&self.fs);
        }
        self.unload_world();
        self.ui.close_all();
        self.mode = online_mode(net, None, None);
        self.connect_addr = None;
        self.timedemo = Default::default();
        self.set_title(format!("{} — demo {name}", version_tag()));
        if self.console.open {
            self.toggle_console();
        }
    }

    /// `demo <name>` / `timedemo <name>` on a CoD2 install (`CL_PlayDemo_f`,
    /// Mac 0x14c288): `demos/<name>.dm_1` (a name already ending so kept)
    /// out of [`Self::demo_dirs`], any game left first, the session read
    /// to its gamestate and its map loaded as a server's
    /// (docs/research/cod2-demos.md). A missing file is
    /// `EXE_ERR_NOT_FOUND`.
    fn play_cod2_demo(&mut self, name: &str, timedemo: bool) {
        let file = vcod_cod2::net::demo::file_name(name);
        let path = self
            .demo_dirs()
            .into_iter()
            .map(|d| d.join(&file))
            .find(|p| p.is_file());
        if self.in_game() || self.session.busy() {
            self.disconnect(None);
        }
        let Some(path) = path else {
            return self.disconnect(Some(format!("EXE_ERR_NOT_FOUND\u{15}demos/{file}")));
        };
        log::info!("playing {}", path.display());
        let played = std::fs::read(&path)
            .map_err(anyhow::Error::from)
            .and_then(|data| play::cod2::Cod2Session::play_demo(data, timedemo))
            .and_then(|s| self.enter_cod2(s));
        if let Err(e) = played {
            return self.disconnect(Some(format!("{e:#}")));
        }
        self.connect_addr = None;
        self.set_title(format!("{} — demo {name}", version_tag()));
        if self.console.open {
            self.toggle_console();
        }
    }

    /// `Com_Frame`'s wait (CoDMP.exe 0x437f40): a frame starts once the
    /// millisecond clock has moved `frame_min_ms(com_maxfps)` past the last.
    fn pace_frame(&mut self) {
        let min = frame_min_ms(self.shell.cvar_f32("com_maxfps") as i32);
        loop {
            let now_ms = (Instant::now() - self.start).as_millis() as u64;
            if now_ms >= self.frame_ms + min {
                self.frame_ms = now_ms;
                return;
            }
            let due = self.start + std::time::Duration::from_millis(self.frame_ms + min);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
    }

    /// `CL_DemoCompleted` (CoDMP.exe 0x40e920): the `timedemo` line, the
    /// disconnect, then `CL_NextDemo` (0x40ed80), which runs `nextdemo`
    /// once and clears it.
    fn demo_completed(&mut self, event_loop: &ActiveEventLoop, now_ms: i32) {
        let line = (self.shell.cvar_f32("timedemo") != 0.0)
            .then(|| self.timedemo.report(now_ms))
            .flatten();
        self.demo_finished(event_loop, line);
    }

    /// `CL_DemoCompleted`'s tail on either game: the `timedemo` line when
    /// there is one, the disconnect, then `nextdemo`.
    fn demo_finished(&mut self, event_loop: &ActiveEventLoop, timedemo: Option<String>) {
        if let Some(line) = timedemo {
            console::log::print(&line);
        }
        self.disconnect(None);
        let next = self.shell.cvar("nextdemo").unwrap_or("").to_string();
        log::debug!("CL_NextDemo: {next}");
        if !next.is_empty() {
            let effects = self.shell.execute("set nextdemo \"\"");
            self.apply(event_loop, effects);
            let effects = self.shell.execute(&next);
            self.apply(event_loop, effects);
        }
    }

    /// `rcon` (CoDMP.exe 0x40f880): to the server once its `connectResponse`
    /// came, else to `rconAddress` (port 28960 when it names none). A bad
    /// address sends nothing, as retail's `NA_BAD` does. CoD2 and CoD4 log
    /// in instead (`Session::login_rcon`).
    fn rcon(&mut self, args: &[String]) {
        use console::rcon;
        if let Some(lines) = self
            .session
            .login_rcon(&mut self.cod4_rcon, &mut self.rcon, args)
        {
            lines.iter().for_each(|l| console::log::print(l));
            return;
        }
        let password = self.shell.cvar("rconPassword").unwrap_or("");
        let Some(packet) = rcon::packet(password, args) else {
            console::log::print(rcon::TOO_LONG);
            return;
        };
        if let Mode::Online { net, .. } = &mut self.mode
            && matches!(
                net.state(),
                net::NetState::LoadingGamestate | net::NetState::Active
            )
        {
            net.send_oob(&packet);
            return;
        }
        let address = self.shell.cvar("rconAddress").unwrap_or("");
        if address.is_empty() {
            console::log::print(rcon::NOT_CONNECTED);
            return;
        }
        match net::master::parse_address(address) {
            Some(to) => self.rcon.send(to, &packet),
            None => log::debug!("rcon: bad rconAddress {address:?}"),
        }
    }

    /// `connect`: leaves any server first, then joins `addr` the way
    /// `--connect` does. Like retail's `CL_Connect_f`, it closes the console.
    fn connect(&mut self, addr: String) {
        if matches!(self.mode, Mode::Online { .. }) {
            self.disconnect(None);
        }
        // Joining elsewhere ends Dedicated No's server, waiting or not.
        if self
            .server_process
            .as_ref()
            .is_some_and(|s| s.joined && addr != format!("127.0.0.1:{}", s.port))
        {
            self.server_process = None;
        }
        // A CoD4 install's search path reads as CoD2's (`iw_*.iwd`); its
        // session says which it is.
        if matches!(
            self.session,
            session::Session::Cod4(_) | session::Session::Cod4Menu(_)
        ) {
            let password = self.shell.cvar("password").unwrap_or("").to_string();
            return self.connect_cod4(addr, password);
        }
        if self.fs.game() == vcod_common::game::Game::Cod2 {
            return self.connect_cod2(addr);
        }
        let install = net::protocol::install_protocol(&self.fs);
        let mut net = match net::NetClient::connect_from(&addr, install) {
            Ok(net) => net,
            Err(e) => {
                log::error!("cannot open a socket to {addr}: {e:#}");
                return;
            }
        };
        net.set_userinfo(self.shell.userinfo());
        log::info!("connecting to {addr}");
        vcod_common::laps::begin_load(format!("cod1 connect {addr}"));
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
        self.set_title(format!("{} — connecting", version_tag()));
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
        // A CoD4 join still on the connect screen is called off, back to
        // the main menu when nothing says why.
        if reason.is_none() && self.cancel_cod4_join() {
            return;
        }
        if let Some(l) = self.cod4_loading.take() {
            l.cancel();
        }
        if let Mode::Online { net, .. } = &mut self.mode {
            net.disconnect();
        }
        self.leave_cod2();
        let mut reason = reason.map(|r| self.localized.message(&r));
        // A server Start New Server launched that died says why in its log.
        if let (Some(r), Some(server)) = (&mut reason, &mut self.server_process)
            && server.joined
            && let Some(status) = server.exited()
        {
            r.push_str(&format!(
                " (vcod-server {status}, see {})",
                server.log.display()
            ));
        }
        // Leaving the game stops the server Dedicated No started, as
        // retail's error path shuts its own down on every disconnect
        // (front-end doc section 18). A dedicated one keeps running.
        if self.server_process.as_ref().is_some_and(|s| s.joined) {
            self.server_process = None;
        }
        if let Some(reason) = &reason {
            log::error!("{reason}");
        }
        self.unload_world();
        // CoD4's search path is its own (`cod4_front::leave_cod4`).
        let cod4 = matches!(
            self.session,
            session::Session::Cod4(_) | session::Session::Cod4Menu(_)
        );
        if !cod4 {
            self.leave_fs_game();
        }
        self.mode = Mode::Idle;
        self.set_grab(false);
        self.set_title(version_tag());
        self.enter_menu(reason.as_deref());
    }

    /// Off a server's pure list at a disconnect. `CL_Disconnect` leaves
    /// the `fs_game` cvar, so a server's mod stays until a systeminfo or the
    /// Mods menu names another (docs/research/cod11-front-end.md, section 16).
    fn leave_fs_game(&mut self) {
        self.switch_fs_game(self.fs_game_cvar());
    }

    /// The `fs_game` cvar as the directory to layer over `mod_dir`: the
    /// Mods menu's pick or the last systeminfo's, whichever forced it last.
    fn fs_game_cvar(&self) -> Option<String> {
        net::download::mod_dir(self.shell.cvar("fs_game").unwrap_or(""), &self.mod_dir)
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
            &mut self.hud,
            &mut self.audio,
            &mut self.fx,
            &mut self.quick_chat,
        ) {
            Ok(()) => {
                if let Some(r) = &mut self.renderer {
                    r.reopen(&self.fs);
                }
                // Only a disconnect switches with a game up, and it drops
                // the game anyway.
                if let Err(e) = self.restart_ui() {
                    log::debug!("{e}");
                }
            }
            Err(e) => log::error!("cannot reopen {}: {e:#}", base.display()),
        }
    }

    /// `CL_Vid_Restart_f` refuses while `sv_running` (CoDMP.exe 0x40fbf8);
    /// the server Dedicated No started stands in for retail's own; a
    /// dedicated one is another process, as retail's would be.
    fn refuse_vid_restart(&mut self) -> bool {
        self.refuse_restart("Listen server cannot video restart.")
    }

    /// Prints `why` and answers true while a Dedicated No server we joined runs.
    fn refuse_restart(&mut self, why: &str) -> bool {
        let running = self
            .server_process
            .as_mut()
            .is_some_and(|s| s.joined && s.exited().is_none());
        if running {
            console::log::print(why);
        }
        running
    }

    /// The Mods menu's `RunMod` (and `Quake3` with `None`): `fs_game` and a
    /// `vid_restart`, which brings the main menu back up off the mod's
    /// paks. In a game, `vid_restart` keeps the connection: the search path
    /// reopens under the server's pure list, the UI reloads closed and the
    /// cgame half sends `cp` again (CoDMP.exe 0x40fbe0,
    /// docs/research/cod11-front-end.md section 17).
    /// CoD2's `Com_ChangePlayerProfile` (Mac 0x111936): `players/active.txt`
    /// names the profile, `com_playerProfile` holds it and its config file
    /// takes over, run when it has one, else written from what is set now.
    /// Retail resets every dvar first; vcod keeps what the file does not set.
    fn load_profile(&mut self, name: &str) {
        let players = self.game_dir.join(&self.mod_dir).join("players");
        if let Err(e) = std::fs::write(players.join("active.txt"), name) {
            log::warn!("cannot write {}: {e}", players.join("active.txt").display());
        }
        self.config_path = players.join(name).join(CONFIG_FILE);
        self.shell
            .execute(&format!("set com_playerProfile \"{name}\""));
        match std::fs::read_to_string(&self.config_path) {
            Ok(text) => {
                self.shell.exec_config(&text);
                if self.shell.upgrade_config(vcod_common::game::Game::Cod2).1
                    && let Err(e) = std::fs::write(&self.config_path, self.shell.config_text())
                {
                    log::warn!("cannot write {}: {e}", self.config_path.display());
                }
            }
            Err(_) => {
                if let Err(e) = std::fs::write(&self.config_path, self.shell.config_text()) {
                    log::warn!("cannot write {}: {e}", self.config_path.display());
                }
            }
        }
        log::info!("player profile {name}: {}", self.config_path.display());
    }

    fn run_mod(&mut self, game: Option<String>) {
        log::info!("fs_game {}", game.as_deref().unwrap_or("(none)"));
        self.shell.force("fs_game", game.as_deref().unwrap_or(""));
        // `RunMod` sets `fs_game` before the `vid_restart` that refuses, so
        // the pick waits for the next restart or the end of the game.
        if self.refuse_vid_restart() {
            return;
        }
        self.vid_restart_stop_sounds();
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
            if let Err(e) = self.reload_live_map() {
                self.disconnect(Some(format!("{e:#}")));
                return;
            }
        }
        if let Err(e) = self.restart_ui() {
            self.disconnect(Some(e));
            return;
        }
        self.after_menu();
        if let Mode::Online { view, .. } = &mut self.mode {
            view.reopen();
        }
        self.restart_cgame();
    }

    /// A live game's map read again off the reopened search path, as
    /// `vid_restart`'s `CL_InitCGame` (CoDMP.exe 0x4049c0) has the cgame's
    /// `CG_Init` (0x30022f90) load the collision map and the world anew: a
    /// mod's own `maps/mp/<map>.bsp` replaces the stock one. The live state
    /// starts fresh, as the cgame's does; snapshots keep flowing.
    fn reload_live_map(&mut self) -> Result<()> {
        let Mode::Online {
            net,
            phase: phase @ Phase::Live(_),
            ..
        } = &mut self.mode
        else {
            return Ok(());
        };
        let (bsp, world) = reread_live_map(&self.fs, net.configstrings())?;
        if let Phase::Live(live) = phase {
            live.world = world;
        }
        if let Some(r) = &mut self.renderer {
            r.load_world(&bsp, &self.fs)?;
        }
        self.world = Some(World {
            bsp,
            fog: None,
            materials: None,
        });
        Ok(())
    }

    /// An in-game `vid_restart`'s cgame half: `CL_ShutdownCGame` (CoDMP.exe
    /// 0x401a20) frees the cgame module and `CL_InitCGame` (0x4049c0)
    /// loads a fresh one, so the HUD, the kick, the effects, the event
    /// rings and the prediction start over; then `CL_SendPureChecksums`
    /// (0x40fac0) sends `cp`. Only past `CA_LOADING`, as retail
    /// (docs/research/cod11-front-end.md section 17, "The cgame restart").
    fn restart_cgame(&mut self) {
        let Mode::Online {
            net,
            view,
            input,
            predictor,
            phase,
            ..
        } = &mut self.mode
        else {
            return;
        };
        if !restart_live_cgame(
            phase,
            view,
            input,
            predictor,
            self.hud.as_mut(),
            &mut self.fx,
            &self.fs,
            net.configstrings(),
        ) {
            return;
        }
        // `CG_Init` replays configstring 3 (sound doc section 9).
        self.audio
            .set_ambient(&self.fs, net::info_value_for_key(net.configstring(3), "n"));
        if let Some(r) = &mut self.renderer {
            r.clear_marks();
            r.set_screen_blend(None);
            if let Phase::Live(live) = &*phase {
                r.set_hud_weapons(Some(&live.weapons));
            }
            prewarm_viewmodels(view, r, &self.fs, net.configstrings());
            if let Phase::Live(live) = &mut *phase {
                prewarm_level(&mut self.fx, r, &self.fs, net, live);
            }
        }
        let feed = net.gamestate().map_or(0, |g| g.checksum_feed);
        net.send_reliable(&self.fs.pure_command(feed, net.protocol().pure_dlls));
    }

    /// A CoD2 session's level change, before the frame: a server drop ends
    /// the game with its reason (the caller's error popup), and a gamestate for another map loads
    /// that map with the session on its pump (play::cod2).
    fn cod2_level(&mut self) -> Result<()> {
        let Some(map) = self.session.take_level_change()? else {
            return Ok(());
        };
        vcod_common::laps::begin_load(format!("cod2 map change {map}"));
        self.reopen_cod2_fs()?;
        let bsp = self.load_cod2_map(&map)?;
        self.world = Some(World {
            bsp,
            fog: None,
            materials: None,
        });
        self.fx.clear();
        self.session.reset_view_fx();
        self.audio.on_gamestate(&map);
        self.set_title(format!("{} — {map}", version_tag()));
        Ok(())
    }

    /// A CoD4 session's map change, before the frame: a gamestate naming
    /// another map loads it as the start-up does (`cod4_fly::load` over the
    /// gamestate's models and weapons) and hands the session its parts.
    fn cod4_level(&mut self) -> Result<()> {
        let Some(c) = self.session.cod4_mut() else {
            return Ok(());
        };
        let Some(map) = c.session.take_map_change() else {
            return Ok(());
        };
        vcod_common::laps::begin_load(format!("cod4 map change {map}"));
        let t0 = Instant::now();
        let ents = c.session.entity_assets();
        let fs_game = c.session.fs_game();
        let (mut m, hud) =
            cod4_fly::load_with_hud(&self.game_dir, fs_game.as_deref(), &map, &ents, None)
                .with_context(|| format!("cannot load CoD4 map '{map}'"))?;
        c.install_hud(hud);
        vcod_common::laps::span(t0, || "cod4 map".into());
        // The image search path follows the new level's `fs_game` and
        // usermap, as the zones do (cod4-fastfile.md 13).
        self.fs = cod4_fly::open_fs(&self.game_dir, fs_game.as_deref(), Some(&map))?;
        for (path, wav) in std::mem::take(&mut m.sound_files) {
            self.fs.overlay(&path, wav);
        }
        c.new_map(
            std::mem::take(&mut m.models),
            std::mem::take(&mut m.parts),
            std::mem::take(&mut m.weapons),
            m.bodies.take(),
            m.av.take(),
            m.world.take(),
            std::mem::take(&mut m.look),
        );
        let bsp = std::mem::take(&mut m.bsp);
        if let Some(r) = &mut self.renderer {
            r.set_material_lib(m.materials.clone());
            r.load_world(&bsp, &self.fs)?;
        }
        self.world = Some(World {
            bsp,
            fog: None,
            materials: Some(m.materials),
        });
        self.fx.clear();
        self.cod4_prewarm();
        self.set_title(format!("{} — {map}", version_tag()));
        Ok(())
    }

    /// The CoD4 level's first-use builds before its first frame, while the
    /// loading screen is still up ([`cod4_client::Cod4Client::prewarm`]).
    fn cod4_prewarm(&mut self) {
        let (Some(c), Some(r)) = (self.session.cod4_mut(), &mut self.renderer) else {
            return;
        };
        let mut cx = cod2_client::Cod2Frame {
            fs: &self.fs,
            shell: &self.shell,
            localized: &self.localized,
            audio: &mut self.audio,
            fx: &mut self.fx,
            ui: &self.ui,
            r,
            console_open: self.console.open,
            time: 0.0,
            dt: 0.0,
        };
        let t0 = Instant::now();
        c.prewarm(&mut cx);
        vcod_common::laps::span(t0, || "cod4 prewarm".into());
    }

    /// The CoD4 join's end, once it comes: its session and loaded map
    /// take over as the start-up's do (the search path under the level's
    /// `fs_game`, the session's parts, the world, the camera).
    fn cod4_join_frame(&mut self) -> Result<()> {
        let Some(l) = &mut self.cod4_loading else {
            return Ok(());
        };
        let Some(done) = l.poll() else {
            return Ok(());
        };
        vcod_common::laps::mark(|| "cod4 join thread done".into());
        self.cod4_loading = None;
        let cod4_loading::Joined {
            session,
            mut map,
            map_name,
            fs_game,
            hud,
        } = match done {
            Ok(j) => j,
            Err(e) => {
                self.cod4_join_failed(&format!("{e:#}"));
                return Ok(());
            }
        };
        self.fs = cod4_fly::open_fs(&self.game_dir, fs_game.as_deref(), Some(&map_name))?;
        for (path, wav) in std::mem::take(&mut map.sound_files) {
            self.fs.overlay(&path, wav);
        }
        let mut client = cod4_client::Cod4Client::new(
            session,
            std::mem::take(&mut map.models),
            std::mem::take(&mut map.parts),
            std::mem::take(&mut map.weapons),
            map.bodies.take(),
            map.av.take(),
            map.world.take(),
            std::mem::take(&mut map.look),
        );
        client.install_hud(hud);
        // The front end waits for the game to end (`cod4_front`); its music
        // ends with it.
        let front = std::mem::replace(&mut self.session, session::Session::Cod4(Box::new(client)));
        if let session::Session::Cod4Menu(f) = front {
            self.cod4_parked = Some(f);
        }
        self.audio.music_stop(0);
        if self.console.open {
            self.toggle_console();
        }
        let bsp = std::mem::take(&mut map.bsp);
        if let Some(r) = &mut self.renderer {
            r.set_material_lib(map.materials.clone());
            r.load_world(&bsp, &self.fs)?;
        }
        self.world = Some(World {
            bsp,
            fog: None,
            materials: Some(map.materials),
        });
        self.audio.on_gamestate(&map_name);
        self.cod4_prewarm();
        let (origin, yaw) = map.spawn;
        self.mode = Mode::Cod2(FlyCamera::new(Vec3::from(origin) + Vec3::Z * 60.0, yaw));
        self.set_title(format!("{} — {map_name}", version_tag()));
        self.set_grab(true);
        Ok(())
    }

    /// Drops the map, its sounds and effects, between servers.
    fn unload_world(&mut self) {
        if let Some(r) = &mut self.renderer {
            r.unload_world();
            r.set_hud_weapons(None);
            r.set_screen_blend(None);
            r.set_world_blur(0.0);
        }
        self.world = None;
        self.fx.clear();
        self.audio.on_gamestate("");
        if let Some(hud) = &mut self.hud {
            leave_server_hud(hud);
        }
    }

    fn set_title(&mut self, title: String) {
        if let Some(window) = &self.window {
            window.set_title(&title);
        }
        self.title = title;
    }

    /// Esc in a game with no menu up, as `CL_KeyEvent` does it
    /// (docs/research/cod11-front-end.md section 13): once the game is live
    /// the `g_scriptMainMenu` script menu (`UI_SetActiveMenu` case 2; vcod
    /// falls back to the in-game main menu when the server named none it
    /// loaded), before that the main menu. The game keeps running.
    fn escape_in_game(&mut self) {
        let Mode::Online {
            net, join, phase, ..
        } = &mut self.mode
        else {
            return;
        };
        let mut out = Vec::new();
        // A demo's Esc opens the main menu (`UI_SetActiveMenu` 1,
        // CoDMP.exe 0x40dc30).
        if matches!(phase, Phase::Live(_)) && !net.is_demo() {
            match join
                .main_menu(net.configstrings(), &self.shell)
                .filter(|n| self.ui.has_menu(n))
            {
                Some(name) => {
                    self.ui
                        .set_server_cvars(join.cvars.all(net.configstrings(), &self.shell));
                    self.ui.open_script_main(&name, &self.shell, &mut out);
                }
                None => self.ui.open_ingame(&self.shell, &mut out),
            }
        } else {
            self.ui.open_main(&self.shell, &mut out);
        }
        self.ui_pending.extend(out);
        self.grab_before_console = false;
        self.sync_menu_mouse();
    }

    /// Esc in a CoD2 game with no menu up: `UI_SetActiveMenu` case 2
    /// (Mac 0x15118e) opens the menu `g_scriptMainMenu` names, else the
    /// main menu over the game.
    fn escape_in_cod2(&mut self) {
        let mut out = Vec::new();
        if !self.session.escape(&mut self.ui, &self.shell, &mut out) {
            return;
        }
        self.ui_pending.extend(out);
        self.grab_before_console = false;
        self.sync_menu_mouse();
    }

    /// The CoD2 session's script menus, once a frame: each gamestate
    /// loads the precached ones into the UI (closing what was up), the
    /// `v` the session left to the player set cvars, its `t`, `u` and `K`
    /// go through [`frontend::cod2::menu_command`], and a refused `t` is
    /// offered again.
    fn cod2_script_menus(&mut self) {
        if self.session.script_menus(
            &mut self.ui,
            &self.fs,
            &mut self.shell,
            &mut self.ui_pending,
        ) {
            self.sync_menu_mouse();
        }
    }

    fn playing_demo(&self) -> bool {
        matches!(&self.mode, Mode::Online { net, .. } if net.is_demo())
            || self.session.playing_demo()
    }

    /// A key under 128 or MOUSE1 in a demo with no menu, console or chat
    /// field up is Esc, and clears `nextdemo` (CoDMP.exe 0x40dc30).
    fn demo_key(&mut self) {
        self.shell.execute("set nextdemo \"\"");
        self.escape();
    }

    /// Esc with no menu up: the in-game menus online, a mouse release
    /// offline.
    fn escape(&mut self) {
        // A CoD4 session's menus took Esc first (`Cod4Client::menu_key`):
        // here it frees the mouse. On the connect screen it calls the join
        // off.
        if self.cancel_cod4_join() {
            return;
        }
        // A CoD4 demo leaves for the main menu, as CoD2's does
        // (cod4-demos.md 5).
        if matches!(self.session, session::Session::Cod4(_)) && self.session.playing_demo() {
            self.disconnect(None);
            return;
        }
        if self.session.live() && !matches!(self.session, session::Session::Cod4(_)) {
            self.escape_in_cod2();
        } else if matches!(self.mode, Mode::Online { .. }) {
            self.escape_in_game();
        } else {
            self.set_grab(false);
            self.clear_held_keys();
        }
    }

    /// `cl_bypassMouseInput` 1, which the quick message menus set: the
    /// mouse stays on the view and its buttons reach the binds
    /// (CoDMP.exe 0x40b0a0, 0x40dfa7).
    fn bypass_mouse(&self) -> bool {
        self.shell.cvar_f32("cl_bypassMouseInput") == 1.0
    }

    /// Whether a key goes to the open menus: every key, unless
    /// `cl_bypassMouseInput` passes on the ones the focused menu has no
    /// `execKey` for (CoDMP.exe 0x40dfa7, UI `vmMain` 14). Esc always
    /// reaches the menus (0x40ddca).
    fn menu_takes_key(&self, code: KeyCode) -> bool {
        self.menu_active()
            && (code == KeyCode::Escape
                || self.shell.cvar_f32("cl_bypassMouseInput") == 0.0
                || self.ui.wants_key(code))
    }

    /// Whether the open menus take the mouse's motion, buttons and wheel.
    fn menu_takes_mouse(&self) -> bool {
        self.menu_active() && !self.bypass_mouse()
    }

    /// Gives the mouse to the menus over a game while they hold it, and
    /// back to the view once they let go; the held keys let go as the
    /// menus take it.
    fn sync_menu_mouse(&mut self) {
        if !self.in_game() {
            self.menu_mouse = false;
            return;
        }
        // The UI sets the bypass back to 0 once no menu has the focus
        // (0x4000a170), so a menu closed without its `onClose` does not
        // leave it on.
        if !self.ui.active() && self.shell.cvar_f32("cl_bypassMouseInput") != 0.0 {
            self.shell.execute("set cl_bypassMouseInput 0");
        }
        let wants = self.ui.active() && !self.bypass_mouse();
        if wants {
            if !self.menu_mouse {
                self.menu_mouse = true;
                self.set_grab(false);
                self.clear_held_keys();
            }
        } else if std::mem::take(&mut self.menu_mouse) && !self.console.open {
            self.set_grab(true);
        }
    }

    /// The last menu over a game closed (Back to Game, Esc): the mouse goes
    /// back to the game unless the menus or the console still hold it.
    fn after_menu(&mut self) {
        if self.in_game() && !(self.ui.active() && !self.bypass_mouse()) && !self.console.open {
            self.menu_mouse = false;
            self.set_grab(true);
        }
    }
}

/// The cgame's script menu precache at its start (cgame_mp_x86.dll
/// 0x3002c060): each menu configstrings 1180..1211 name loads into the UI.
/// A missing file is the cgame's `Com_Error(ERR_DROP)`, whose text is the
/// error the caller drops the game with.
fn load_script_menus(
    ui: &mut frontend::Ui,
    fs: &dyn AssetSource,
    shell: &console::shell::Shell,
    configstrings: &[String],
    cs: &net::protocol::CsLayout,
) -> Result<(), String> {
    let language = shell.cvar("cl_language").unwrap_or("");
    let names = play::join::script_menu_names(configstrings, cs);
    match ui.load_script_menus(fs, &names, language).first() {
        Some(name) => Err(format!("Could not load script menu file '{name}'")),
        None => Ok(()),
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 900.0));
        // CoD4 1.7's lists are built alike (`%ix%i` of at least 640 x 480,
        // `%i Hz`; iw3mp.exe 0x5f42f0, 0x5f4305, 0x5f43f5).
        if matches!(
            self.fs.game(),
            vcod_common::game::Game::Cod2 | vcod_common::game::Game::Cod4
        ) {
            let monitor = event_loop
                .primary_monitor()
                .or_else(|| event_loop.available_monitors().next());
            set_cod2_mode_lists(&mut self.shell, monitor);
        }
        let mode = video_mode_or(self.window_size, &self.shell);
        self.ignore_hw_gamma = ignore_hw_gamma(&self.shell);
        if let Some((w, h)) = mode.size {
            attrs = attrs.with_inner_size(winit::dpi::PhysicalSize::new(w, h));
        }
        let t_window = Instant::now();
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                return self.fail(
                    event_loop,
                    anyhow::Error::new(e).context("cannot create a window"),
                );
            }
        };
        // Full screen once the window knows its monitor.
        if mode.fullscreen != video::Fullscreen::Windowed {
            apply_video_mode(&window, mode);
        }
        self.refresh_mode_lists(&window);
        match self.build_renderer(window.clone()) {
            Ok(r) => self.renderer = Some(r),
            Err(e) => return self.fail(event_loop, e),
        }
        vcod_common::laps::span(t_window, || "window and renderer".into());
        // The title was built before the backend was known.
        self.title = self.title.replacen(TITLE, &version_tag(), 1);
        window.set_title(&self.title);
        window.request_redraw();
        self.window = Some(window);
        self.last_frame = Instant::now();
        for line in std::mem::take(&mut self.exec) {
            let effects = self.shell.execute(&line);
            self.apply(event_loop, effects);
        }
    }

    // The loop runs `ControlFlow::Poll`, so this sees a signal within a frame.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if quit::requested() {
            event_loop.exit();
        }
        self.poll_server_join();
        self.poll_cod2_connect();
        self.cod4_front_requests();
        for text in self
            .rcon
            .poll()
            .into_iter()
            .chain(self.session.take_prints())
        {
            console::log::print(&text);
        }
        for line in self.session.take_menu_console() {
            self.ui_pending.push(frontend::UiEffect::Command(line));
        }
        let pending = std::mem::take(&mut self.ui_pending);
        self.ui_effects(event_loop, pending);
        self.sync_menu_mouse();
        self.sync_capture();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // Alt+Enter flips `r_fullscreen` and restarts the video
                // ahead of the console and binds (video::toggled). Its
                // release goes on, as retail's `WM_SYSKEYUP` does
                // (cod11-front-end.md 17, "Alt+Enter's release").
                if pressed
                    && !event.repeat
                    && self.modifiers.alt_key()
                    && matches!(code, KeyCode::Enter | KeyCode::NumpadEnter)
                {
                    let on = video::toggled(self.shell.cvar_f32("r_fullscreen") as i32);
                    let effects = self
                        .shell
                        .execute(&format!("r_fullscreen {on}; vid_restart"));
                    self.apply(event_loop, effects);
                    return;
                }
                // ` and ~ toggle the console whatever they are bound to
                // (CoDMP.exe 0x40dc30).
                // CoD4's `monkeytoy` 1 (Enable Console No) keeps a closed
                // console closed (iw3mp.exe 0x467fb9; cod4-front-end.md 9).
                if code == KeyCode::Backquote {
                    let restricted = self.game == vcod_common::game::Game::Cod4
                        && !self.console.open
                        && self.shell.cvar_f32("monkeytoy") != 0.0;
                    if pressed && !event.repeat && !restricted {
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
                        KeyCode::Escape if self.console.open && self.in_game() => {
                            // A CoD4 session's script menus take it first
                            // (`Cod4Client::menu_key`), then it frees the
                            // mouse.
                            if matches!(self.session, session::Session::Cod4(_)) {
                                if self.session.chat_key(code, None) != Some(true) {
                                    self.escape();
                                }
                            } else if self.ui.active() {
                                let (_, effects) = self.ui.key(code, &self.shell);
                                self.ui_effects(event_loop, effects);
                            } else {
                                self.escape();
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
                if self.menu_takes_key(code) {
                    if event.repeat {
                        if pressed {
                            let effects =
                                self.ui.key_repeat(code, event.text.as_deref(), &self.shell);
                            self.ui_effects(event_loop, effects);
                        }
                        return;
                    }
                    if !pressed {
                        let effects = self.ui.key_up(&self.shell);
                        self.ui_effects(event_loop, effects);
                        self.after_menu();
                        return;
                    }
                    self.ui.release();
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
                    let (used, effects) = self.ui.key(code, &self.shell);
                    self.ui_effects(event_loop, effects);
                    self.after_menu();
                    if used || !matches!(code, KeyCode::F3 | KeyCode::F4) {
                        return;
                    }
                }
                if pressed && self.chat_key(code, event.text.as_deref()) {
                    return;
                }
                if pressed
                    && self.playing_demo()
                    && console::keys::key_name(code)
                        .is_some_and(|k| console::keys::number(k) < 0x80)
                {
                    self.demo_key();
                    return;
                }
                if pressed && !event.repeat && self.eat_stray_key() {
                    return;
                }
                // auto-repeat would retrigger the jump and walk's prone toggle
                if event.repeat {
                    return;
                }
                if pressed && code == KeyCode::Escape {
                    self.escape();
                    return;
                }
                // vcod's own: M opens the script menu Esc does.
                if pressed
                    && code == KeyCode::KeyM
                    && !self.ui.active()
                    && matches!(
                        self.mode,
                        Mode::Online {
                            phase: Phase::Live(_),
                            ..
                        }
                    )
                {
                    self.escape_in_game();
                    return;
                }
                if code == KeyCode::F3 && pressed {
                    self.debug_overlay = !self.debug_overlay;
                    return;
                }
                if code == KeyCode::F4 && pressed {
                    self.cull_mode = self.cull_mode.next();
                    return;
                }
                if self.plays_binds() {
                    if let Some(key) = console::keys::key_name(code) {
                        self.bound_key(event_loop, key, pressed);
                    }
                    return;
                }
                let grabbed = self.grabbed;
                match &mut self.mode {
                    Mode::Idle | Mode::Online { .. } => {}
                    Mode::Fly(_) | Mode::Cod2(_) => match code {
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
                // Only where the pointer cannot be captured: a captured one
                // moves the cursor by its deltas (`device_event`).
                if !self.captured
                    && let Some(r) = &self.renderer
                {
                    let at = [position.x as f32, position.y as f32];
                    if self
                        .session
                        .cod4_menu_mouse(None, Some(at), r.screen_size())
                    {
                        return;
                    }
                }
                if !self.captured
                    && self.menu_takes_mouse()
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
                if self.menu_takes_mouse() {
                    if pressed
                        && self.ui.waiting_for_key()
                        && let Some(key) = console::keys::mouse_name(button)
                    {
                        let effects = self.ui.bind_key(key, &self.shell);
                        self.ui_effects(event_loop, effects);
                    } else if pressed && let Some(key) = console::keys::mouse_name(button) {
                        let key = console::keys::number(key);
                        let effects = self.ui.mouse_button(key, Instant::now(), &self.shell);
                        self.ui_effects(event_loop, effects);
                        self.after_menu();
                    } else if !pressed {
                        let effects = self.ui.key_up(&self.shell);
                        self.ui_effects(event_loop, effects);
                        self.after_menu();
                    }
                    return;
                }
                if self.console_active() {
                    return;
                }
                // A CoD4 script menu takes the buttons; MOUSE1 clicks it, a
                // waiting bind row takes any (`Cod4Client::menu_button`).
                if self.session.cod4_menu_has_mouse() {
                    if !pressed && button == MouseButton::Left {
                        self.session.cod4_menu_release();
                    }
                    if pressed
                        && let Some(key) = console::keys::mouse_name(button)
                        && let Some(r) = &self.renderer
                    {
                        let size = r.screen_size();
                        self.session.cod4_menu_button(key, size);
                    }
                    return;
                }
                if button == MouseButton::Left && pressed && self.playing_demo() {
                    self.demo_key();
                    return;
                }
                // the click that captures the mouse must not also fire
                if button == MouseButton::Left && pressed && !self.grabbed {
                    self.set_grab(true);
                    return;
                }
                if pressed && self.eat_stray_key() {
                    return;
                }
                if self.plays_binds() {
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
                    Mode::Idle | Mode::Online { .. } | Mode::Fly(_) | Mode::Cod2(_) => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // One press a whole notch (console::keys::WheelNotches).
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => self.wheel.step(Some(y), None),
                    MouseScrollDelta::PixelDelta(p) => self.wheel.step(None, Some(p.y as f32)),
                } as f32;
                if self.menu_takes_mouse() {
                    if scroll != 0.0 && self.ui.waiting_for_key() {
                        let key = if scroll > 0.0 {
                            "MWHEELUP"
                        } else {
                            "MWHEELDOWN"
                        };
                        let effects = self.ui.bind_key(key, &self.shell);
                        self.ui_effects(event_loop, effects);
                    } else if scroll != 0.0 {
                        let effects = self.ui.wheel(scroll < 0.0, &self.shell);
                        self.ui_effects(event_loop, effects);
                    }
                    return;
                }
                if self.console_active() {
                    if scroll != 0.0 {
                        self.console.scroll(scroll > 0.0);
                    }
                    return;
                }
                if self.session.cod4_menu_has_mouse() {
                    if scroll != 0.0
                        && let Some(r) = &self.renderer
                    {
                        let key = if scroll > 0.0 {
                            "MWHEELUP"
                        } else {
                            "MWHEELDOWN"
                        };
                        let size = r.screen_size();
                        self.session.cod4_menu_button(key, size);
                    }
                    return;
                }
                if self.plays_binds() {
                    if self.grabbed && scroll != 0.0 {
                        let key = if scroll < 0.0 {
                            "MWHEELDOWN"
                        } else {
                            "MWHEELUP"
                        };
                        if self.eat_stray_key() {
                            return;
                        }
                        for _ in 0..scroll.abs() as i32 {
                            self.bound_key(event_loop, key, true);
                            self.bound_key(event_loop, key, false);
                        }
                    }
                } else if let Mode::Fly(cam) | Mode::Cod2(cam) = &mut self.mode {
                    cam.adjust_speed(scroll);
                }
            }
            WindowEvent::RedrawRequested => {
                {
                    let _pace = frame_prof::span(frame_prof::Stage::Pace);
                    self.pace_frame();
                }
                let frame_start = Instant::now();
                self.mouse_frame();
                self.cod2_script_menus();
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;
                let elapsed = now - self.start;
                let time = elapsed.as_secs_f32();
                let local_ms = elapsed.as_secs_f64() * 1000.0;
                let cull = self.cull_mode;
                let blood = self.shell.cvar_f32("cg_blood") as i32 != 0;
                let eject_brass = self.shell.cvar_f32("cg_brass") as i32 != 0;
                self.texture_mode_changed();
                self.audio
                    .set_master_volume(self.shell.cvar_f32(audio::volume_cvar(&self.shell)));
                let demo_dirs = self.demo_dirs();
                let gamma = gamma_cvar(&mut self.shell);
                let fullscreen = self
                    .window
                    .as_ref()
                    .is_some_and(|w| w.fullscreen().is_some());
                let overbright =
                    gamma::overbright_bits(self.fs.game(), fullscreen, !self.ignore_hw_gamma);
                // A drop or a failed map load ends the game for the main
                // menu with its reason, as a CoD1 server's does.
                if let Err(e) = self.cod2_level() {
                    self.disconnect(Some(format!("{e:#}")));
                }
                // The join in flight: its session and map once loaded; a
                // refused or failed join or load comes back to the main
                // menu with its reason.
                if let Err(e) = self.cod4_join_frame() {
                    self.unload_world();
                    self.cod4_join_failed(&format!("{e:#}"));
                }
                // A CoD4 map that fails to load ends the game for the main
                // menu with its reason.
                if let Err(e) = self.cod4_level() {
                    self.disconnect(Some(format!("{e:#}")));
                }
                // A CoD4 drop (`w`, an out-of-band `disconnect`, `cl_timeout`
                // of silence) ends the game for the main menu, its reason
                // the server's notice (`Com_SetErrorMessage`).
                if let Some(why) = self
                    .session
                    .cod4_mut()
                    .and_then(|c| c.session.take_dropped())
                {
                    let why = play::cod4::localize_drop(&why, &self.game_dir);
                    log::error!("{why}");
                    self.leave_cod4_game(Some((&why, true)));
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                // The CoD4 systeminfo's dvars, `sv_cheats` among them
                // (`CL_SystemInfoChanged`, iw3mp.exe 0x473ab0).
                if let Some(si) = self
                    .session
                    .cod4_mut()
                    .and_then(|c| c.session.take_systeminfo())
                {
                    let out = self.shell.apply_systeminfo(&si);
                    self.apply(event_loop, out);
                }
                // A CoD2 demo played out (`CL_DemoCompleted`, Mac 0x14c096).
                if let Some(line) = self.session.demo_over() {
                    self.demo_finished(event_loop, line);
                    return;
                }
                let Some(r) = &mut self.renderer else { return };
                r.set_gamma(if self.ignore_hw_gamma { 1.0 } else { gamma }, overbright);
                r.set_bicubic(self.shell.cvar_f32("vc_bicubic") != 0.0);
                r.set_bloom(
                    self.shell.cvar_f32("vc_bloom"),
                    self.shell.cvar_f32("vc_bloomThreshold"),
                );
                r.set_ssao(self.shell.cvar_f32("vc_ssao"));
                r.set_soft_particles(self.shell.cvar_f32("vc_softParticles"));
                r.set_shadows(self.shell.cvar_f32("vc_shadows") != 0.0);
                r.set_sun_shadow(
                    self.shell.cvar_f32("sm_enable") != 0.0,
                    vcod_cod4::sunshadow::Settings {
                        sample_size_near: self.shell.cvar_f32("sm_sunSampleSizeNear"),
                        scale: self.shell.cvar_f32("sm_sunShadowScale"),
                        center: self
                            .shell
                            .cvar("sm_sunShadowCenter")
                            .and_then(vcod_common::bsp::parse_vec3)
                            .map_or(glam::Vec3::ZERO, glam::Vec3::from),
                    },
                );
                let on = |n: &str| self.shell.cvar_f32(n) != 0.0;
                r.set_spot_shadow(crate::cod4_spotshadow::Settings {
                    on: on("r_spotLightShadows") && on("sm_spotEnable"),
                    entities: on("r_spotLightEntityShadows"),
                    smodels: on("r_spotLightSModelShadows"),
                    max_lights: (self.shell.cvar_f32("sm_maxLights").max(0.0) as usize)
                        .min(crate::cod4_spotshadow::MAX_LIGHTS),
                    fade_time: self
                        .shell
                        .cvar_f32("sm_spotShadowFadeTime")
                        .clamp(0.01, 5.0),
                    eye_project_dist: self.shell.cvar_f32("sm_lightScore_eyeProjectDist"),
                    spot_project_frac: self.shell.cvar_f32("sm_lightScore_spotProjectFrac"),
                    offset_bias: self.shell.cvar_f32("sm_polygonOffsetBias"),
                    offset_scale: self.shell.cvar_f32("sm_polygonOffsetScale"),
                });
                r.set_cookie_settings(cookie_settings(&self.shell));
                r.set_post(post::PostSettings {
                    grade: self.shell.cvar_f32("vc_grade"),
                    vignette: self.shell.cvar_f32("vc_vignette"),
                    grain: self.shell.cvar_f32("vc_grain"),
                });
                r.set_flares(self.shell.cvar_f32("r_flares") as i32 != 0);
                r.set_draw_sun(self.shell.cvar_f32("r_drawSun") as i32 != 0);
                r.set_swap_interval(self.shell.cvar_f32("r_swapInterval") as i32 != 0);
                // CoD4's Model Detail sets the rigid pair (iw3mp.exe
                // 0x62a29c; cod4-front-end.md 9); vcod has one LOD scale.
                let (lod_scale, lod_bias) = if self.game == vcod_common::game::Game::Cod4 {
                    ("r_lodScaleRigid", "r_lodBiasRigid")
                } else {
                    ("r_lodscale", "r_lodbias")
                };
                r.set_lod(
                    self.shell.cvar_f32(lod_scale),
                    self.shell.cvar_f32(lod_bias),
                );
                if let Some(c) = self.session.cod4_mut() {
                    c.session.menus.listen_server = self
                        .server_process
                        .as_mut()
                        .is_some_and(|s| s.joined && s.exited().is_none());
                }
                // CoD4's views project at `r_aspectRatio`'s aspect, the
                // value its last `vid_restart` latched, and its 2D layer
                // places by the same `vidConfig` (camera::Cod4Screen)
                r.set_view_aspect((self.game == vcod_common::game::Game::Cod4).then(|| {
                    let (w, h) = r.screen_size();
                    let setting = self.shell.cvar("r_aspectRatio").unwrap_or_default();
                    let screen = camera::Cod4Screen::of(
                        setting,
                        w,
                        h,
                        self.shell.cvar_f32("r_fullscreen") as i32 != 0,
                        cvar_on(&self.shell, "vc_windowAspect"),
                    );
                    hud::cod2elem::set_cod4_screen(&screen);
                    screen.aspect
                }));
                r.set_cod4_options(cod4_options(&self.shell));
                r.set_depth_prepass(cvar_on(&self.shell, "r_vc_depthprepass"));
                r.set_dynamic_lights(
                    self.shell.cvar_f32("r_dynamiclight") as i32,
                    self.shell.cvar_f32("r_dlightQuality") as i32 != 0,
                );
                r.set_sun_shafts(self.shell.cvar_f32("vc_sunShafts"));
                let aspect = r.aspect();
                // vcod's Hor+: every fov below is a 4:3 view's width until
                // the frame widens it (camera::view_fov_x).
                let widescreen = self.shell.cvar_f32("vc_widescreen") != 0.0;
                // Set inside the online arm where `self` is borrowed out
                // field-by-field; acted on once the borrows end.
                let mut fatal: Option<anyhow::Error> = None;
                // The demo played out: back to the menu, no error.
                let mut demo_over = false;
                // What the mode draws on the 2D layer; the console goes on top.
                let mut hud_quads: Vec<hud::HudQuad> = Vec::new();
                if let Some(l) = &mut self.cod4_loading {
                    hud_quads.extend(l.quads(r.screen_size(), elapsed.as_millis() as i32));
                }
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
                            fov_x: camera::DEFAULT_FOV_DEG,
                            cull,
                            hud_lines: Vec::new(),
                        },
                        None,
                    ),
                    Mode::Fly(cam) => {
                        self.look_zoom = (1.0, false);
                        cam.update(&self.input, dt);
                        if let Some(av) = &mut self.cod4_fly {
                            av.frame(&self.fs, &mut self.audio, &mut self.fx, f64::from(time));
                            r.set_level_look(Some(av.look.at((time * 1000.0) as i32, false)));
                        }
                        let fly_fov =
                            camera::view_fov_x(camera::DEFAULT_FOV_DEG, aspect, widescreen);
                        let view = cod2_client::DrawView {
                            yaw: cam.yaw,
                            pitch: cam.pitch,
                            roll: 0.0,
                            fov: fly_fov,
                            vm: None,
                        };
                        ViewTail {
                            fs: &self.fs,
                            shell: &self.shell,
                            audio: &mut self.audio,
                            fx: &mut self.fx,
                            session: &self.session,
                            r,
                            time,
                            dt,
                            aspect,
                            cull,
                        }
                        .run(
                            cam,
                            view,
                            fly_fov,
                            &mut hud_quads,
                            &mut self.fx_ms,
                        )
                    }
                    Mode::Cod2(cam) => {
                        self.session.finish_pump();
                        self.session.set_fly_keys(&self.input);
                        let mut cod2_vm = None;
                        let mut cod2_fx = None;
                        let mut cx = cod2_client::Cod2Frame {
                            fs: &self.fs,
                            shell: &self.shell,
                            localized: &self.localized,
                            audio: &mut self.audio,
                            fx: &mut self.fx,
                            ui: &self.ui,
                            r: &mut *r,
                            console_open: self.console.open,
                            time,
                            dt,
                        };
                        if self.session.pump(&mut cx, cam) {
                            let hud_span = frame_prof::span(frame_prof::Stage::Hud);
                            (cod2_vm, hud_quads) = self.session.scene(&mut cx, cam);
                            drop(hud_span);
                            let events = self.session.events(&mut cx);
                            self.session.audio(&mut cx, &events, cam.pos);
                            self.session.effects(&mut cx, &events, cam.pos);
                            if let Some(out) = self.session.view(&mut cx, &events, cam) {
                                self.look_zoom = (
                                    out.fov / camera::DEFAULT_FOV_DEG * out.shock.mouse.sensitivity,
                                    false,
                                );
                                cod2_fx = Some(out);
                            }
                        } else {
                            self.look_zoom = (1.0, false);
                            self.session.idle();
                            cam.update(&self.input, dt);
                        }
                        let fly_fov =
                            camera::view_fov_x(camera::DEFAULT_FOV_DEG, aspect, widescreen);
                        let view = self.session.draw_view(
                            &self.fs,
                            r,
                            cam,
                            cod2_fx.as_ref(),
                            cod2_vm,
                            (fly_fov, aspect, widescreen),
                        );
                        frame_prof::count(frame_prof::Count::Particles, self.fx.counts().0);
                        ViewTail {
                            fs: &self.fs,
                            shell: &self.shell,
                            audio: &mut self.audio,
                            fx: &mut self.fx,
                            session: &self.session,
                            r,
                            time,
                            dt,
                            aspect,
                            cull,
                        }
                        .run(
                            cam,
                            view,
                            fly_fov,
                            &mut hud_quads,
                            &mut self.fx_ms,
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
                    } => {
                        // `VCOD_FRAME_PROFILE`'s stages through the frame; the
                        // player bodies inside `build_instances` are `bodies`.
                        let mut prof = frame_prof::span(frame_prof::Stage::Net);
                        let mut vm = None;
                        // A demo is read up to the clock (`CL_SetCGameTime`,
                        // CoDMP.exe 0x404d60), nothing while a map loads.
                        let mut demo_clock = play::clock::DemoClock::default();
                        let events = if net.is_demo() {
                            let knobs = play::demos::DemoKnobs {
                                freeze: self.shell.cvar_f32("cl_freezeDemo") != 0.0,
                                timedemo: self.shell.cvar_f32("timedemo") != 0.0,
                                nudge: self.shell.cvar_f32("cl_timeNudge") as i32,
                            };
                            match &mut *phase {
                                Phase::Live(live) => {
                                    let (events, how) = play::demos::read_frame(
                                        net,
                                        &mut live.clock,
                                        &mut self.timedemo,
                                        now,
                                        local_ms as i32,
                                        knobs,
                                    );
                                    demo_clock = how;
                                    events
                                }
                                _ => net.pump_demo(now, None),
                            }
                        } else {
                            net.pump()
                        };
                        let mut gamestate_ready = false;
                        for ev in &events {
                            if let net::NetEvent::Chat { team, .. } = ev
                                && !hud::chat::takes(*team, self.shell.cvar("cg_teamChatsOnly"))
                            {
                                continue;
                            }
                            if let Some(hud) = &mut self.hud {
                                let p = net.protocol();
                                let bound =
                                    |cmd: &str| bound_key(&self.shell, &self.localized, p, cmd);
                                hud.on_net_event(ev, time, &self.localized, &bound);
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
                                net::NetEvent::DemoEnded(_) => demo_over = true,
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
                                // `j/k/l` is quick chat; `s <idx>` is the announcer.
                                net::NetEvent::ServerCommand(tokens) => {
                                    // `CG_MapRestart` (cgame 0x3002ca60) frees
                                    // the local entities, the marks and every
                                    // effect, stops all sounds, restarts the
                                    // ambient and stands the player up running
                                    // (events doc 9.8).
                                    if play::join::is_map_restart(tokens) {
                                        join.on_restart();
                                        let out = join.cvars.on_map_restart(&mut self.shell);
                                        cvar_effects(&self.shell, net, &self.config_path, out);
                                        // `cg.mapRestart`: the next snapshot runs
                                        // the respawn reset (0x30028a70) whatever
                                        // its spawn count.
                                        view.map_restart();
                                        input.map_restart();
                                        self.fx.clear();
                                        r.clear_marks();
                                        self.audio.stop_all();
                                        self.audio.set_ambient(
                                            &self.fs,
                                            net::info_value_for_key(net.configstring(3), "n"),
                                        );
                                        input.force_stance(play::input::Stance::Stand);
                                    }
                                    // UO's `F <ms>`, `rewindFx` (uo cgame
                                    // 0x3003ac90, trap 0xf0).
                                    if net.protocol().is_uo()
                                        && tokens.first().is_some_and(|t| t == "F")
                                        && let Some(ms) =
                                            tokens.get(1).and_then(|t| t.parse::<i32>().ok())
                                    {
                                        self.fx.rewind(ms as f32);
                                    }
                                    // `v` sets a client cvar in the console's store,
                                    // a demo's too. A UO seat's `v cl_stance 0`
                                    // (game 0x8998d) moves vcod's stance, which
                                    // lives in the input, not a cvar.
                                    if let Some(out) =
                                        join.cvars.on_server_command(tokens, &mut self.shell)
                                    {
                                        cvar_effects(&self.shell, net, &self.config_path, out);
                                        if tokens[1].eq_ignore_ascii_case("cl_stance") {
                                            let value = tokens.get(2).map_or("", String::as_str);
                                            input.force_stance(match value.trim() {
                                                "1" => play::input::Stance::Crouch,
                                                "2" => play::input::Stance::Prone,
                                                _ => play::input::Stance::Stand,
                                            });
                                        }
                                    }
                                    // `a <weapon>` selects (cgame 0x3002e139).
                                    if tokens.first().is_some_and(|t| t == "a") {
                                        input.server_select(
                                            tokens.get(1).map_or("", String::as_str),
                                        );
                                    }
                                    // `g`, the bold game message, beeps.
                                    if tokens.first().is_some_and(|t| t == "g") {
                                        self.audio.play_local(&self.fs, "game_message");
                                    }
                                    if let Some(cmd) = play::demos::auto_command(
                                        tokens,
                                        net.protocol(),
                                        self.shell.cvar("cg_autodemo").unwrap_or("0"),
                                        self.shell.cvar("name").unwrap_or(""),
                                        &play::demos::stamp_now(),
                                    ) {
                                        match cmd.strip_prefix("record ") {
                                            Some(name) => {
                                                start_record(net, &demo_dirs[0], Some(name))
                                            }
                                            None => {
                                                if net.stop_recording().unwrap_or(false) {
                                                    console::log::print("Stopped demo.");
                                                }
                                            }
                                        }
                                    }
                                    use play::join::MenuCommand;
                                    // A demo answers no menu.
                                    let menu = if net.is_demo() {
                                        None
                                    } else {
                                        join.on_server_command(
                                            tokens,
                                            net.configstrings(),
                                            net.server_id(),
                                        )
                                    };
                                    match menu {
                                        Some(MenuCommand::Send(cmd)) => net.send_reliable(&cmd),
                                        Some(MenuCommand::Popup {
                                            name,
                                            index,
                                            no_mouse,
                                        }) => {
                                            let mut out = Vec::new();
                                            self.ui.set_server_cvars(
                                                join.cvars.all(net.configstrings(), &self.shell),
                                            );
                                            if !self.ui.open_script_popup(
                                                &name,
                                                no_mouse,
                                                &self.shell,
                                                &mut out,
                                            ) && let Some(noop) =
                                                join.wait(play::join::Waiting {
                                                    name,
                                                    index,
                                                    no_mouse,
                                                })
                                            {
                                                net.send_reliable(&noop);
                                            }
                                            self.ui_pending.extend(out);
                                        }
                                        Some(MenuCommand::Close) => {
                                            let out = self.ui.close_script_popup(&self.shell);
                                            self.ui_pending.extend(out);
                                        }
                                        None => {}
                                    }
                                    let newest = net.snapshots().newest();
                                    let protocol = net.protocol();
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
                                    join.set_layout(net.protocol().cs);
                                    join.on_gamestate();
                                    // The UI catcher goes down with it, so an
                                    // in-game menu closes (CoDMP.exe 0x40ffb0).
                                    self.ui.close_all();
                                    gamestate_ready = true;
                                    predictor.fall_heights =
                                        pmove::FallHeights::from_systeminfo(net.configstring(1));
                                    system_info_changed(&mut self.shell, net, &self.config_path);
                                }
                                net::NetEvent::ConfigstringChanged(1) => {
                                    predictor.fall_heights =
                                        pmove::FallHeights::from_systeminfo(net.configstring(1));
                                    system_info_changed(&mut self.shell, net, &self.config_path);
                                }
                                _ => {}
                            }
                        }

                        // The cgame offers a refused `t` again each frame
                        // (cgame_mp_x86.dll 0x3002dd10).
                        if let Some(w) = join.waiting() {
                            let mut out = Vec::new();
                            self.ui
                                .set_server_cvars(join.cvars.all(net.configstrings(), &self.shell));
                            if self
                                .ui
                                .open_script_popup(&w.name, w.no_mouse, &self.shell, &mut out)
                            {
                                join.opened_waiting();
                            }
                            self.ui_pending.extend(out);
                        }
                        if self.ui.active() {
                            self.ui
                                .set_server_cvars(join.cvars.all(net.configstrings(), &self.shell));
                        }

                        // One quick-chat line per second, played and shown
                        // like a chat line (retail queues text + alias).
                        if let Some(line) = self.quick_chat.drain(time) {
                            let name = net
                                .snapshots()
                                .newest()
                                .and_then(|s| s.clients.get(&line.client_num))
                                .map(|c| c.name(net.protocol()))
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
                            // `CL_SystemInfoChanged` forced the `fs_game` cvar
                            // (`system_info_changed` above) and sets the pure
                            // list, and the gamestate parse restarts the
                            // search path when either changed
                            // (docs/research/cod11-front-end.md, section 16).
                            // A server with no `fs_game` sends no key, which
                            // leaves the cvar as it was. A demo's systeminfo
                            // sets neither (CoDMP.exe 0x415eb0).
                            let (game, pure) = if net.is_demo() {
                                (self.fs_game.clone(), self.fs_pure.clone())
                            } else {
                                (
                                    net::download::mod_dir(
                                        self.shell.cvar("fs_game").unwrap_or(""),
                                        &self.mod_dir,
                                    ),
                                    net::download::pure_paks(net.configstring(1)),
                                )
                            };
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
                                    &mut self.hud,
                                    &mut self.audio,
                                    &mut self.fx,
                                    &mut self.quick_chat,
                                ) {
                                    Ok(()) => {
                                        r.reopen(&self.fs);
                                        view.reopen();
                                        self.shell.execute("set ui_menuFiles ui_mp/menus.txt");
                                        self.ui = new_ui(
                                            &self.fs,
                                            &self.game_dir,
                                            &self.mod_dir,
                                            self.shell.cvar("cl_language").unwrap_or(""),
                                        );
                                    }
                                    Err(e) => fatal = Some(e),
                                }
                            }
                        }
                        // Only with the map up: the first cmd after a gamestate
                        // is what enters the client into the world
                        // (docs/protocol-1.1.md, "Entering the world").
                        // A demo's clock never slews toward its snapshots
                        // (`CL_AdjustTimeDelta`, CoDMP.exe 0x404bf0), and it
                        // sends no cmds.
                        if let Some(soak) = &mut self.soak
                            && !net.is_demo()
                        {
                            let ps = match &*phase {
                                Phase::Live(_) => net.snapshots().newest().map(|s| &s.ps),
                                _ => None,
                            };
                            soak.act(
                                input,
                                ps,
                                net.protocol(),
                                net.configstring(0),
                                cam.pos,
                                time,
                                self.ui.active(),
                            );
                        }
                        if net.is_demo()
                            && !gamestate_ready
                            && let (Phase::Live(live), Some(s)) =
                                (&mut *phase, net.snapshots().newest())
                        {
                            let nudge = self.shell.cvar_f32("cl_timeNudge") as i32;
                            live.clock.update_demo(
                                local_ms as i32,
                                s.server_time,
                                nudge,
                                demo_clock,
                            );
                        }
                        if matches!(phase, Phase::Live(_))
                            && !gamestate_ready
                            && !net.is_demo()
                            && net.state() == net::NetState::Active
                        {
                            let realtime = local_ms as i32;
                            // Before the first snapshot there is no clock yet;
                            // the entering cmd goes out on the gamestate's.
                            let server_now = match (&mut *phase, net.snapshots().newest()) {
                                (Phase::Live(live), Some(s)) => {
                                    let nudge = self.shell.cvar_f32("cl_timeNudge") as i32;
                                    live.clock.update(
                                        realtime,
                                        s.server_time,
                                        s.message_num,
                                        nudge,
                                    );
                                    live.clock.cmd_time(s.server_time)
                                }
                                _ => net.server_clock_ms(),
                            };
                            let times = cmd_clock.due(server_now);
                            if !times.is_empty() {
                                let weapons = match &*phase {
                                    Phase::Live(live) => live.weapons.as_slice(),
                                    _ => &[],
                                };
                                let held = net
                                    .snapshots()
                                    .newest()
                                    .map_or_else(play::input::Held::default, |s| {
                                        play::input::Held::from_ps(&s.ps, net.protocol(), weapons)
                                    });
                                input.sync(&held);
                                // A select or a respawn puts `cl_run` back to
                                // 1, the cgame's write of the cvar (0x30037f20,
                                // 0x30028a70).
                                if input.take_run_reset() {
                                    self.shell.execute("set cl_run 1");
                                }
                                input.cl_run =
                                    play::input::ClRun(self.shell.cvar_f32("cl_run") as i32);
                                input.talking = play::input::talking(
                                    self.console.open,
                                    self.ui.active(),
                                    self.hud.as_ref().is_some_and(|h| h.chat_field.is_some()),
                                    self.shell.cvar_f32("cl_bypassMouseInput") as i32,
                                );
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
                                    vcod_common::laps::mark(|| "gamestate".into());
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
                                        allow_download(&self.shell),
                                        self.fs.vanilla(),
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
                                            &mut self.hud,
                                            &mut self.audio,
                                            &mut self.fx,
                                            &mut self.quick_chat,
                                        ) {
                                            Ok(()) => {
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
                                        let t_load = Instant::now();
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
                                                vcod_common::laps::span(t_load, || {
                                                    format!("load_map {map}")
                                                });
                                                // `CL_DownloadsComplete` sends it
                                                // after the cgame loads; a pure
                                                // server drops a client without it.
                                                let feed =
                                                    net.gamestate().map_or(0, |g| g.checksum_feed);
                                                net.send_reliable(
                                                    &self.fs.pure_command(
                                                        feed,
                                                        net.protocol().pure_dlls,
                                                    ),
                                                );
                                                view.set_protocol(net.protocol());
                                                prewarm_viewmodels(
                                                    view,
                                                    r,
                                                    &self.fs,
                                                    net.configstrings(),
                                                );
                                                if let Phase::Live(live) = &mut *phase {
                                                    prewarm_level(
                                                        &mut self.fx,
                                                        r,
                                                        &self.fs,
                                                        net,
                                                        live,
                                                    );
                                                }
                                                // `CL_DownloadsComplete` frees the
                                                // old cgame and loads a fresh one.
                                                fresh_cgame(
                                                    view,
                                                    input,
                                                    predictor,
                                                    self.hud.as_mut(),
                                                    &mut self.fx,
                                                );
                                                let t_menus = Instant::now();
                                                if let Err(e) = load_script_menus(
                                                    &mut self.ui,
                                                    &self.fs,
                                                    &self.shell,
                                                    net.configstrings(),
                                                    &net.protocol().cs,
                                                ) {
                                                    fatal = Some(anyhow!(e));
                                                }
                                                vcod_common::laps::span(t_menus, || {
                                                    "script menus".into()
                                                });
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
                                    vcod_common::laps::begin_load("cod1 map change");
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
                                        allow_download(&self.shell),
                                        self.fs.vanilla(),
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
                                        looped_fx,
                                        vehicle_trails,
                                        drawn_pos,
                                        drawn_vehicle_angles,
                                        seat_turn,
                                        gunner_sway,
                                        turret_sway,
                                        seat_frac,
                                    } = &mut **live;
                                    let p = net.protocol();
                                    let client_num = net.gamestate().map_or(-1, |g| g.client_num);
                                    // While following, ps.clientNum is the followed
                                    // client's (docs/research/cod11-events-and-fx.md,
                                    // section 7); the camera sits inside that body, so
                                    // skip it instead of ours.
                                    let ps_client = net
                                        .snapshots()
                                        .newest()
                                        .map_or(-1, |s| s.ps.field_i32(p, "clientNum"));
                                    // A demo draws its recorder as a followed
                                    // player, off the snapshots
                                    // (`CG_InterpolatePlayerState` under
                                    // `cg.demoPlayback`).
                                    let demo = net.is_demo();
                                    let following =
                                        ps_client >= 0 && (ps_client != client_num || demo);
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
                                    // Per-shooter `tag_brass`, for spent cases.
                                    let mut brass: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
                                    // Bodies with no muzzle tag, where their tracers start.
                                    let mut shot_fallbacks: HashMap<u32, Vec3> = HashMap::new();
                                    let mut weapon_flash: HashMap<i32, String> = HashMap::new();
                                    let mut entity_pos: HashMap<u32, Vec3> = HashMap::new();
                                    let mut entity_angles: HashMap<u32, Vec3> = HashMap::new();
                                    let mut vehicle_angles: HashMap<u32, [f32; 3]> = HashMap::new();
                                    let mut vehicle_wheels = Vec::new();
                                    let mut heads: HashMap<u32, Vec3> = HashMap::new();
                                    let mut turret_eye = None;
                                    let mut shadow_marks = Vec::new();
                                    // The drawn `leanf`; the intermission view
                                    // takes none (cgame 0x30033413).
                                    let mut view_lean = 0.0;
                                    // The drawn origin's height, under which
                                    // the eye never goes (cgame 0x30032de5).
                                    let mut feet_z = None;

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
                                        prof.next(frame_prof::Stage::Pose);
                                        // Last frame's drawn bodies, for the gunner's
                                        // trace down.
                                        let bodies = play::predict::solid_bodies(
                                            p,
                                            &b.entities,
                                            client_num as u32,
                                            drawn_pos,
                                        );
                                        scene.swing_speed = self.shell.cvar_f32("bg_swingSpeed");
                                        // The view a ridden UO gun turns with: ours
                                        // off the cmd, a followed one's off the
                                        // snapshots.
                                        scene.view = Some(if snapshot_view {
                                            let (va, vb) = (a.ps.viewangles(p), b.ps.viewangles(p));
                                            std::array::from_fn(|i| {
                                                camera::lerp_angle(va[i], vb[i], f)
                                            })
                                        } else {
                                            let delta = net::DELTA_ANGLE_FIELDS
                                                .map(|name| b.ps.field_i32(p, name));
                                            let (y, pt, r) = own_view(input.cmd_angles(), delta);
                                            [-pt.to_degrees(), y.to_degrees(), r.to_degrees()]
                                        });
                                        // The ridden body draws in a UO seat the view
                                        // leaves (cgame 0x3003392b), and is posed
                                        // undrawn otherwise. It is not in the snapshot,
                                        // so it is built from the playerstate, as
                                        // `cg.predictedPlayerEntity` is.
                                        let seat_body = b.ps.field_i32(p, "pm_type") != 5
                                            && play::seat::of_snapshot(p, b).is_some_and(
                                                |(seat, ..)| {
                                                    play::seat::own_body_drawn(seat, *seat_frac)
                                                },
                                            );
                                        let with_own = |s: &net::snapshot::Snapshot| {
                                            let mut s = s.clone();
                                            let e = play::seat::own_body(p, &s.ps);
                                            s.entities.insert(e.number, e);
                                            s
                                        };
                                        let own_pair = play::seat::adds_view_body(
                                            b.ps.field_i32(p, "pm_flags"),
                                            seat_body,
                                            following,
                                        )
                                        .then(|| (with_own(a), with_own(b)));
                                        let (a, b) =
                                            own_pair.as_ref().map_or((a, b), |(a, b)| (a, b));
                                        let built = entities::build_instances(
                                            scene,
                                            (a, b, f),
                                            render_time,
                                            net.snapshots()
                                                .newest()
                                                .map_or(b.server_time, |s| s.server_time),
                                            if seat_body { -1 } else { skip_num },
                                            net.configstrings(),
                                            &self.fs,
                                            Some(MoveWorld::new(world, &bodies, u32::MAX)),
                                            r,
                                            p,
                                        );
                                        self.build_ms = t0.elapsed().as_secs_f32() * 1000.0;
                                        prof.next(frame_prof::Stage::Net);
                                        instances = built.instances;
                                        muzzles = built.muzzles;
                                        shot_fallbacks = built.shot_fallbacks;
                                        brass = built.brass;
                                        weapon_flash = built.weapon_flash;
                                        entity_pos = built.entity_pos;
                                        entity_angles = built.entity_angles;
                                        vehicle_angles = built.vehicle_angles;
                                        vehicle_wheels = built.vehicle_wheels;
                                        heads = built.heads;
                                        turret_eye = built.turret_eye;
                                        frame_digest::submodels(&built.submodels);
                                        r.set_submodels(&built.submodels);
                                        r.set_entity_lights(built.scene_lights);
                                        // `cg_shadows` 2 is the renderer's
                                        // stencil path; only 1 places blobs.
                                        if self.shell.cvar_f32("cg_shadows") as i32 == 1 {
                                            shadow_marks = built
                                                .shadow_casters
                                                .iter()
                                                .filter_map(|&o| {
                                                    blob_shadow::player_shadow(&*world, oa, o)
                                                })
                                                .collect();
                                        }
                                        // Over 512 u is a teleport, not motion.
                                        let pos = if oa.distance(ob) > 512.0 {
                                            ob
                                        } else {
                                            oa.lerp(ob, f)
                                        };
                                        // viewHeightCurrent: eye offset above the feet origin.
                                        let vh = a.ps.field_f32(p, "viewHeightCurrent") * (1.0 - f)
                                            + b.ps.field_f32(p, "viewHeightCurrent") * f;
                                        cam.pos =
                                            pos + Vec3::Z * play::view::eye_height(pm_type, vh);
                                        feet_z = Some(pos.z);
                                        // `CG_InterpolatePlayerState` lerps `leanf`
                                        // too (cgame 0x3002936a).
                                        if pm_type != 5 {
                                            view_lean = a.ps.field_f32(p, "leanf") * (1.0 - f)
                                                + b.ps.field_f32(p, "leanf") * f;
                                        }
                                        if snapshot_view {
                                            let va_a = a.ps.viewangles(p);
                                            let va_b = b.ps.viewangles(p);
                                            cam.yaw = camera::lerp_angle(va_a[1], va_b[1], f)
                                                .to_radians();
                                            cam.pitch = -camera::lerp_angle(va_a[0], va_b[0], f)
                                                .to_radians();
                                        }
                                    }
                                    // A UO vehicle seat (docs/research/coduo-vehicles.md).
                                    let seat_now = net
                                        .snapshots()
                                        .newest()
                                        .and_then(|s| play::seat::of_snapshot(p, s));
                                    predictor.seat_angles =
                                        seat_now.map_or([0.0; 3], |(_, num, vang)| {
                                            drawn_vehicle_angles.get(&num).copied().unwrap_or(vang)
                                        });
                                    let predicted = if ps_client == client_num && !demo {
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
                                    drawn_vehicle_angles.clone_from(&vehicle_angles);
                                    if let Some(v) = &predicted {
                                        cam.pos = v.origin
                                            + Vec3::Z
                                                * play::view::eye_height(pm_type, v.view_height);
                                        feet_z = Some(v.origin.z);
                                        view_lean = v.pred.ps.lean;
                                    }
                                    if let Some(s) = net.snapshots().newest() {
                                        let spawn =
                                            (s.ps.field_i32(p, "clientNum"), s.ps.arrays.stats[5]);
                                        view.track_spawn(spawn);
                                        if let Some(hud) = &mut self.hud {
                                            hud.track_spawn(spawn);
                                        }
                                        let mut held =
                                            play::input::Held::from_ps(&s.ps, p, weapons);
                                        // `cg.snap`, the older of the drawn pair.
                                        if let Some((snap, _)) = render_time
                                            .and_then(|t| net.snapshots().two_for_time(t))
                                        {
                                            held.cg_snap(&s.ps, &snap.ps, p);
                                        }
                                        input.sync(&held);
                                    }
                                    // A followed player's view weapon, zoom and scope
                                    // come off the snapshot, as retail draws them.
                                    let view_ps = net.snapshots().newest().and_then(|s| {
                                        pmove::predict::on_foot(pm_type).then(|| {
                                            let mut v = match &predicted {
                                                Some(v) => play::view::ViewPs::from_predicted(
                                                    &v.pred,
                                                    s.ps.field_i32(p, "viewmodelIndex"),
                                                ),
                                                None => play::view::ViewPs::from_snapshot(p, &s.ps),
                                            };
                                            v.seat = seat_now.map(|s| s.0);
                                            v
                                        })
                                    });
                                    if let Some(ps) = &view_ps
                                        && let Some(models) =
                                            view.sync_rig(&self.fs, net.configstrings(), ps)
                                    {
                                        r.set_viewmodel(&self.fs, &models);
                                        self.viewmodel = models;
                                    }
                                    // `CG_TransitionPlayerState` feeds a hit ahead of
                                    // the snapshot's events and the view, mounted
                                    // or not (hud doc "Damage direction").
                                    if let (Some(hud), Some(s)) =
                                        (&mut self.hud, net.snapshots().newest())
                                    {
                                        let own = pmove::predict::on_foot(pm_type);
                                        hud.feed_damage(
                                            p,
                                            &s.ps,
                                            predicted.as_ref().filter(|_| own).map(|v| &v.pred),
                                            render_time.unwrap_or(0),
                                        );
                                    }
                                    // Retail runs a snapshot's events in
                                    // `CG_ProcessSnapshots`, ahead of the view
                                    // (cgame 0x30033c14, 0x30033d9b): a quake or a
                                    // landing moves the frame it arrives on.
                                    prof.next(frame_prof::Stage::Events);
                                    let mut evs = Vec::new();
                                    if let Some(newest) = net.snapshots().newest() {
                                        // Our own ring plays off the prediction, and
                                        // the snapshot's copy of it is skipped.
                                        let own = ps_client == client_num && !demo;
                                        evs = play::events::frame_events(
                                            predicted_events,
                                            events,
                                            newest,
                                            p,
                                            own,
                                            own && pmove::predict::predictable(pm_type),
                                            predicted.as_ref().map(|v| &v.pred),
                                            ps_client,
                                        );
                                        if let Some(ps) = &view_ps {
                                            for _ in 0..play::events::view_kicks(&evs, ps_client) {
                                                view.fire(weapons, ps);
                                            }
                                        }
                                        let fall = pmove::FallHeights::from_systeminfo(
                                            net.configstring(1),
                                        );
                                        for ev in &evs {
                                            let Some(now) = render_time else {
                                                break;
                                            };
                                            // At the event body's drawn origin; ours
                                            // is never in `entity_pos`.
                                            let body = entity_pos
                                                .get(&ev.entity_num)
                                                .copied()
                                                .unwrap_or(Vec3::from(ev.pos));
                                            if let Some(q) = play::quake::from_event(
                                                p,
                                                ev,
                                                newest.entities.get(&ev.entity_num),
                                                body,
                                            ) {
                                                view.start_quake(q, now);
                                            }
                                            if ev.client_num == ps_client
                                                && let Some(c) =
                                                    play::view::land_change(ev.event, ev.parm, fall)
                                            {
                                                view.land(c, now);
                                            }
                                        }
                                    }
                                    prof.next(frame_prof::Stage::Vm);
                                    view.step_recoil(weapons, view_ps.as_ref(), local_ms);
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
                                    // `CG_OffsetFirstPersonView` (cgame 0x30032b58)
                                    // first adds the damage kick and the sight's
                                    // bob; the intermission arm skips it.
                                    let mut damage_kick = [0.0; 3];
                                    if pm_type != 5
                                        && let (Some(hud), Some(s)) =
                                            (&mut self.hud, net.snapshots().newest())
                                    {
                                        let own = pmove::predict::on_foot(pm_type);
                                        let [kp, ky, kr] = hud.view_kick(
                                            p,
                                            &s.ps,
                                            predicted.as_ref().filter(|_| own).map(|v| &v.pred),
                                            render_time.unwrap_or(0),
                                            weapons,
                                        );
                                        damage_kick = [kp, ky, kr];
                                        cam.pitch -= kp.to_radians();
                                        cam.yaw += ky.to_radians();
                                        view_roll += kr.to_radians();
                                        // Then the walk bob, whose amplitudes a 1.5
                                        // server sets through the 140/204 mirror
                                        // (cod15-movement.md 6).
                                        hud.bob = if pmove::MoveRules::for_protocol(p).v15() {
                                            pmove::BobCvars::from_cvars(|c| {
                                                join.cvars.get(c, net.configstrings(), &self.shell)
                                            })
                                        } else {
                                            pmove::BobCvars::default()
                                        };
                                        let kicked = [
                                            -cam.pitch.to_degrees(),
                                            cam.yaw.to_degrees(),
                                            view_roll.to_degrees(),
                                        ];
                                        cam.pos += hud.eye_bob(
                                            p,
                                            &s.ps,
                                            predicted.as_ref().filter(|_| own).map(|v| &v.pred),
                                            render_time.unwrap_or(0),
                                            kicked,
                                        );
                                    }
                                    // Then the landing's dip (0x30032ca1), and it
                                    // ends on `AddLeanToPosition` (cgame
                                    // 0x30032dda): a lean moves the eye and rolls
                                    // nothing.
                                    if pm_type != 5
                                        && let Some(now) = render_time
                                    {
                                        cam.pos.z += view.land_dip(now);
                                    }
                                    cam.pos = pmove::aim::add_lean(
                                        cam.pos,
                                        cam.yaw,
                                        view_lean,
                                        vcod_common::patch::Arith::Msvc,
                                    );
                                    if pm_type != 5
                                        && let Some(z) = feet_z
                                    {
                                        cam.pos.z = cam.pos.z.max(z + 8.0);
                                    }
                                    // In a vehicle seat `CG_VehicleView` (uo cgame
                                    // 0x30040810) replaces the first-person offsets.
                                    if let Some((seat, num, vang)) = seat_now {
                                        let delta = match &predicted {
                                            Some(v) => v.delta_angles,
                                            None => net.snapshots().newest().map_or([0; 3], |s| {
                                                net::DELTA_ANGLE_FIELDS
                                                    .map(|name| s.ps.field_i32(p, name))
                                            }),
                                        };
                                        let newest_ps =
                                            &net.snapshots().newest().expect("seated").ps;
                                        let frac = predicted.as_ref().map_or_else(
                                            || newest_ps.field_f32(p, "fWeaponPosFrac"),
                                            |v| v.pred.ps.weapon_pos_frac,
                                        );
                                        *seat_frac = frac;
                                        // A followed rider's view is the snapshot's.
                                        let wire_view = |input: &play::input::PlayInput| {
                                            let (y, pt) = if snapshot_view {
                                                (cam.yaw, cam.pitch)
                                            } else {
                                                let (y, pt, _) = own_view_at(
                                                    input.cmd_angles(),
                                                    delta,
                                                    SEAT_PITCH_CLAMP,
                                                );
                                                (y, pt)
                                            };
                                            [-pt.to_degrees(), y.to_degrees(), 0.0]
                                        };
                                        let mut tag =
                                            |name: &str| scene.tag_pose(&self.fs, num, name);
                                        // 0x30040580: a four-wheel turns its riders by
                                        // the vehicle's `lerpAngles` (0x3004060f).
                                        let drawn_angles =
                                            vehicle_angles.get(&num).copied().unwrap_or(vang);
                                        if !snapshot_view
                                            && seat.class == play::seat::CLASS_FOUR_WHEEL
                                            && seat.pos != 2
                                            && !seat.own_weapon()
                                            && newest_ps.field_i32(p, "weapon") != 0
                                        {
                                            if let Some(v) = seat_turn.step(
                                                wire_view(input),
                                                drawn_angles,
                                                frac != 0.0,
                                                newest_ps.field_i32(p, "eFlags")
                                                    & net::flags::EF_HULL_VIEW_V22
                                                    != 0,
                                            ) {
                                                input.set_view(v, delta);
                                            }
                                        } else {
                                            seat_turn.reset();
                                        }
                                        let origin =
                                            entity_pos.get(&num).copied().unwrap_or(cam.pos);
                                        // The gunner's sway off a `thompson_MP` copy of
                                        // the predicted playerstate, and its shake off
                                        // `cg.snap`'s (coduo-vehicles.md 5).
                                        let mut gunner = play::seat::GunnerEye::default();
                                        if seat.pos == 2
                                            && let Some(now) = render_time
                                        {
                                            gunner.time_ms = now;
                                            let thompson = net
                                                .configstring(7)
                                                .split(' ')
                                                .position(|w| w.eq_ignore_ascii_case("thompson_mp"))
                                                .and_then(|i| weapons.get(i + 1)?.as_ref());
                                            if let (Some(v), Some(def)) = (&predicted, thompson) {
                                                gunner.sway = gunner_sway.step(
                                                    &v.pred.ps,
                                                    def,
                                                    wire_view(input),
                                                    now,
                                                );
                                            }
                                            let snaps = net.snapshots();
                                            if let Some(s) = snaps
                                                .two_for_time(now)
                                                .map(|(a, _)| a)
                                                .or(snaps.newest())
                                            {
                                                let movetype = s
                                                    .entities
                                                    .get(&num)
                                                    .map_or(0, |e| e.field_i32(p, "animMovetype"));
                                                if play::seat::gunner_shakes(
                                                    s.ps.field_i32(p, "eFlags"),
                                                    movetype,
                                                ) {
                                                    gunner.shake =
                                                        Some(play::seat::shake_phase(now));
                                                }
                                            }
                                        }
                                        if let Some(sc) = play::seat::camera(
                                            seat,
                                            wire_view(input),
                                            frac,
                                            origin,
                                            gunner,
                                            &mut tag,
                                        ) {
                                            if !snapshot_view {
                                                if let Some(v) = sc.snap {
                                                    input.set_view(v, delta);
                                                }
                                                (cam.yaw, cam.pitch, _) = own_view_at(
                                                    input.cmd_angles(),
                                                    delta,
                                                    SEAT_PITCH_CLAMP,
                                                );
                                            }
                                            view_roll = 0.0;
                                            if let Some([pitch, roll]) = sc.gun_pitch_roll {
                                                cam.pitch = -pitch.to_radians();
                                                view_roll = roll.to_radians();
                                            }
                                            cam.yaw += sc.yaw_shake.to_radians();
                                            let half = Vec3::splat(8.0);
                                            cam.pos = play::seat::clip(&sc, &mut |a, b, mask| {
                                                let t = world.move_trace(a, b, -half, half, mask);
                                                (t.fraction, t.endpos, t.startsolid)
                                            });
                                        }
                                    } else {
                                        seat_turn.reset();
                                    }
                                    // On a mounted gun the view rides the gun's
                                    // `tag_player` and barrel, not the cmd's angles
                                    // (docs/research/cod11-turrets.md section 14).
                                    // 1.5's and UO's (1.5 cgame 0x300348c0, uo
                                    // 0x30041550) hold the view inside the gun's
                                    // arcs; UO's sways the eye.
                                    if let Some(eye) = turret_eye.map(|e| e.kicked(damage_kick)) {
                                        cam.pos = eye.pos;
                                        (cam.yaw, cam.pitch) = eye.view();
                                        view_roll = eye.angles[2].to_radians();
                                        if !snapshot_view && let Some([pt, y]) = eye.snap {
                                            let delta = match &predicted {
                                                Some(v) => v.delta_angles,
                                                None => {
                                                    net.snapshots().newest().map_or([0; 3], |s| {
                                                        net::DELTA_ANGLE_FIELDS
                                                            .map(|name| s.ps.field_i32(p, name))
                                                    })
                                                }
                                            };
                                            // The cmd adds the kick back on.
                                            input.set_view(
                                                [pt - input.kick[0], y - input.kick[1]],
                                                delta,
                                            );
                                        }
                                        if let Some(axis) = eye.axis {
                                            let thompson = net
                                                .configstring(7)
                                                .split(' ')
                                                .position(|w| w.eq_ignore_ascii_case("thompson_mp"))
                                                .and_then(|i| weapons.get(i + 1)?.as_ref());
                                            if let (Some(v), Some(def), Some(now)) =
                                                (&predicted, thompson, render_time)
                                            {
                                                let o = turret_sway.step(
                                                    &v.pred.ps,
                                                    def,
                                                    scene.view.unwrap_or(eye.angles),
                                                    now,
                                                );
                                                cam.pos += turret::sway_offset(axis, o);
                                            }
                                        }
                                    }
                                    // The quake shakes whatever view that left
                                    // (cgame 0x30033da9, after `CG_CalcViewValues`).
                                    if let Some(now) = render_time {
                                        let [sp, sy, sr] = view.shake(now, cam.pos);
                                        cam.pitch -= sp.to_radians();
                                        cam.yaw += sy.to_radians();
                                        view_roll += sr.to_radians();
                                    }
                                    // The shellshock off `cg.snap`'s playerstate
                                    // (cgame 0x3002f7d0, 0x3002e900); its kick
                                    // turns the view after the quake (0x30033dbd).
                                    let shock = match render_time {
                                        Some(now) => {
                                            if let Some(args) = view.shock_command.take() {
                                                let args: Vec<&str> =
                                                    args.iter().map(String::as_str).collect();
                                                if let Err(e) =
                                                    view.shock.command(&self.fs, &args, now)
                                                {
                                                    console::log::print(&e);
                                                }
                                            }
                                            let snaps = net.snapshots();
                                            let fields = snaps
                                                .two_for_time(now)
                                                .map(|(a, _)| a)
                                                .or(snaps.newest())
                                                .map_or((0, 0, 0), |s| {
                                                    let f = |n: &str| s.ps.field_i32(p, n);
                                                    (
                                                        f("shellshockIndex"),
                                                        f("shellshockTime"),
                                                        f("shellshockDuration"),
                                                    )
                                                });
                                            let cs = net.configstrings();
                                            view.shock.frame(
                                                &self.fs,
                                                |i| {
                                                    p.cs.slot(net::protocol::CsRange::ShellShock, i)
                                                        .and_then(|k| cs.get(k))
                                                        .map(String::as_str)
                                                },
                                                fields,
                                                now,
                                            )
                                        }
                                        None => play::shock::ShockFrame::rest(),
                                    };
                                    if shock.kick != [0.0; 2] {
                                        let pitch;
                                        [pitch, cam.yaw, view_roll] = play::shock::kicked_angles(
                                            [
                                                -cam.pitch.to_degrees(),
                                                cam.yaw.to_degrees(),
                                                view_roll.to_degrees(),
                                            ],
                                            shock.kick,
                                        )
                                        .map(f32::to_radians);
                                        cam.pitch = -pitch;
                                    }
                                    input.shellshock = shock.slowed;
                                    if let Some(hud) = &mut self.hud {
                                        hud.drawn_view = Some([
                                            -cam.pitch.to_degrees(),
                                            cam.yaw.to_degrees(),
                                            view_roll.to_degrees(),
                                        ]);
                                    }
                                    input.set_mouse_caps(
                                        dt * 1000.0,
                                        [shock.mouse.max_pitch, shock.mouse.max_yaw],
                                    );
                                    self.audio.set_shellshock(&self.fs, &shock.sound);
                                    r.set_screen_blend(shock.blend_ms);

                                    // `0x300371f0` poses the view weapon off the
                                    // finished view, the gun's aim stepped once for
                                    // it and the scope (combat doc 17.4).
                                    let local_player =
                                        ps_client == client_num && pmove::predict::on_foot(pm_type);
                                    if let Some(hud) = &mut self.hud {
                                        hud.set_shock_left(shock.left_ms);
                                    }
                                    let gun = match (&mut self.hud, net.snapshots().newest()) {
                                        (Some(hud), Some(s)) => hud.step_gun(
                                            p,
                                            &s.ps,
                                            predicted
                                                .as_ref()
                                                .filter(|_| local_player)
                                                .map(|v| &v.pred),
                                            render_time.unwrap_or(0),
                                            weapons,
                                            view.gun_kick(),
                                        ),
                                        _ => None,
                                    };
                                    let world_up = {
                                        let [f, l, u] = pmove::aim::angles_to_axis_msvc([
                                            -cam.pitch.to_degrees(),
                                            cam.yaw.to_degrees(),
                                            view_roll.to_degrees(),
                                        ]);
                                        Vec3::new(f[2], l[2], u[2])
                                    };
                                    let cg_fov = cg_fov(&self.shell);
                                    view.draw_gun = play::view::DrawGun::from_cvar(
                                        self.shell.cvar_f32("cg_drawGun") as i32,
                                    );
                                    let (mut vm_draw, fov) = view.frame(
                                        cg_fov,
                                        weapons,
                                        view_ps.as_ref(),
                                        local_ms,
                                        gun,
                                        world_up,
                                    );
                                    // `0x30033e97`: the shellshock's sensitivity
                                    // scale rides the zoom's, on the 4:3 fovs.
                                    self.look_zoom = (
                                        fov / cg_fov * shock.mouse.sensitivity,
                                        view_ps.as_ref().is_some_and(|v| v.mounted),
                                    );
                                    let fov = camera::view_fov_x(fov, aspect, widescreen);
                                    if let Some(d) = &mut vm_draw {
                                        d.fov_x = fov;
                                    }
                                    vm = vm_draw;

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
                                    // The ridden body's cases leave the viewmodel's
                                    // `tag_brass` (`CG_EjectBrass`, cgame 0x30038a30).
                                    if let Some(tag) =
                                        view.brass(cam.pos, (cam_forward, cam_right, cam_up))
                                    {
                                        brass.insert(u32::MAX, tag);
                                        if let Ok(num) = u32::try_from(skip_num) {
                                            brass.insert(num, tag);
                                        }
                                    }

                                    // Step before this frame's events spawn, or the
                                    // [now-dt, now] integration would move particles born
                                    // at `now` (a tracer would start ahead of its muzzle).
                                    let fx_t0 = Instant::now();
                                    prof.next(frame_prof::Stage::FxStep);
                                    self.fx.set_view(cam.pos);
                                    self.fx.set_bolts(&muzzles);
                                    // The `playFxOnTag` tags a live effect rides,
                                    // posed as drawn (CoDMP.exe 0x49e8d0, a bolt
                                    // posed once a frame).
                                    let tag_poses = self
                                        .fx
                                        .tag_bolts()
                                        .into_iter()
                                        .filter_map(|b| {
                                            let tag = fx::registry::bolt_tag(
                                                net.configstrings(),
                                                &p.cs,
                                                b.slot,
                                            )?;
                                            Some((b, scene.tag_pose(&self.fs, b.entity, tag)?))
                                        })
                                        .collect();
                                    self.fx.set_tag_bolts(&tag_poses);
                                    self.fx.step(dt, time, Some(&*world));
                                    prof.next(frame_prof::Stage::Fx);
                                    let chained = self.fx.spawn_chained(&self.fs, time);
                                    // Placed by the wall-clock step (the brass).
                                    frame_digest::clocked(|| self.audio.play_fx(&self.fs, chained));
                                    // UO's wheel dust (uo cgame 0x30020540): the
                                    // world under each wheel, mask 0x211, then
                                    // every other vehicle's collmap where drawn.
                                    fx::vehicle_trails::place_collmaps(world, &vehicle_wheels);
                                    let trails = vehicle_trails.frame(
                                        self.shell.cvar_f32("cg_vehicletrails") as i32 != 0,
                                        &vehicle_wheels,
                                        |n, a, b| {
                                            let t = fx::vehicle_trails::wheel_trace(world, n, a, b);
                                            (t.fraction < 1.0).then_some(t.surface_flags)
                                        },
                                    );
                                    for play in trails {
                                        let at = fx::sim::SpawnAt::Directed {
                                            pos: play.pos,
                                            dir: play.dir,
                                        };
                                        let sounds = self.fx.spawn(&self.fs, play.path, at, time);
                                        self.audio.play_fx(&self.fs, sounds);
                                    }
                                    // `ET_FX` off the drawn snapshot (`CG_FX`, cgame
                                    // 0x3001cf20), on the drawn server time.
                                    if let Some(t) = render_time
                                        && let Some((a, _)) = net.snapshots().two_for_time(t)
                                    {
                                        for play in
                                            looped_fx.frame(a, p, t, cam.pos, net.configstrings())
                                        {
                                            let sounds = self.fx.spawn_marked(
                                                &self.fs, &play.path, play.at, None, time, true,
                                            );
                                            self.audio.play_fx(&self.fs, sounds);
                                        }
                                    }

                                    prof.next(frame_prof::Stage::Events);
                                    let (screen_w, screen_h) = r.screen_size();

                                    let no_clients = BTreeMap::new();
                                    let no_entities = BTreeMap::new();
                                    let newest = net.snapshots().newest();
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
                                            p.is_uo(),
                                        )?;
                                        let w = s.entities.get(&num)?.field_i32(p, "weapon");
                                        usize::try_from(w).ok()
                                    });
                                    // UO's barrel icon turns with the ridden
                                    // vehicle's `tag_turret` (uo cgame 0x30031e20).
                                    let vehicle_turret_yaw = newest
                                        .filter(|s| {
                                            s.ps.field_i32(p, "eFlags")
                                                & net::flags::EF_IN_VEHICLE_V22
                                                != 0
                                        })
                                        .and_then(|s| {
                                            let num = u32::try_from(
                                                s.ps.field_i32(p, "viewlocked_entNum"),
                                            )
                                            .ok()?;
                                            let f = scene
                                                .tag_pose(&self.fs, num, "tag_turret")?
                                                .axis[0];
                                            Some(f.y.atan2(f.x).to_degrees())
                                        });
                                    let client_cvar = |c: &str| {
                                        join.cvars.get(c, net.configstrings(), &self.shell)
                                    };
                                    let server_address =
                                        net.transport().peer().map(|a| a.to_string());
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
                                        vehicle_turret_yaw,
                                        cvar: &client_cvar,
                                        bound_key: &|cmd| {
                                            bound_key(&self.shell, &self.localized, p, cmd)
                                        },
                                        draw: hud::DrawToggles {
                                            crosshair: self.shell.cvar_f32("cg_drawCrosshair")
                                                as i32
                                                != 0,
                                            status: self.shell.cvar_f32("cg_drawStatus") as i32
                                                != 0,
                                        },
                                        weapon_select: input.selected(),
                                        own_client: net
                                            .gamestate()
                                            .and_then(|g| u32::try_from(g.client_num).ok()),
                                        server_address: server_address.as_deref(),
                                    };

                                    // UO's tracers: the length gate, and `sv_night`
                                    // off the systeminfo (fx::sim::TracerRules).
                                    let uo = p.is_uo();
                                    self.fx.set_tracer_rules(fx::sim::TracerRules {
                                        min_length: uo,
                                        night: uo
                                            && net::info_value_for_key(
                                                net.configstring(1),
                                                "sv_night",
                                            )
                                            .and_then(|v| v.trim().parse::<f32>().ok())
                                            .is_some_and(|v| v as i32 != 0),
                                    });
                                    // Events use the newest snapshot, not the interpolation
                                    // pair; they must not wait out the interp delay.
                                    if let Some(newest) = newest {
                                        let ctx = fx::registry::ResolveCtx {
                                            brass: &brass,
                                            weapons: weapons.as_slice(),
                                            eject_brass,
                                            configstrings: net.configstrings(),
                                            cs: &p.cs,
                                            blood,
                                            muzzles: &muzzles,
                                            shot_fallbacks: &shot_fallbacks,
                                            weapon_flash: &weapon_flash,
                                            view_flash: vm.is_some().then_some(weapons.as_slice()),
                                            view_body: fx::registry::view_body(
                                                newest.ps.field_i32(p, "pm_flags"),
                                                ps_client,
                                            ),
                                        };
                                        // A client-traced round's hit joins the
                                        // queue after its fire event, with the
                                        // tracer's tag as its shooter's muzzle.
                                        let mut evs: std::collections::VecDeque<_> =
                                            evs.into_iter().map(|e| (e, None)).collect();
                                        let bodies = play::predict::solid_bodies(
                                            p,
                                            &newest.entities,
                                            client_num as u32,
                                            &entity_pos,
                                        );
                                        // Brush models and vehicles' collmaps where
                                        // the newest snapshot has them, as the
                                        // cgame's entity clip meets them.
                                        let movers = pmove::movers::SnapshotMovers::from_entities(
                                            p,
                                            &newest.entities,
                                        );
                                        movers.place(world, newest.server_time);
                                        let round_trace = |s: Vec3, e: Vec3, skip: u32| {
                                            let w = MoveWorld::new(world, &bodies, skip);
                                            let t = w.box_trace(
                                                s,
                                                e,
                                                Vec3::ZERO,
                                                Vec3::ZERO,
                                                fx::client_bullet::MASK_CLIENT_SHOT,
                                            );
                                            fx::client_bullet::RoundTrace {
                                                fraction: t.fraction,
                                                endpos: t.endpos,
                                                normal: t.normal,
                                                surface_flags: t.surface_flags,
                                                entity: w.entity_num(&t),
                                            }
                                        };
                                        // The second trace stops only on a
                                        // vehicle; the coaxial's passes its own.
                                        let vehicle_trace =
                                            |s: Vec3, e: Vec3, skip: Option<u32>| {
                                                let own = skip.map(collision::Pusher::Copy);
                                                let t = world.point_trace_except(
                                                    s,
                                                    e,
                                                    fx::client_bullet::MASK_VEHICLE_SHOT,
                                                    false,
                                                    own,
                                                );
                                                let entity = world.entity_num(&t);
                                                newest
                                                    .entities
                                                    .get(&entity)
                                                    .is_some_and(|v| {
                                                        v.field_i32(p, "eType")
                                                            == net::flags::ET_VEHICLE_V22
                                                    })
                                                    .then_some(fx::client_bullet::RoundTrace {
                                                        fraction: t.fraction,
                                                        endpos: t.endpos,
                                                        normal: t.normal,
                                                        surface_flags: t.surface_flags,
                                                        entity,
                                                    })
                                            };
                                        let view = fx::client_bullet::View {
                                            origin: cam.pos,
                                            angles: [0, 1, 2].map(|i| {
                                                newest.ps.field_f32(p, &format!("viewangles[{i}]"))
                                            }),
                                        };
                                        while let Some((ev, round_muzzle)) = evs.pop_front() {
                                            let ctx = match &round_muzzle {
                                                Some(m) => {
                                                    fx::registry::ResolveCtx { muzzles: m, ..ctx }
                                                }
                                                None => fx::registry::ResolveCtx { ..ctx },
                                            };
                                            let muzzles = ctx.muzzles;
                                            if let Some(round) =
                                                fx::registry::client_round(&ev, &ctx)
                                                && let Some(pose) = scene.tag_pose(
                                                    &self.fs,
                                                    round.entity,
                                                    round.tag,
                                                )
                                            {
                                                let def = usize::try_from(round.weapon)
                                                    .ok()
                                                    .and_then(|w| weapons.get(w)?.as_ref());
                                                match self.client_bullets.fire(
                                                    time.to_bits(),
                                                    &round,
                                                    def,
                                                    &pose,
                                                    Some(view),
                                                    &round_trace,
                                                    &vehicle_trace,
                                                ) {
                                                    Some(fx::client_bullet::Outcome::Hit {
                                                        pos,
                                                        normal,
                                                        reflect,
                                                        surf_type,
                                                    }) => {
                                                        // No tag, no tracer (0x30048b60).
                                                        let tracer: HashMap<u32, (Vec3, Vec3)> =
                                                            scene
                                                                .tag_pose(
                                                                    &self.fs,
                                                                    round.entity,
                                                                    round.tracer_tag,
                                                                )
                                                                .map(|t| {
                                                                    (
                                                                        round.entity,
                                                                        (t.origin, t.axis[0]),
                                                                    )
                                                                })
                                                                .into_iter()
                                                                .collect();
                                                        evs.push_front((
                                                            fx::client_bullet::hit_event(
                                                                &round, pos, normal, reflect,
                                                                surf_type,
                                                            ),
                                                            Some(tracer),
                                                        ));
                                                    }
                                                    Some(fx::client_bullet::Outcome::Miss {
                                                        start,
                                                        end,
                                                    }) => {
                                                        let heavy = def
                                                            .is_some_and(fx::registry::heavy_round);
                                                        self.fx.spawn_tracer(
                                                            (start, end),
                                                            false,
                                                            (heavy, true),
                                                            time,
                                                            dt,
                                                        );
                                                    }
                                                    None => {}
                                                }
                                            }
                                            self.ev_seen += 1;
                                            if let Some(hud) = &mut self.hud {
                                                hud.on_game_event(&ev, &hud_frame);
                                            }
                                            if let Some(s) = play::events::forced_stance(
                                                &ev,
                                                client_num,
                                                self.shell.cvar_f32("cl_stanceTemp") as i32 != 0,
                                            ) {
                                                input.force_stance(s);
                                            }
                                            if let Some(w) = play::events::picked_up_weapon(
                                                &ev,
                                                ctx.view_body,
                                                net.protocol(),
                                            ) && input.selected() == 0
                                            {
                                                input.picked_up(w);
                                                if let Some(hud) = &mut self.hud {
                                                    hud.weapon_selected();
                                                }
                                            }
                                            if play::events::runs_out_of_ammo(&ev, ctx.view_body) {
                                                // Off the predicted playerstate, as
                                                // 0x300381b0 reads it.
                                                if let Some(v) =
                                                    predicted.as_ref().filter(|_| local_player)
                                                {
                                                    input.predict_held(&v.pred.ps, weapons);
                                                }
                                                input.out_of_ammo();
                                            }
                                            self.audio.on_game_event(
                                                &self.fs,
                                                &ev,
                                                net.configstrings(),
                                                muzzles,
                                                ctx.shot_fallbacks,
                                            );
                                            for r in fx::registry::resolve(&ev, &ctx) {
                                                match r {
                                                    fx::registry::Resolved::Spawn {
                                                        path,
                                                        at,
                                                        nomarks,
                                                        bolt,
                                                    } => {
                                                        let sounds = self.fx.spawn_marked(
                                                            &self.fs, &path, at, bolt, time,
                                                            !nomarks,
                                                        );
                                                        self.audio.play_fx(&self.fs, sounds);
                                                    }
                                                    fx::registry::Resolved::OnTag {
                                                        path,
                                                        entity,
                                                        tag,
                                                        slot,
                                                    } => {
                                                        // The playerState ring's
                                                        // events are the view
                                                        // body's, posed undrawn.
                                                        let entity = match entity {
                                                            u32::MAX => ps_client as u32,
                                                            n => n,
                                                        };
                                                        if let Some(pose) =
                                                            scene.tag_pose(&self.fs, entity, &tag)
                                                        {
                                                            let bolt =
                                                                fx::sim::TagBolt { entity, slot };
                                                            let sounds = self.fx.spawn_on_tag(
                                                                &self.fs, &path, bolt, pose, time,
                                                            );
                                                            self.audio.play_fx(&self.fs, sounds);
                                                        }
                                                    }
                                                    fx::registry::Resolved::Tracer {
                                                        muzzle,
                                                        impact,
                                                        flesh,
                                                        heavy,
                                                        roll,
                                                    } => {
                                                        self.fx.spawn_tracer(
                                                            (muzzle, impact),
                                                            flesh,
                                                            (heavy, roll),
                                                            time,
                                                            dt,
                                                        );
                                                    }
                                                    fx::registry::Resolved::Known => {}
                                                    fx::registry::Resolved::Unknown => {
                                                        self.ev_unknown += 1;
                                                        let foreign =
                                                            ev.event & net::event_ids::EV_FOREIGN;
                                                        if foreign != 0
                                                            && self
                                                                .foreign_events
                                                                .insert(ev.event & !foreign)
                                                        {
                                                            log::info!(
                                                                "protocol {} event {} not played",
                                                                p.transport.version,
                                                                ev.event & !foreign
                                                            );
                                                        }
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

                                    // A UO effect's `CameraShake` feeds the same
                                    // shake core as an earthquake (uo cgame
                                    // vmMain 16, 0x3002b0e0).
                                    for sh in self.fx.take_shakes() {
                                        if let Some(now) = render_time {
                                            view.start_quake(
                                                play::quake::Quake {
                                                    start_ms: 0,
                                                    scale: sh.scale,
                                                    duration_ms: sh.duration_ms,
                                                    radius: sh.radius,
                                                    origin: sh.origin,
                                                },
                                                now,
                                            );
                                        }
                                    }

                                    prof.next(frame_prof::Stage::Sound);
                                    // `es.loopSound` (docs/research/cod11-sound-system.md,
                                    // section 9) and each missile's `projectileSound`
                                    // (section 8), reconciled once per snapshot. An entity
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
                                        self.audio.set_projectile_sounds(
                                            &self.fs,
                                            net.configstrings(),
                                            &audio::missile_sounds(p, newest),
                                        );
                                    }

                                    // UO's sound blend entities, every frame
                                    // off the drawn pair (uo cgame 0x30021860).
                                    if let Some(t) = render_time
                                        && let Some((a, b)) = net.snapshots().two_for_time(t)
                                    {
                                        let f = ((t - a.server_time) as f32
                                            / (b.server_time - a.server_time).max(1) as f32)
                                            .clamp(0.0, 1.0);
                                        // A blend restarts when its voice ran out,
                                        // on the wall clock.
                                        frame_digest::clocked(|| {
                                            self.audio.step_sound_blends(
                                                &self.fs,
                                                p,
                                                (a, b, f),
                                                (net.configstrings(), weapons),
                                                &entity_pos,
                                            )
                                        });
                                    }

                                    // After the drain, so new voices get this frame's positions.
                                    self.audio.step_fire_loops(
                                        &self.fs,
                                        dt * 1000.0,
                                        &entity_pos,
                                        &muzzles,
                                    );
                                    self.audio.step(&entity_pos, &entity_angles, Some(&*world));

                                    prof.next(frame_prof::Stage::Pose);
                                    // The brass below steps on wall-clock time.
                                    frame_digest::models(&instances);
                                    // After the events, so a case ejected this frame
                                    // draws this frame.
                                    for m in self.fx.models(time) {
                                        if let Some(model) = scene.fx_model(r, &self.fs, &m.model) {
                                            let [f, l, u] = m.axis;
                                            instances.push(DynamicModelInstance {
                                                model,
                                                transform: glam::Mat4::from_cols(
                                                    f.extend(0.0),
                                                    l.extend(0.0),
                                                    u.extend(0.0),
                                                    m.origin.extend(1.0),
                                                ),
                                                bones: None,
                                                light_origin: m.origin,
                                                shader_time: None,
                                            });
                                        }
                                    }
                                    frame_prof::count(frame_prof::Count::Models, instances.len());
                                    frame_prof::count(
                                        frame_prof::Count::Particles,
                                        self.fx.counts().0,
                                    );
                                    r.set_dynamic_models(&instances);
                                    prof.next(frame_prof::Stage::FxDraw);

                                    // `cg.snap`'s own player and solid bodies, the
                                    // older of the drawn pair (coduo-diff.md 19):
                                    // the chunks clip them and the report tests
                                    // the player.
                                    let (victim, bodies) = if p.is_uo() {
                                        let snap = render_time
                                            .and_then(|t| net.snapshots().two_for_time(t))
                                            .map(|(a, _)| a)
                                            .or(newest);
                                        let victim = snap.and_then(|s| {
                                            let v = |n: &str| {
                                                Vec3::from_array([0, 1, 2].map(|i| {
                                                    s.ps.field_f32(p, &format!("{n}[{i}]"))
                                                }))
                                            };
                                            Some(fx::flame::Victim {
                                                client: u32::try_from(
                                                    s.ps.field_i32(p, "clientNum"),
                                                )
                                                .ok()?,
                                                origin: v("origin"),
                                                mins: v("mins"),
                                                maxs: v("maxs"),
                                            })
                                        });
                                        let bodies = snap
                                            .zip(victim)
                                            .map(|(s, v)| {
                                                play::predict::solid_bodies(
                                                    p,
                                                    &s.entities,
                                                    v.client,
                                                    &entity_pos,
                                                )
                                            })
                                            .unwrap_or_default();
                                        (victim, bodies)
                                    } else {
                                        (None, Vec::new())
                                    };
                                    // UO's flamethrower streams off each firing
                                    // body's `tag_flash` (fx::flame).
                                    let nozzles = newest
                                        .map(|s| {
                                            fx::flame::nozzles(p, s, ps_client, weapons, &muzzles)
                                        })
                                        .unwrap_or_default();
                                    if !nozzles.is_empty() || !self.flame.is_empty() {
                                        let sounds = self.flame.step(
                                            time * 1000.0,
                                            dt * 1000.0,
                                            &nozzles,
                                            cam.pos,
                                            Some(&fx::flame::ChunkWorld {
                                                world,
                                                bodies: &bodies,
                                                local: victim,
                                                max_clients: fx::flame::max_clients(
                                                    net.configstring(0),
                                                ),
                                            }),
                                        );
                                        for s in sounds {
                                            use fx::flame::FlameSound;
                                            let cue = |alias: &str, owner: u32, pos: Vec3| {
                                                audio::cues::Cue {
                                                    alias: alias.to_string(),
                                                    source: audio::cues::Source::Entity {
                                                        num: owner,
                                                        pos,
                                                    },
                                                    delay_s: 0.0,
                                                }
                                            };
                                            match s {
                                                FlameSound::Start { owner, pos } => {
                                                    self.audio.play(
                                                        &self.fs,
                                                        cue("flamethrower_start", owner, pos),
                                                    );
                                                }
                                                FlameSound::Fire { owner, pos } => self.audio.hold(
                                                    &self.fs,
                                                    owner,
                                                    "flamethrower_fire",
                                                    pos,
                                                ),
                                                FlameSound::Stop { owner, pos } => {
                                                    self.audio.release(owner);
                                                    self.audio.play(
                                                        &self.fs,
                                                        cue("flamethrower_cooldown", owner, pos),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    // UO's flame report on `cg.snap`'s player.
                                    let mut fdc = None;
                                    if p.is_uo() {
                                        let flame_trace = |s: Vec3, e: Vec3| {
                                            let w = MoveWorld::new(world, &bodies, u32::MAX);
                                            let t = w.box_trace(
                                                s,
                                                e,
                                                Vec3::ZERO,
                                                Vec3::ZERO,
                                                fx::client_bullet::MASK_CLIENT_SHOT,
                                            );
                                            (t.fraction, w.entity_num(&t))
                                        };
                                        let report = self.flame.report(
                                            time * 1000.0,
                                            victim,
                                            net.flood_window_open(),
                                            Some(&flame_trace),
                                        );
                                        input.in_flame = report.in_flame;
                                        fdc = report.fdc;
                                    }
                                    let mut fx_quads =
                                        self.fx.build_quads(cam.pos, cam_right, cam_up, time);
                                    fx_quads.extend(self.flame.quads(
                                        time * 1000.0,
                                        cam_right,
                                        cam_up,
                                    ));
                                    if let Some(newest) = newest {
                                        fx_quads.extend(head_icon::quads(
                                            &head_icon::Scene {
                                                protocol: p,
                                                entities: &newest.entities,
                                                clients: &newest.clients,
                                                configstrings: net.configstrings(),
                                                viewer: ps_client,
                                                own: client_num,
                                                eye: cam.pos,
                                                heads: &heads,
                                                entity_pos: &entity_pos,
                                            },
                                            cam_right,
                                            cam_up,
                                        ));
                                        fx::sim::sort_back_to_front(&mut fx_quads, cam.pos);
                                    }
                                    r.set_fx_quads(&self.fs, fx_quads, time);
                                    // CoD2's snow (fx::cod2_cloud), tested against the map's roofs
                                    let clouds = self.fx.clouds(cam.pos, cam_right, cam_up, time);
                                    let map = self.session.map_name();
                                    r.set_fx_clouds(&self.fs, map, &clouds, time);
                                    r.set_marks(
                                        &self.fs,
                                        &self.fx.take_marks(time),
                                        &shadow_marks,
                                        (time * 1000.0) as i32,
                                        time,
                                        self.shell.cvar_f32("cg_marks") != 0.0,
                                    );
                                    r.set_fx_lights(&self.fx.lights(cam.pos, time));
                                    r.set_fog(
                                        net::FogParams::parse(
                                            net.configstring(net.protocol().cs.fog),
                                        ),
                                        time,
                                    );
                                    self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;

                                    prof.next(frame_prof::Stage::Hud);
                                    if let Some(hud) = &mut self.hud {
                                        let hud_t0 = Instant::now();
                                        hud.set_message_widths(
                                            self.shell.cvar_f32("cg_gameMessageWidth") as i32,
                                            self.shell.cvar_f32("cg_gameBoldMessageWidth") as i32,
                                            screen_w as u32,
                                        );
                                        hud.set_chat_rules(hud::chat::Rules::read(
                                            |c| self.shell.cvar(c),
                                            p.is_uo(),
                                        ));
                                        hud_quads = hud.build(&hud_frame);
                                        self.hud_ms = hud_t0.elapsed().as_secs_f32() * 1000.0;
                                        // The vote draw's `player_talk` on a
                                        // changed time or count.
                                        if std::mem::take(&mut hud.vote_sound) {
                                            self.audio.play_local(&self.fs, "player_talk");
                                        }
                                    }
                                    // After the HUD, which borrows the snapshot ring.
                                    if let Some(n) = fdc {
                                        net.send_reliable(&format!("fdc {n}"));
                                    }

                                    renderer::Frame {
                                        view_proj: camera::view_proj_from(
                                            cam.pos, cam.yaw, cam.pitch, view_roll, fov, aspect,
                                        ),
                                        eye: cam.pos,
                                        fwd: cam_forward,
                                        up: Vec3::Z,
                                        time,
                                        fov_x: fov,
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
                        view_weapon,
                        configstrings,
                        weapon_slot,
                        reserve,
                        switch_to,
                        rigs,
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
                        let chained = self.fx.spawn_chained(&self.fs, time);
                        frame_digest::clocked(|| self.audio.play_fx(&self.fs, chained));

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
                                    scale: 0,
                                    other_entity_num: u32::MAX,
                                    attacker_entity_num: -1,
                                    e_type: 0,
                                    eflags: 0,
                                },
                                configstrings,
                                &HashMap::new(),
                                &HashMap::new(),
                            );
                        }
                        input.jump = false; // Q3: a held jump doesn't autohop
                        let v = ps.view();
                        let (eye_forward, eye_right, eye_up) = camera::basis(v.yaw, v.pitch);
                        self.audio
                            .set_listener(v.eye, eye_forward, eye_right, eye_up, u32::MAX);
                        let mut bone_sets = Vec::new();
                        let mut fov =
                            camera::view_fov_x(camera::DEFAULT_FOV_DEG, aspect, widescreen);
                        let mut gun_pose = hud::scope::GunPose::default();
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
                            let frame = w.anims.get(&out.anim).map_or(0.0, |(anim, _)| {
                                anim.frame_pos(out.anim_time, out.looping)
                            });
                            w.pose(out.ads_frac, *ads_held, out.anim, frame);
                            // two models, hands then gun, in set_viewmodel order
                            bone_sets = (0..2)
                                .map(|m| w.pose.skin_matrices(&w.skeleton, m))
                                .collect();
                            fov = weapon::view_fov_x(
                                cg_fov(&self.shell),
                                Some(&w.def),
                                out.ads_frac,
                                *ads_held,
                                false,
                                false,
                            );
                            fov = camera::view_fov_x(fov, aspect, widescreen);
                            // The walk sight runs outside pmove, so the gun's
                            // sway reads its fraction off a copy.
                            let mut sighted = **ps;
                            sighted.weapon_pos_frac = out.ads_frac;
                            gun_pose = gun_aim
                                .step(
                                    Some(&w.def),
                                    &sighted,
                                    (time * 1000.0) as i32,
                                    Default::default(),
                                )
                                .unwrap_or_default();
                            if hud::scope::overlay_frac(&w.def, out.ads_frac, *ads_held).is_some() {
                                let screen = r.screen_size();
                                let fovs = (fov, camera::fov_y(fov, aspect));
                                let at = hud::scope::gun_point(gun_pose.angles, fovs, screen);
                                let layout = hud::scope::Layout::of(
                                    net::protocol::install_protocol(&self.fs),
                                );
                                hud::scope::build(&w.def, layout, at, screen, &mut scope_quads);
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
                                        scale: 0,
                                        other_entity_num: u32::MAX,
                                        attacker_entity_num: -1,
                                        e_type: 0,
                                        eflags: 0,
                                    },
                                    configstrings,
                                    &HashMap::new(),
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
                                        brass: &HashMap::new(),
                                        weapons: &[],
                                        eject_brass: false,
                                        configstrings: &[],
                                        cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
                                        blood: true,
                                        muzzles: &muzzles,
                                        shot_fallbacks: &HashMap::new(),
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
                                        scale: 0,
                                        other_entity_num: u32::MAX,
                                        attacker_entity_num: -1,
                                        e_type: 0,
                                        eflags: 0,
                                    };
                                    for r in fx::registry::resolve(&ev, &ctx) {
                                        match r {
                                            fx::registry::Resolved::Spawn { path, at, .. } => {
                                                let sounds =
                                                    self.fx.spawn(&self.fs, &path, at, time);
                                                self.audio.play_fx(&self.fs, sounds);
                                            }
                                            fx::registry::Resolved::Tracer {
                                                muzzle,
                                                impact,
                                                flesh,
                                                heavy,
                                                roll,
                                            } => {
                                                self.fx.spawn_tracer(
                                                    (muzzle, impact),
                                                    flesh,
                                                    (heavy, roll),
                                                    time,
                                                    dt,
                                                );
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        *fire_edge = false;
                        *reload_edge = false;

                        hud_quads = scope_quads;
                        self.audio.step(&HashMap::new(), &HashMap::new(), None);
                        r.set_fx_quads(
                            &self.fs,
                            self.fx.build_quads(v.eye, eye_right, eye_up, time),
                            time,
                        );
                        let clouds = self.fx.clouds(v.eye, eye_right, eye_up, time);
                        let map = self.session.map_name();
                        r.set_fx_clouds(&self.fs, map, &clouds, time);
                        r.set_marks(
                            &self.fs,
                            &self.fx.take_marks(time),
                            &[],
                            (time * 1000.0) as i32,
                            time,
                            self.shell.cvar_f32("cg_marks") != 0.0,
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
                                    v.eye, v.yaw, v.pitch, 0.0, fov, aspect,
                                ),
                                eye: v.eye,
                                fwd: camera::basis(v.yaw, v.pitch).0,
                                up: camera::up_hint(v.yaw, v.pitch, 0.0),
                                time,
                                fov_x: fov,
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
                                transform: play::view::pose_transform(gun_pose),
                                light_origin: ps.origin + Vec3::Z * ps.view_height(),
                                fov_x: fov,
                                bone_sets,
                            }),
                        )
                    }
                };
                // A drop or a failed load ends the session, not the program:
                // retail falls back to its console and menus.
                if demo_over && fatal.is_none() {
                    self.demo_completed(event_loop, local_ms as i32);
                    return;
                }
                if let Some(err) = fatal {
                    self.disconnect(Some(format!("{err:#}")));
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                let ui_span = frame_prof::span(frame_prof::Stage::Ui);
                // Over the game, under the menus (CoDMP.exe 0x416b10, drawn
                // in `CA_ACTIVE` with no fullscreen menu up, cod11-demos.md 8).
                if let Mode::Online {
                    net,
                    phase: Phase::Live(_),
                    ..
                } = &self.mode
                    && !self.ui.fullscreen()
                    && let Some((name, bytes)) = net.recording_status()
                {
                    let v1 = net.protocol().is_v1();
                    let show_name = v1 || self.shell.cvar_f32("cg_showdemoname") as i32 == 1;
                    let text = console::recording_text(name, bytes, show_name);
                    let (w, h) = r.screen_size();
                    hud_quads.extend(self.console.recording_line(&text, v1, w, h));
                }
                // CoD2's `SCR_UpdateFrame` (Mac 0x172d88): the name always,
                // white at (5, 479) on the 640x480 grid as 1.5's.
                if !self.ui.fullscreen()
                    && let Some((name, bytes)) = self.session.recording_status()
                {
                    let text = console::recording_text(name, bytes, true);
                    let (w, h) = r.screen_size();
                    hud_quads.extend(self.console.recording_line(&text, false, w, h));
                }
                // The CoD2 connect screen: the address and the download.
                for (i, line) in self
                    .session
                    .connect_lines(&self.localized)
                    .iter()
                    .enumerate()
                {
                    let (w, h) = r.screen_size();
                    let y = 240.0 + 20.0 * i as f32;
                    hud_quads.extend(self.console.screen_text(line, 24.0, y, w, h));
                }
                if self.ui.active() {
                    if let Mode::Online { net, .. } = &self.mode {
                        self.ui.offer_players(now, || {
                            let protocol = net.protocol();
                            net.snapshots()
                                .newest()
                                .map(|s| s.clients.values().map(|c| c.name(protocol)).collect())
                                .unwrap_or_default()
                        });
                    }
                    self.ui.frame(now, &self.shell);
                    let (w, h) = r.screen_size();
                    self.ui.set_screen(w, h);
                    hud_quads.extend(self.ui.build(w, h, &self.localized, &self.shell));
                }
                hud_quads.extend(self.session.cod4_front_quads(r.screen_size()));
                self.console.drain_log();
                self.console.update(
                    dt * 1000.0,
                    self.shell.cvar_f32("scr_conspeed"),
                    matches!(self.mode, Mode::Idle)
                        && !self.ui.active()
                        && !self.session.cod4_front_active()
                        && self.cod4_loading.is_none(),
                );
                if self.console.visible() {
                    let (w, h) = r.screen_size();
                    hud_quads.extend(self.console.build(w, h, elapsed.as_millis() as u64));
                }
                frame_digest::hud(&hud_quads);
                r.set_hud_quads(&self.fs, hud_quads);
                drop(ui_span);
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
                        self.session.cam_tag(),
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
                    if let Some(lines) = self.session.debug_lines() {
                        frame.hud_lines.extend(lines);
                    }
                }
                if frame_digest::on() {
                    frame_digest::view(&frame, vm.as_ref(), !self.session.live());
                }
                let t0 = Instant::now();
                r.render(frame, vm);
                self.render_ms = t0.elapsed().as_secs_f32() * 1000.0;
                if vcod_common::laps::loading()
                    && (self.digest_time().is_some() || self.session.cod4_in_game())
                {
                    vcod_common::laps::span(frame_start, || "the first frame in game".into());
                    vcod_common::laps::end_load();
                }
                if frame_digest::on() {
                    frame_digest::end_frame(self.digest_time());
                }
                if let Some(soak) = &mut self.soak {
                    soak.end_frame(self.audio.stats().misses);
                    for ask in soak.take_asks() {
                        match ask {
                            play::cod1_soak::Ask::Scores(on) => {
                                let effects =
                                    self.shell.execute(if on { "+scores" } else { "-scores" });
                                self.apply(event_loop, effects);
                            }
                            play::cod1_soak::Ask::Esc if self.ui.active() => {
                                let (_, effects) = self.ui.key(KeyCode::Escape, &self.shell);
                                self.ui_effects(event_loop, effects);
                                self.after_menu();
                            }
                            play::cod1_soak::Ask::Esc => self.escape(),
                        }
                    }
                }
                frame_prof::end_frame();
                if (!self.exec_in_game.is_empty()
                    || !self.exec_after.is_empty() && self.exec_after_last.is_none())
                    && (self.digest_time().is_some() || self.session.cod4_in_game())
                {
                    for line in std::mem::take(&mut self.exec_in_game) {
                        let effects = self.shell.execute(&line);
                        self.apply(event_loop, effects);
                    }
                    self.exec_after_last = Some(std::time::Instant::now());
                }
                if let (Some(last), Some(&(secs, _))) =
                    (self.exec_after_last, self.exec_after.front())
                    && last.elapsed().as_secs_f32() >= secs
                {
                    let (_, line) = self.exec_after.pop_front().expect("front");
                    frame_prof::exec(&line);
                    let effects = self.shell.execute(&line);
                    self.apply(event_loop, effects);
                    // after the line's own work (a `vid_restart`'s rebuild)
                    self.exec_after_last = Some(std::time::Instant::now());
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if !self.captured {
                return;
            }
            let (dx, dy) = (dx as f32, dy as f32);
            // `CL_MouseEvent` (0x40b0a0): the UI catcher takes the motion
            // unless `cl_bypassMouseInput`, under the console too.
            if self.ui.active() && !self.bypass_mouse() {
                let effects = self.ui.mouse_delta(dx, dy, &self.shell);
                self.ui_effects(event_loop, effects);
                return;
            }
            if let Some(r) = &self.renderer
                && self
                    .session
                    .cod4_menu_mouse(Some([dx, dy]), None, r.screen_size())
            {
                return;
            }
            if self.grabbed {
                self.mouse.add(dx, dy);
            }
        }
    }
}

/// The cgame's key text for `cmd` (0x30046940), for a command in its bind
/// table only (`hud::bind_commands`).
fn bound_key(
    shell: &console::shell::Shell,
    loc: &vcod_common::localize::Localized,
    p: &net::protocol::Protocol,
    cmd: &str,
) -> Option<String> {
    hud::bind_commands(p)
        .iter()
        .any(|c| c.eq_ignore_ascii_case(cmd))
        .then(|| shell.key_text(cmd, loc))
        .flatten()
}

/// `cg_fov` as cgame reads it, clamped to 80..160 (0x30032e20).
fn cg_fov(shell: &console::shell::Shell) -> f32 {
    shell.cvar_f32("cg_fov").clamp(weapon::CG_FOV, 160.0)
}

/// `CL_SystemInfoChanged` (CoDMP.exe 0x415eb0) at a gamestate or a `d 1`:
/// every systeminfo key becomes a client cvar (`rate` and `snaps` among
/// them). A demo sets none.
fn system_info_changed(
    shell: &mut console::shell::Shell,
    net: &mut net::NetClient<net::UdpTransport>,
    config_path: &std::path::Path,
) {
    if net.is_demo() {
        return;
    }
    let out = shell.apply_systeminfo(net.configstring(1));
    cvar_effects(shell, net, config_path, out);
}

/// What a forced cvar set asks for: a userinfo cvar goes back to the
/// server as a userinfo change, an archived one into the config, as
/// `cvar_modifiedFlags` does. A demo's connection sends nothing.
fn cvar_effects(
    shell: &console::shell::Shell,
    net: &mut net::NetClient<net::UdpTransport>,
    config_path: &std::path::Path,
    effects: Vec<console::shell::Effect>,
) {
    for effect in effects {
        match effect {
            console::shell::Effect::Userinfo if !net.is_demo() => {
                net.set_userinfo(shell.userinfo())
            }
            console::shell::Effect::SaveConfig => {
                if let Err(e) = std::fs::write(config_path, shell.config_text()) {
                    log::warn!("cannot write {}: {e}", config_path.display());
                }
            }
            _ => {}
        }
    }
}

/// `--probe-say`'s `SECS:COMMAND`.
fn parse_probe_say(s: &str) -> Result<(f32, String), String> {
    let (secs, cmd) = s.split_once(':').ok_or("expected SECS:COMMAND")?;
    let secs = secs.trim().parse().map_err(|e| format!("{secs}: {e}"))?;
    Ok((secs, cmd.to_string()))
}

/// The `sc_*` dvars as the cookies read them, each clamped to its
/// registered range (cod2-materials.md 9.8).
fn cookie_settings(shell: &console::shell::Shell) -> renderer::CookieSettings {
    let f = |name: &str, lo: f32, hi: f32| shell.cvar_f32(name).clamp(lo, hi);
    let n = |name: &str| f(name, 0.0, 24.0) as i32;
    renderer::CookieSettings {
        enable: shell.cvar_f32("sc_enable") != 0.0,
        blur: f("sc_blur", 0.0, 4.0) as u32,
        want: n("sc_wantCount"),
        want_margin: n("sc_wantCountMargin"),
        fade_range: f("sc_fadeRange", 0.0, 1.0),
        in_rate: f("sc_shadowInRate", 0.0, 20.0),
        out_rate: f("sc_shadowOutRate", 0.0, 20.0),
        length: f("sc_length", 1.0, 2000.0),
        casters: n("sc_debugCasterCount") as usize,
        receivers: n("sc_debugReceiverCount") as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CoD2 session's `vid_restart` stops its sounds as retail's
    /// `SND_StopSounds(1)` does; the offline fly camera keeps them.
    #[test]
    fn only_the_offline_viewer_keeps_sounds_on_vid_restart() {
        let cam = || FlyCamera::new(Vec3::ZERO, 0.0);
        assert!(Mode::Fly(cam()).keeps_sounds_on_vid_restart());
        assert!(!Mode::Cod2(cam()).keeps_sounds_on_vid_restart());
        assert!(!Mode::Idle.keeps_sounds_on_vid_restart());
    }

    /// The chat field takes printable keys but no quote, Backspace
    /// edits, Enter hands the line over and Escape drops it.
    #[test]
    fn the_chat_field_types_and_hands_over_its_line() {
        let mut f = hud::ChatField::default();
        for t in ["h", "i", "\"", "!"] {
            assert_eq!(chat_field_key(&mut f, KeyCode::KeyA, Some(t)), None);
        }
        assert_eq!(chat_field_key(&mut f, KeyCode::Backspace, None), None);
        assert_eq!(f.text, "hi");
        assert_eq!(
            chat_field_key(&mut f, KeyCode::Enter, None),
            Some(Some("hi".to_string()))
        );
        assert_eq!(chat_field_key(&mut f, KeyCode::Enter, None), Some(None));
        f.text = "x".into();
        assert_eq!(chat_field_key(&mut f, KeyCode::Escape, None), Some(None));
    }

    /// `Com_Frame`'s integer wait: retail's default 85 is 11 ms frames
    /// (90 fps), past 1000 no wait at all, and 0 or less the 1 ms floor.
    #[test]
    fn com_maxfps_paces_in_whole_milliseconds() {
        assert_eq!(frame_min_ms(85), 11);
        assert_eq!(frame_min_ms(125), 8);
        assert_eq!(frame_min_ms(2000), 0);
        assert_eq!(frame_min_ms(0), 1);
        assert_eq!(frame_min_ms(-5), 1);
    }

    /// A demo whose map no pak holds drops with retail's text, not a wait
    /// on a download.
    #[test]
    fn a_demo_without_its_map_says_couldnt_load() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let l = vcod_common::localize::Localized::load(&fs);
        assert_eq!(
            l.message(&couldnt_load("mp_nowhere")),
            "Couldn't load maps/mp/mp_nowhere.bsp"
        );
        let mut loader =
            loading::MapLoader::without_downloads("mp_nowhere".into(), couldnt_load("mp_nowhere"));
        assert_eq!(
            loader.step(&[], None, false, Instant::now()),
            loading::Action::Failed(couldnt_load("mp_nowhere"))
        );
    }

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
        hud: Option<hud::Hud>,
        audio: audio::AudioSystem,
        fx: fx::sim::FxSystem,
        quick_chat: quick_chat::QuickChat,
    }

    /// `reopen_fs` rewrites the process-wide impact table
    /// (`fx::registry::init`), so the tests that reopen take turns.
    static REOPEN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl Reopened {
        fn load(fs: &Pk3Fs) -> Reopened {
            let mut audio = audio::AudioSystem::new(
                fs,
                audio::AudioOpts {
                    enabled: false,
                    volume: 1.0,
                    rate: 0,
                },
            );
            audio.on_gamestate("mp_mod");
            Reopened {
                localized: vcod_common::localize::Localized::load(fs),
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
                &mut self.hud,
                &mut self.audio,
                &mut self.fx,
                &mut self.quick_chat,
            )
            .unwrap();
        }
    }

    /// A precached script menu with no file drops the game with the
    /// cgame's text (cgame_mp_x86.dll 0x3002c060).
    #[test]
    fn a_missing_script_menu_is_the_games_drop() {
        let fs = Pk3Fs::empty();
        let mut ui = frontend::Ui::new(&fs, "0");
        let shell = console::shell::Shell::new();
        let mut cs = vec![String::new(); 1300];
        assert_eq!(
            load_script_menus(&mut ui, &fs, &shell, &cs, &net::protocol::CS_LAYOUT_V1),
            Ok(())
        );
        cs[1181] = "team_gone".into();
        assert_eq!(
            load_script_menus(&mut ui, &fs, &shell, &cs, &net::protocol::CS_LAYOUT_V1),
            Err("Could not load script menu file 'team_gone'".into())
        );
    }

    /// A downloaded pak can carry the menus and strings the paks before it
    /// lacked; a miss cached against the old search path must not outlive it.
    #[test]
    fn reopen_reads_menus_and_strings_off_the_new_paks() {
        let _table = REOPEN_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", &[("readme.txt", b"")]);
        let mut fs = Pk3Fs::open(dir.path()).unwrap();
        let mut s = Reopened::load(&fs);
        let mut ui = frontend::Ui::new(&fs, "0");
        let team = ["team_mod".to_string()];
        assert_eq!(ui.load_script_menus(&fs, &team, "0"), team);

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
        assert!(
            ui.load_script_menus(&fs, &team, "0").is_empty(),
            "menu from the new pak"
        );
        assert_eq!(s.localized.translate("@MODMENU_ALLIES"), "Allies");
    }

    /// The fonts, the sound aliases, the effect files and the impact table
    /// are read off the search path too, and a miss there must not outlive
    /// it either.
    #[test]
    fn reopen_reads_fonts_aliases_and_effects_off_the_new_paks() {
        let _table = REOPEN_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            scale: 0,
            other_entity_num: 40,
            attacker_entity_num: -1,
            e_type: 0,
            eflags: 0,
        };
        let ctx = fx::registry::ResolveCtx {
            brass: &HashMap::new(),
            weapons: &[],
            eject_brass: false,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            blood: true,
            muzzles: &HashMap::new(),
            shot_fallbacks: &HashMap::new(),
            weapon_flash: &HashMap::new(),
            view_flash: None,
            view_body: None,
        };
        match fx::registry::resolve(&hit, &ctx).as_slice() {
            [fx::registry::Resolved::Spawn { path, .. }] => assert_eq!(path, "fx/mod/hit.efx"),
            other => panic!("impact table from the new pak: {other:?}"),
        }
    }

    /// An in-game `vid_restart` starts the cgame over on the engine's
    /// collision map and clock, parses configstring 7 as it stands and
    /// empties the cgame's chat; before the game is live it does nothing.
    #[test]
    fn a_vid_restart_in_a_game_restarts_the_cgame() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut cs = vec![String::new(); 8];
        cs[0] = "\\mapname\\mp_harbor".into();
        cs[7] = "m1carbine_mp".into();
        let (bsp, world) = reread_live_map(&fs, &cs).unwrap();
        let mut live = LivePhase::new(world, live_weapons(&fs, &cs[7]));
        live.clock.update(1000, 5000, 1, 0);
        let time = live.clock.server_time();
        live.drawn_pos.insert(3, Vec3::ONE);
        live.seat_frac = 1.0;
        let mut phase = Phase::Live(Box::new(live));
        cs[7] = "m1carbine_mp kar98k_mp".into();
        let mut view = play::view::OnlineView::default();
        let mut input = play::input::PlayInput::default();
        let mut predictor = play::predict::Predictor::default();
        let mut fx = fx::sim::FxSystem::new();
        let mut hud = hud::Hud::new(&fs).unwrap();
        hud.chat.push("hello", 0);
        let mut restart = |phase: &mut Phase, hud: &mut hud::Hud| {
            restart_live_cgame(
                phase,
                &mut view,
                &mut input,
                &mut predictor,
                Some(hud),
                &mut fx,
                &fs,
                &cs,
            )
        };
        assert!(!restart(&mut Phase::Connecting, &mut hud));
        assert!(!hud.chat.is_empty());
        assert!(restart(&mut phase, &mut hud));
        let Phase::Live(live) = &phase else {
            panic!("not live")
        };
        assert_eq!(live.world.model_count(), bsp.models.len());
        assert_eq!(live.clock.server_time(), time);
        assert!(live.drawn_pos.is_empty());
        assert_eq!(live.seat_frac, 0.0);
        assert_eq!(live.weapons.iter().flatten().count(), 2);
        assert!(hud.chat.is_empty());
    }

    /// A disconnect frees the cgame, chat ring and all.
    #[test]
    fn leaving_a_server_drops_the_chat_ring() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = hud::Hud::new(&fs).unwrap();
        hud.chat.push("last words", 0);
        leave_server_hud(&mut hud);
        assert!(hud.chat.is_empty());
    }

    /// A map change loads a fresh cgame as `vid_restart` does: the chat
    /// ring the gamestate's resets keep goes once the new map is up.
    #[test]
    fn a_map_load_starts_a_fresh_cgame() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = hud::Hud::new(&fs).unwrap();
        hud.chat.push("before the map change", 0);
        hud.on_gamestate();
        assert!(!hud.chat.is_empty(), "the gamestate parse keeps the ring");
        hud.chat.push("during the load", 1000);
        fresh_cgame(
            &mut play::view::OnlineView::default(),
            &mut play::input::PlayInput::default(),
            &mut play::predict::Predictor::default(),
            Some(&mut hud),
            &mut fx::sim::FxSystem::new(),
        );
        assert!(hud.chat.is_empty());
    }

    /// A Mods pick in a game reads the map again off the mod's paks, as
    /// `CL_InitCGame`'s `CM_LoadMap` does with no local server: a mod that
    /// ships its own `maps/mp/mp_carentan.bsp` replaces the stock geometry
    /// and collision.
    #[test]
    fn a_mod_switch_in_a_game_reads_the_mods_map() {
        let Some(stock) = vcod_common::testing::game_fs() else {
            return;
        };
        let base = vcod_common::testing::game_dir().join("main");
        let harbor = stock.read("maps/mp/mp_harbor.bsp").unwrap();
        let mods = tempfile::tempdir().unwrap();
        let game = mods.path().join("modharbor");
        std::fs::create_dir(&game).unwrap();
        make_pk3(&game, "z_mod.pk3", &[("maps/mp/mp_carentan.bsp", &harbor)]);

        let mut cs = vec![String::new(); 8];
        cs[0] = "\\mapname\\mp_carentan".into();
        let models = |fs: &Pk3Fs| {
            let (bsp, world) = reread_live_map(fs, &cs).unwrap();
            assert_eq!(world.model_count(), bsp.models.len());
            (bsp.models.len(), bsp.entities.len())
        };
        let before = models(&Pk3Fs::open_search(&base, None, None, false).unwrap());
        let after = models(&Pk3Fs::open_search(&base, Some(&game), None, false).unwrap());
        let harbor = bsp::parse(&harbor).unwrap();
        assert_ne!(before, after);
        assert_eq!(after, (harbor.models.len(), harbor.entities.len()));
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
