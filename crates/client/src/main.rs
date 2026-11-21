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
mod cod4_names;
mod cod4_radar;
mod cod4_scoreboard;
mod cod4_spotshadow;
mod cod4_sunshadow;
mod cod4_vehicles;
mod cod4_viewmodel;
mod cod4_waypoints;
mod console;
mod corona;
mod dlight;
mod edge_probe;
mod entities;
mod entity_light;
mod entity_material;
mod fatal;
mod flame_probe;
mod frame_clock;
mod frame_digest;
mod frame_prof;
mod frontend;
mod fx;
mod gamma;
mod head_icon;
mod hud;
mod hud_text;
mod input_events;
mod legs_probe;
mod lightmap_blocks;
mod live_frame;
mod loading;
mod marks;
mod online_frame;
mod play;
mod post;
mod probe;
mod quick_chat;
mod quit;
mod redraw;
mod renderer;
mod scene_copy;
mod screen_blend;
mod screenshot;
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
mod voice;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use glam::Vec3;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use vcod_common::asset_source::{AssetSource, RawFile};
use vcod_common::net::Msec;
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

/// The language check (CoDMP.exe 0x4a9b10) at start-up and each
/// `vid_restart`: `cl_languagesavailable`, and a `cl_language` whose strings
/// load (front-end doc 1). CoD 1 only.
fn check_language(shell: &mut console::shell::Shell, fs: &Pk3Fs) {
    if fs.game() != vcod_common::game::Game::Cod1 {
        return;
    }
    let chosen = shell.cvar("cl_language").unwrap_or("").to_string();
    let (count, set) = vcod_common::ui_menu::check_language(fs, &chosen);
    if count == 0 {
        console::log::print(
            "^1ERROR: No languages available because no localized assets were found",
        );
    }
    shell.force_set("cl_languagesavailable", &count.to_string());
    if let Some(v) = set {
        shell.force_set("cl_language", &v);
    }
}

/// [`TITLE`] with the GPU backend, for the window title and the main menu.
fn version_tag() -> String {
    match BACKEND.get() {
        Some(b) => format!("{TITLE} ({b})"),
        None => TITLE.to_string(),
    }
}

#[derive(Parser)]
#[command(about = "Client and map viewer for Call of Duty 1.1 and 1.5, United Offensive, Call of Duty 2 and Call of Duty 4", version = env!("VCOD_VERSION"))]
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
    /// Fly mode (`vcod <map>`): start at "x y z yaw [pitch]", the order and
    /// degrees the F3 overlay's camera line prints, instead of the first spawn
    #[arg(long, value_name = "X Y Z YAW [PITCH]", allow_hyphen_values = true,
          value_parser = FlyCamera::parse_view)]
    pos: Option<FlyCamera>,
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
    /// with the view swung off it. The wait before the script goes in as
    /// `!wsnap` and `!wcmd` lines. Tag the fixture `moveshot-<part>-1.5` or
    /// `-uo`; moveshot_ab.rs replays it (cod11-combat.md 2.1).
    #[arg(long, value_enum, value_name = "PART")]
    probe_moveshot: Option<probe::MoveShot>,
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
    probe_pronemove: Option<probe::ProneMove>,
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
    /// The map's base name (`mp_carentan`), which a levelshot is named by.
    name: String,
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
    /// no menu is up, as retail's is when disconnected. A CoD1 install
    /// reopens main each frame none is up (`App::cod1_front_end`).
    Idle,
    /// `vcod <map>`: a free camera over a CoD1, CoD2 or CoD4 map, no server.
    Fly(FlyCamera),
    /// A CoD2 or CoD4 session, online or a demo (`App::session`'s
    /// `Cod2Client` or `Cod4Client`): the camera follows its frames, or
    /// flies until the first one.
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
#[expect(clippy::too_many_arguments)]
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
    run().inspect_err(|e| fatal::record(TITLE, &format!("Error: {e:?}")))
}

fn run() -> Result<()> {
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
        fatal::record(TITLE, &info.to_string());
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
    // `vcod <map>` on CoD2 flies over it: no walk, server, probe or list.
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
                demo: args.save_demo,
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
                pronemove: args.probe_pronemove,
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
    let config_text = read_config(&read_path);
    if let Some(text) = &config_text {
        let mut read = |file: &str| exec_text(&fs, config_path.parent(), file);
        for effect in shell.exec_config(text, &mut read) {
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
    check_language(&mut shell, &fs);
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
    // A CoD2 `--connect` loads its map once the window's connect has the
    // gamestate (`App::connect_cod2`).
    let map_arg = match &cod4_session {
        Some(s) => Some(s.map_name().context("cod4: no mapname in serverinfo")?),
        None if cod2_connect => None,
        None => args.map.clone(),
    };
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
            let cam = match (args.pos.clone(), spawn) {
                (Some(cam), _) => cam,
                (None, Some((origin, yaw))) => {
                    FlyCamera::new(Vec3::from(origin) + Vec3::Z * 60.0, yaw)
                }
                (None, None) => {
                    let (min, max) = mesh::map_bounds(&bsp);
                    let center = (Vec3::from(min) + Vec3::from(max)) * 0.5;
                    log::warn!("no player spawn found, starting at the center of the map");
                    FlyCamera::new(center, 0.0)
                }
            };
            // A watched CoD4 session drives the camera.
            match cod4_session.is_some() {
                true => Mode::Cod2(cam),
                false => Mode::Fly(cam),
            }
        };
        (
            mode,
            Some(World {
                name: map_base_name(&map),
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
        fs_game: None,
        fs_pure: None,
        connect_addr: args.connect.clone(),
        team: args.team.clone(),
        weapon: args.weapon.clone(),
        console,
        shell,
        config_path,
        grab_before_console: false,
        keys_down: Vec::new(),
        key_releases: Vec::new(),
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
        shot: None,
        shooter: Default::default(),
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
            (true, None) => session::Session::Cod2(Box::new(cod2_client::Cod2Client::new(None))),
            (false, None) if cod4_menu => session::Session::Cod4Menu(Box::new(
                cod4_front::Cod4Front::new(args.game_dir.clone()),
            )),
            (false, None) => session::Session::Cod1,
        },
        wheel: console::keys::WheelNotches::default(),
    };
    // A CoD2 `--connect` runs the handshake and its downloads beside the
    // window, as the console's `connect` does, under the connect screen.
    if let Some(addr) = args.connect.clone().filter(|_| cod2_connect) {
        app.start_cod2_connect(addr);
    } else if matches!(app.mode, Mode::Idle) && app.cod4_loading.is_none() {
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

/// A switch cvar, on unless set to 0 (on when unregistered: the CoD4
/// Options dvars exist only on a CoD4 install).
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

/// A config file's text, decoded lossily as `exec` decodes: `None` when
/// there is none, or with a warning when it exists but cannot be read.
fn read_config(path: &std::path::Path) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => Some(config_from_bytes(path, bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("cannot read {}: {e}", path.display());
            None
        }
    }
}

/// A config's bytes as text ([`vcod_common::text::utf8_or_latin1`]).
fn config_from_bytes(path: &std::path::Path, bytes: Vec<u8>) -> String {
    if std::str::from_utf8(&bytes).is_err() {
        log::debug!("{}: not UTF-8, read as Latin-1", path.display());
    }
    vcod_common::text::utf8_or_latin1(bytes)
}

/// A CoD2 profile's config into `shell`: run when the file is there,
/// written from what is set now only when it is absent, so a file that
/// fails to read is never overwritten.
fn load_profile_config(shell: &mut console::shell::Shell, fs: &Pk3Fs, path: &std::path::Path) {
    let write = match std::fs::read(path) {
        Ok(bytes) => {
            let mut read = |file: &str| exec_text(fs, path.parent(), file);
            let _ = shell.exec_config(&config_from_bytes(path, bytes), &mut read);
            shell.upgrade_config(vcod_common::game::Game::Cod2).1
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            log::warn!("cannot read {}: {e}", path.display());
            false
        }
    };
    if write && let Err(e) = std::fs::write(path, shell.config_text()) {
        log::warn!("cannot write {}: {e}", path.display());
    }
}

/// [`App::demo_dirs`] off the fields it reads, for a caller holding the
/// mode borrowed.
fn demo_dirs(
    config_path: &std::path::Path,
    game_dir: &std::path::Path,
    fs_game: &Option<String>,
    mod_dir: &str,
    shell: &console::shell::Shell,
) -> Vec<std::path::PathBuf> {
    let base = config_path
        .parent()
        .map_or_else(|| game_dir.to_path_buf(), std::path::Path::to_path_buf);
    let fs_game_cvar = net::download::mod_dir(shell.cvar("fs_game").unwrap_or(""), mod_dir);
    let mut dirs: Vec<_> = fs_game
        .iter()
        .chain(&fs_game_cvar)
        .map(|g| game_dir.join(g))
        .chain([base])
        .map(|d| d.join("demos"))
        .collect();
    dirs.dedup();
    dirs
}

/// `exec`'s file: the search path's, else the one under `config_dir`
/// (vcod's own fallback), which a rooted, `..` or drive name never reaches.
fn exec_text(fs: &Pk3Fs, config_dir: Option<&std::path::Path>, file: &str) -> Option<Vec<u8>> {
    fs.read(file)
        .or_else(|| std::fs::read(vcod_common::game_dir::confined(config_dir?, file)?).ok())
}

/// `record [name]` on `net` (CoDMP.exe 0x40e540): the file in `dir`, its
/// refusals and progress printed as retail prints them.
fn start_record<T: net::Transport>(
    net: &mut net::NetClient<T>,
    dir: &std::path::Path,
    name: Option<&str>,
) {
    // Refused before the file is made, as retail checks first.
    if net.is_recording() {
        console::log::print("Already recording.");
        return;
    }
    let Some((path, name)) = play::demos::record_path(dir, name, net.protocol()) else {
        console::log::print("ERROR: couldn't open.");
        return;
    };
    let file = match std::fs::create_dir_all(dir).and_then(|_| std::fs::File::create(&path)) {
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
#[expect(clippy::too_many_arguments)]
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
    fx.restart_cgame();
    *title = format!("{} — loading {map}", version_tag());
    if let Some(window) = window {
        window.set_title(title);
    }
    // A demo sets no pak list (`CL_SystemInfoChanged` returns first while
    // one plays, CoDMP.exe 0x415eb0), so nothing is fetched, and a map
    // missing here fails as `CM_LoadMap` does (0x419c90).
    let systeminfo = net.configstring(1).to_string();
    if net.is_demo() {
        return Ok(Phase::Loading {
            loader: loading::MapLoader::without_downloads(map.clone(), couldnt_load(&map))
                .for_systeminfo(systeminfo),
        });
    }
    // The pak candidates in search order; `MapLoader` caps them.
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
            loader: loading::MapLoader::without_downloads(map, why).for_systeminfo(systeminfo),
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
        loader: loading::MapLoader::new(map, candidates).for_systeminfo(systeminfo),
    })
}

/// `CM_LoadMap`'s drop for a map no pak holds (CoDMP.exe 0x419c90): the
/// localized `Couldn't load maps/mp/<map>.bsp`.
fn couldnt_load(map: &str) -> String {
    format!("EXE_ERR_COULDNT_LOAD\u{15}maps/mp/{map}.bsp")
}

/// [`App::restart_ui`]'s front end: `ui_menuFiles` back to its default,
/// a fresh UI off `fs`, the server lists of `ui` carried over.
fn restart_ui(
    ui: &mut frontend::Ui,
    shell: &mut console::shell::Shell,
    fs: &Pk3Fs,
    game_dir: &std::path::Path,
    mod_dir: &str,
) {
    shell.execute("set ui_menuFiles ui_mp/menus.txt");
    let browser = std::mem::take(&mut ui.browser);
    *ui = new_ui(
        fs,
        game_dir,
        mod_dir,
        shell.cvar("cl_language").unwrap_or(""),
    );
    ui.set_browser(browser);
}

/// The front end off `fs`, with the favourites file and the Mods menu's
/// directory listing under `game_dir`.
fn new_ui(fs: &Pk3Fs, game_dir: &std::path::Path, mod_dir: &str, language: &str) -> frontend::Ui {
    use vcod_common::net::server_cache::Layout;
    // Each game's own cache file and shape (cod11-front-end.md 8).
    let layout = match fs.game() {
        vcod_common::game::Game::Cod2 => Layout::Cod2,
        _ => match vcod_common::net::protocol::install_protocol(fs)
            .transport
            .version
        {
            6 => Layout::Cod15,
            22 => Layout::Uo,
            _ => Layout::Cod11,
        },
    };
    let mut ui = frontend::Ui::new(fs, language)
        .with_server_cache(game_dir.join(layout.file_name()), layout);
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
#[expect(clippy::too_many_arguments)]
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

/// A map argument's base name, as the renderer's world keeps it: no
/// folder, no extension (`maps/mp/mp_carentan.bsp` -> `mp_carentan`).
fn map_base_name(map: &str) -> String {
    let file = map.rsplit(['/', '\\']).next().unwrap_or(map);
    file.split_once('.')
        .map_or(file, |(stem, _)| stem)
        .to_string()
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
    fx.restart_cgame();
}

/// What `CG_Init` (cgame 0x30022f90) does with the gamestate besides the
/// live phase: the 140/204 mirror written into the console's cvars
/// (0x30023106) and the shellshocks registered (0x30021a3a), where a file
/// that fails is a `CG_Error` that drops the game. `Err` is its text.
fn cgame_init(
    view: &mut play::view::OnlineView,
    join: &mut play::join::Join,
    shell: &mut console::shell::Shell,
    fs: &Pk3Fs,
    net: &mut net::NetClient<net::UdpTransport>,
    config_path: &std::path::Path,
) -> Result<(), String> {
    let out = join.cvars.apply_mirror(net.configstrings(), shell);
    cvar_effects(shell, net, config_path, out);
    let p = net.protocol();
    let names = (1..).map_while(|i| p.cs.slot(net::protocol::CsRange::ShellShock, i));
    let names = names.map(|i| net.configstring(i));
    view.shock.register(fs, names)
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
        name: map_base_name(map),
        bsp,
        fog: None,
        materials: None,
    });
    Ok(phase)
}

/// The between-maps frame: the status text over an empty view, with chat
/// still flowing through the normal HUD build (no snapshot clients).
#[expect(clippy::too_many_arguments)]
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
    /// Bound keys whose press reached the binds and that are still down:
    /// what `Key_ClearStates` lets go ([`Self::clear_held_keys`]).
    keys_down: Vec<String>,
    /// The release commands a [`Self::clear_held_keys`] ran, applied at
    /// the next `about_to_wait`.
    key_releases: Vec<console::shell::Effect>,
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
    /// A screenshot command waiting for the next frame, and the writer.
    shot: Option<screenshot::Command>,
    shooter: screenshot::Shooter,
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
    /// Menu labels and localized strings, loaded off the search path.
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
    /// The game's session: CoD2's or CoD4's client or CoD4's main menu,
    /// or CoD1's (whose state is
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

    /// `CL_AdjustAngles` (CoDMP.exe 0x40ade0) online, then `CL_MouseMove`
    /// (0x40b240) once a frame: the motion since the last frame, or with
    /// `m_filter` its mean with the last frame's, turns the view or,
    /// online, moves the cmd (`MouseLook::moves`).
    fn mouse_frame(&mut self) {
        let filter = self.shell.cvar_f32("m_filter") as i32 != 0;
        let (dx, dy) = self.mouse.take(filter);
        if !self.grabbed {
            self.mouse.clear();
            return;
        }
        // `frame_msec`, the ms since the last frame's cmd, at most 200
        // (CoDMP.exe 0x40b8ef).
        let frame_ms = Instant::now()
            .saturating_duration_since(self.last_frame)
            .as_millis()
            .min(200) as i32;
        if let Mode::Online { input, .. } = &mut self.mode {
            let speeds = play::input::AngleSpeeds {
                yaw: self.shell.cvar_f32("cl_yawspeed"),
                pitch: self.shell.cvar_f32("cl_pitchspeed"),
                anglespeedkey: self.shell.cvar_f32("cl_anglespeedkey"),
            };
            input.adjust_angles(frame_ms, speeds);
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
            accel: self.shell.cvar_f32("cl_mouseAccel"),
            frame_ms,
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

    /// Gives the mouse to the view (`grabbed`) or takes it back.
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
        // `Key_ClearStates` (CoDMP.exe 0x40e260): each held key's release
        // goes through `CL_KeyEvent` in key number order, so a `+` bind's
        // `-` half runs.
        let mut down = std::mem::take(&mut self.keys_down);
        down.sort_by_key(|k| console::keys::number(k));
        for key in down {
            let effects = self.shell.key_event(&key, false);
            self.key_releases.extend(effects);
        }
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

    /// Keys, mouse buttons and the wheel go through the binds: online,
    /// a CoD2 session included (`Mode::Cod2`,
    /// `play::cod2_input`).
    fn plays_binds(&self) -> bool {
        matches!(self.mode, Mode::Online { .. }) || self.session.live()
    }

    /// Whether keys go to the console: it is down, or there is nothing else.
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
        restart_ui(
            &mut self.ui,
            &mut self.shell,
            &self.fs,
            &self.game_dir,
            &self.mod_dir,
        );
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
        let shell = &self.shell;
        hud::font::set_front_end_colours(&self.fs, &|n| shell.cvar(n).map(str::to_string));
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
                UiEffect::MutePlayer(client) => {
                    if let Some(c) = self.session.cod2_mut() {
                        c.toggle_mute(client);
                    }
                }
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
        self.keys_down.retain(|k| k != key);
        if pressed {
            self.keys_down.push(key.to_string());
        }
        let effects = self.shell.key_event(key, pressed);
        self.apply(event_loop, effects);
    }

    /// Carries out what the console's commands asked for.
    fn apply(&mut self, event_loop: &ActiveEventLoop, effects: Vec<console::shell::Effect>) {
        use console::shell::Effect;
        let mut queue = std::collections::VecDeque::from(effects);
        while let Some(effect) = queue.pop_front() {
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
                Effect::Screenshot { jpeg, args } => {
                    self.shot = Some(screenshot::parse(jpeg, &args));
                }
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
                // after the exec'd file, ahead of what was queued behind it
                Effect::Then(rest) => {
                    for e in self.shell.execute(&rest).into_iter().rev() {
                        queue.push_front(e);
                    }
                }
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
        check_language(&mut self.shell, &self.fs);
        self.vid_restart_ui();
        self.ignore_hw_gamma = ignore_hw_gamma(&self.shell);
        // Retail's `vid_restart` reloads every image; vcod rebuilds
        // only when the format, the MSAA or an image setting moved.
        self.shell.cod4_set_picmip();
        let textures = self.texture_settings();
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
        let textures = self.texture_settings();
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

    /// [`texture_cvars`] with the install's custom mip rule: 1.1's and
    /// 1.5's client refuses an image over a `#N` level of the wrong size,
    /// UO's filters the level.
    fn texture_settings(&self) -> texture_settings::TextureSettings {
        let mut t = texture_cvars(&self.shell, self.textures.filter);
        t.refuse_bad_levels = self.game == vcod_common::game::Game::Cod1
            && !net::protocol::install_protocol(&self.fs).is_uo();
        t
    }

    fn build_renderer_now(&mut self, window: Arc<Window>) -> Result<Renderer> {
        self.textures = self.texture_settings();
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

    /// `vid_restart` with a changed `vc_hdr`, MSAA or image setting (or
    /// CoD4's `r_applyPicmip`): every world pipeline targets the scene
    /// format and samples the images, so the renderer is rebuilt and the
    /// map reloaded into it. The entity scene's model handles index the old renderer, so
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
        let text = exec_text(&self.fs, self.config_path.parent(), file);
        let Some(text) = text else {
            console::log::print(&format!("couldn't exec {file}"));
            return;
        };
        if self.exec_depth >= console::shell::EXEC_DEPTH {
            log::warn!("exec {file}: too deep, skipped");
            return;
        }
        console::log::print(&format!("execing {file}"));
        self.exec_depth += 1;
        let effects = self.shell.execute(&vcod_common::text::utf8_or_latin1(text));
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
        demo_dirs(
            &self.config_path,
            &self.game_dir,
            &self.fs_game,
            &self.mod_dir,
            &self.shell,
        )
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
            .filter_map(|d| vcod_common::game_dir::confined(&d, &file))
            .find(|p| p.is_file());
        if self.in_game() || self.session.busy() {
            self.disconnect(None);
        }
        let Some(path) = path else {
            return self.disconnect(Some(format!("EXE_ERR_NOT_FOUND\u{15}demos/{file}")));
        };
        log::info!("playing {}", path.display());
        // `timedemo`'s csv goes where `record` writes.
        let csv_dir = self.demo_dirs()[0].clone();
        let r_mode = self.shell.int("r_mode");
        let played = std::fs::read(&path)
            .map_err(anyhow::Error::from)
            .and_then(|data| play::cod2::Cod2Session::play_demo(data, timedemo))
            .and_then(|mut s| {
                s.write_timedemo_csv(csv_dir, r_mode);
                self.enter_cod2(s)
            });
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

    /// `cl_avidemo`'s fixed frame step, queueing its silent shot
    /// (`screenshot::avidemo`).
    fn avidemo(&mut self) -> Option<std::time::Duration> {
        let frames = self.shell.cvar_f32("cl_avidemo") as i32;
        if frames == 0 {
            return None;
        }
        let timescale = match self.shell.cvar("timescale") {
            Some(_) => self.shell.cvar_f32("timescale"),
            None => 1.0,
        };
        let f = screenshot::avidemo(
            self.game,
            net::protocol::install_protocol(&self.fs).is_uo(),
            frames,
            timescale,
            self.client_active(),
            self.shell.cvar_f32("cl_forceavidemo") != 0.0,
        )?;
        if let Some(shot) = f.shot {
            self.shot = Some(screenshot::Command::Screen(shot));
        }
        Some(f.step)
    }

    /// `cls.state` at `CA_ACTIVE`: a game or a demo past its first
    /// snapshot.
    fn client_active(&self) -> bool {
        match &self.mode {
            Mode::Online { net, .. } => net.state() == net::NetState::Active,
            _ => self.session.cod4_in_game() || self.session.frame_time().is_some(),
        }
    }

    /// `fs_homepath/<fs_gamedir>`, where shots write: the mod's folder
    /// under `fs_game`, else the base game's.
    fn gamedir(&self) -> std::path::PathBuf {
        let game = self.fs_game.as_deref().unwrap_or(&self.mod_dir);
        self.game_dir.join(game)
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

    /// CoD2's `Com_ChangePlayerProfile` (Mac 0x111936): `players/active.txt`
    /// names the profile, `com_playerProfile` holds it and its config file
    /// takes over, run when it has one, else written from what is set now.
    /// Retail resets every dvar first; vcod keeps what the file does not set.
    fn load_profile(&mut self, name: &str) {
        let players = self.game_dir.join(&self.mod_dir).join("players");
        let Some(dir) = vcod_common::game_dir::confined_component(&players, name) else {
            log::warn!("player profile {name:?} refused");
            return;
        };
        if let Err(e) = std::fs::write(players.join("active.txt"), name) {
            log::warn!("cannot write {}: {e}", players.join("active.txt").display());
        }
        self.config_path = dir.join(CONFIG_FILE);
        self.shell
            .execute(&format!("set com_playerProfile \"{name}\""));
        load_profile_config(&mut self.shell, &self.fs, &self.config_path);
        log::info!("player profile {name}: {}", self.config_path.display());
    }

    /// The Mods menu's `RunMod` (and `Quake3` with `None`): `fs_game` and a
    /// `vid_restart`, which brings the main menu back up off the mod's
    /// paks. In a game, `vid_restart` keeps the connection: the search path
    /// reopens under the server's pure list, the UI reloads closed and the
    /// cgame half sends `cp` again (CoDMP.exe 0x40fbe0,
    /// docs/research/cod11-front-end.md section 17).
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
            name: map_base_name(
                net::info_value_for_key(net.configstring(0), "mapname").unwrap_or_default(),
            ),
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
            join,
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
        if let Err(e) = cgame_init(
            view,
            join,
            &mut self.shell,
            &self.fs,
            net,
            &self.config_path,
        ) {
            self.disconnect(Some(e));
        }
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
            name: map_base_name(&map),
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
    /// another map starts its load on a worker under the loading screen
    /// (`Cod4Client::start_map_load`, the zones over the gamestate's models
    /// and weapons); once it lands the session takes its parts and the
    /// world and the prewarm go to the GPU here.
    fn cod4_level(&mut self) -> Result<()> {
        let Some(c) = self.session.cod4_mut() else {
            return Ok(());
        };
        if let Some(map) = c.session.take_map_change() {
            vcod_common::laps::begin_load(format!("cod4 map change {map}"));
            c.start_map_load(&self.game_dir, map);
        }
        let Some(done) = c.take_map_load() else {
            return Ok(());
        };
        let cod4_loading::MapLoaded {
            map: mut m,
            name: map,
            hud,
            fs,
        } = done?;
        c.install_hud(hud);
        self.fs = fs;
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
            name: map_base_name(&map),
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
            name: map_base_name(&map_name),
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
        self.audio.on_gamestate("menu");
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

    /// No game up on a CoD1 install, whose key handler and frame follow
    /// CoDMP.exe's with no game: the main menu, or the console over it.
    fn cod1_front_end(&self) -> bool {
        matches!(self.mode, Mode::Idle)
            && self.game == vcod_common::game::Game::Cod1
            && self.cod4_loading.is_none()
    }

    /// `UI_SetActiveMenu` 1 with no game up: the main menu, and the error
    /// popup over it while `com_errorMessage` holds one.
    fn reopen_main(&mut self) {
        let mut out = Vec::new();
        self.ui.set_active_main(&self.shell, &mut out);
        self.ui_pending.extend(out);
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
        // `CL_Frame` (CoDMP.exe 0x4112c0..0x4112e9): disconnected, with
        // the UI's key catcher down and no server running, the client
        // stops its sounds and reopens the main menu every frame.
        if self.cod1_front_end() && !self.ui.active() && self.ui.has_menu("main") {
            self.audio.stop_all();
            self.reopen_main();
        }
        let pending = std::mem::take(&mut self.ui_pending);
        self.ui_effects(event_loop, pending);
        let releases = std::mem::take(&mut self.key_releases);
        self.apply(event_loop, releases);
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
            WindowEvent::KeyboardInput { event, .. } => self.on_key(event_loop, event),
            WindowEvent::Focused(false) => self.clear_held_keys(),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(event_loop, position),
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_input(event_loop, state, button)
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(event_loop, delta),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
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
    p: &net::protocol::Cod1Wire,
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

    /// `exec`'s disk fallback reads a file beside the config, in a
    /// subdirectory too, and nothing a rooted, `..` or drive name points at.
    #[test]
    fn exec_reads_only_under_the_config_directory() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        std::fs::create_dir_all(main.join("cfgs")).unwrap();
        std::fs::write(main.join("mine.cfg"), "a").unwrap();
        std::fs::write(main.join("cfgs/sub.cfg"), "b").unwrap();
        std::fs::write(root.path().join("secret"), "s").unwrap();
        let fs = Pk3Fs::empty();
        let read = |f: &str| exec_text(&fs, Some(&main), f);
        assert_eq!(read("mine.cfg").as_deref(), Some(&b"a"[..]));
        assert_eq!(read("cfgs/sub.cfg").as_deref(), Some(&b"b"[..]));
        let abs = root.path().join("secret");
        for bad in [
            "../secret",
            "cfgs/../../secret",
            "C:secret",
            abs.to_str().unwrap(),
        ] {
            assert_eq!(read(bad), None, "{bad}");
        }
    }

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

    /// A config holding a byte that is not UTF-8 (a retail config's
    /// Latin-1 `ö`) runs as Latin-1, at startup and as a CoD2 profile's,
    /// the profile's file not overwritten, and the `ö` survives vcod's
    /// write and read back. A UTF-8 file reads as UTF-8.
    #[test]
    fn a_config_that_is_not_utf8_reads_as_latin1_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        // Not vcod's header, so the stamp is current and nothing upgrades.
        let bytes = b"bind F1 \"say sch\xf6n\"\n".to_vec();
        std::fs::write(&path, &bytes).unwrap();

        let text = read_config(&path).expect("the config reads");
        let mut shell = console::shell::Shell::new();
        shell.exec_config(&text, &mut |_| None);
        assert_eq!(shell.bind("F1"), Some("say sch\u{f6}n"));

        let mut shell = console::shell::Shell::new();
        load_profile_config(&mut shell, &Pk3Fs::empty(), &path);
        assert_eq!(shell.bind("F1"), Some("say sch\u{f6}n"));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "the profile's config was rewritten"
        );

        std::fs::write(&path, shell.config_text()).unwrap();
        let mut again = console::shell::Shell::new();
        again.exec_config(&read_config(&path).unwrap(), &mut |_| None);
        assert_eq!(again.bind("F1"), Some("say sch\u{f6}n"));

        std::fs::write(&path, "bind F1 \"say sch\u{f6}n\"\n").unwrap();
        let mut utf8 = console::shell::Shell::new();
        utf8.exec_config(&read_config(&path).unwrap(), &mut |_| None);
        assert_eq!(utf8.bind("F1"), Some("say sch\u{f6}n"));
    }

    /// The gamestate's search path restart and `vid_restart` rebuild the
    /// UI through [`restart_ui`], which keeps the Internet and LAN lists.
    #[test]
    fn a_ui_restart_keeps_the_server_lists() {
        use frontend::browser::Source;
        let dir = tempfile::tempdir().unwrap();
        let fs = Pk3Fs::empty();
        let mut shell = console::shell::Shell::new();
        let mut ui = new_ui(&fs, dir.path(), "main", "");
        let addr = std::net::SocketAddrV4::new([10, 0, 0, 1].into(), 28960);
        ui.browser.list_server(Source::Internet, addr);
        restart_ui(&mut ui, &mut shell, &fs, dir.path(), "main");
        assert_eq!(ui.browser.listed(Source::Internet), vec![addr]);
    }
}
