//! The headless probes `--probe-*` runs against a server or a demo: a
//! scripted walk that prints the lines the capture recipes parse.

use super::*;
use vcod_common::net::Msec;

/// The scripted player of `vcod --net-probe ADDR --game cod2 --probe-team
/// T`: a 16 s loop of a walk, a strafe while turning, a jump, a back-up, a
/// crouched walk, a stand, three semi-automatic taps, a sighted tap, a
/// weapon switch and back, a reload and a still second, through the same
/// binds a player presses ([`Cod2Input`]); the switch presses 2, the way
/// back turns the wheel up, both through CoD2's stock binds. With
/// `VCOD_PROBE_REWEAPON=<weapon>` it presses X (`openscriptmenu ingame
/// changeweapon`) 17 s in and answers the weapon menu that opens with it.
/// `VCOD_PROBE_INGAME=<response>` instead answers the `ingame` menu's
/// button with that response 17 s in, as its click does. Every `t`, `u`,
/// `K` and `v` left to the player prints as a `cod2: menu command` line.
struct ProbeScript {
    /// The impulses already sent this loop, by their time.
    done: Vec<u32>,
    lap: u32,
    shell: crate::console::shell::Shell,
    reweapon: Option<String>,
    reweapon_asked: bool,
    ingame: Option<String>,
}

impl Default for ProbeScript {
    fn default() -> Self {
        let mut shell = crate::console::shell::Shell::new();
        shell.use_cod2_binds();
        Self {
            done: Vec::new(),
            lap: 0,
            shell,
            reweapon: std::env::var("VCOD_PROBE_REWEAPON").ok(),
            reweapon_asked: false,
            ingame: std::env::var("VCOD_PROBE_INGAME").ok(),
        }
    }
}

impl ProbeScript {
    fn step(&mut self, s: &mut Cod2Session, cam: &mut FlyCamera, t: f32) {
        let lap = (t / 16.0) as u32;
        if lap != self.lap {
            self.lap = lap;
            self.done.clear();
        }
        let p = t % 16.0;
        let within = |a: f32, b: f32| p >= a && p < b;
        let held = [
            (Action::Forward, within(0.0, 4.5) || within(6.0, 7.5)),
            (Action::Right, within(3.0, 4.5)),
            (Action::Back, within(5.0, 6.0)),
            (Action::Jump, within(4.5, 4.6) || within(7.5, 7.6)),
            (
                Action::Attack,
                within(8.0, 8.1) || within(8.6, 8.7) || within(9.2, 9.3) || within(10.6, 10.7),
            ),
            (Action::Reload, within(13.0, 13.1)),
        ];
        for (a, down) in held {
            s.input.key(a, down);
        }
        if within(3.0, 4.5) {
            cam.look([0.0, 0.3]);
        }
        let ps = s.view.snap.as_ref().map(|v| v.ps.raw.clone());
        for (at, a) in [
            (6.0, Action::Crouch),
            (10.0, Action::ToggleAds),
            (11.0, Action::ToggleAds),
        ] {
            let key = (at * 10.0) as u32;
            if p >= at && !self.done.contains(&key) {
                self.done.push(key);
                s.input.impulse(a, ps.as_ref());
            }
        }
        // The keys as `input_events.rs` routes them: bind, effect, input.
        for (at, k) in [(14.5, "2"), (15.5, "MWHEELUP")] {
            let key = (at * 10.0) as u32;
            if p >= at && !self.done.contains(&key) {
                self.done.push(key);
                let ps = s.frame().map(|f| f.snap.ps.raw.clone());
                for e in self.shell.key_event(k, true) {
                    s.input.effect(&e, ps.as_ref());
                }
                println!("cod2: pressed {k}, selected {}", s.input.weapons().0);
            }
        }
        if self.reweapon.is_some() && !self.reweapon_asked && t >= 17.0 {
            self.reweapon_asked = true;
            for e in self.shell.key_event("X", true) {
                if let Effect::OpenScriptMenu { menu, response } = e {
                    s.menu_response(&menu, &response);
                }
            }
        }
        if !self.reweapon_asked
            && t >= 17.0
            && let Some(response) = self.ingame.clone()
        {
            self.reweapon_asked = true;
            s.menu_response("ingame", &response);
        }
        // The weapon menu the X opened, answered as a player clicks it.
        for words in s.take_menu_commands() {
            println!("cod2: menu command {}", words.join(" "));
            if let (["t", n, ..], Some(w)) = (
                words
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice(),
                &self.reweapon,
            ) && let Some(menu) = n
                .parse()
                .ok()
                .and_then(|n| s.view.configstrings.script_menu(n))
                .filter(|m| m.starts_with("weapon_"))
                .map(str::to_string)
            {
                println!("cod2: the player's {menu}: {w}");
                s.menu_response(&menu, &w.clone());
            }
        }
    }
}

/// Reads `map`'s world with `s` on the pump, as the windowed client
/// loads; `stall` seconds are added to the load. Prints how it went.
fn probe_load(s: Box<Cod2Session>, map: &str, fs: &Pk3Fs, stall: u64) -> Box<Cod2Session> {
    let pump = s.pump();
    let t0 = Instant::now();
    // the map parsed once, the prediction's collision built off it
    let world = vcod_cod2::map::MapFile::find(fs, map).ok().map(|f| {
        let w = f.world();
        let cw = f.clip().map(|c| prediction_world(&w, Arc::new(c)));
        (w, cw)
    });
    std::thread::sleep(Duration::from_secs(stall));
    let mut s = pump.finish();
    println!(
        "cod2: loaded {map} ({}) under the pump in {:.1} s",
        if world.is_some() { "ok" } else { "failed" },
        t0.elapsed().as_secs_f32()
    );
    if let Some((_, cw)) = world {
        s.install_world(fs, map, cw, t0);
    }
    s
}

/// `vcod --game cod2 --probe-demo FILE [--probe-timedemo]`: play a `.dm_1`
/// headless through the session's demo path (the same parse, clock, view,
/// events, cues, effects and scene builders as the window's), a line a
/// second, then the totals. Fails on a parse error or a demo with no
/// snapshot.
pub fn demo_probe(path: &Path, fs: &Pk3Fs, timedemo: bool) -> anyhow::Result<()> {
    let data = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let bytes = data.len();
    let s = Cod2Session::play_demo(data, timedemo)?;
    let map = s.map_name().unwrap_or("?").to_string();
    println!(
        "cod2: demo {} ({bytes} bytes): gamestate for {map}, client {}, {} configstrings, {} baselines",
        path.display(),
        s.view.client_num,
        s.view
            .configstrings
            .all()
            .iter()
            .filter(|c| !c.is_empty())
            .count(),
        s.client
            .parser
            .gamestate
            .as_ref()
            .map_or(0, |g| g.baselines.len()),
    );
    let mut s = probe_load(Box::new(s), &map, fs, 0);
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
    let start = Instant::now();
    let mut last_print = start;
    let mut last = s.stats;
    let (mut events_all, mut cues_all, mut fx_all, mut frames) = (0usize, 0usize, 0usize, 0u64);
    let (mut first_time, mut last_time, mut most_ents) = (None, 0, 0);
    while !s.demo_over() {
        s.fly(&mut cam, fs);
        frames += 1;
        if let Some(why) = s.dropped.take() {
            bail!("{why}");
        }
        if let Some(m) = s.take_map_change() {
            println!("cod2: map change to {m}");
            s = probe_load(s, &m, fs, 0);
            draw = crate::cod2_scene::Cod2Draw::new();
        }
        let events = s.take_events();
        let (cues, _) = s.sound_cues(&events, fs, cam.pos, draw.muzzles());
        let plays = s.effects(&events, fs, cam.pos, draw.fx_scene(true));
        events_all += events.len();
        cues_all += cues.len();
        fx_all += plays.len();
        if let Some(f) = s.frame() {
            first_time.get_or_insert(f.time_ms);
            last_time = f.time_ms;
            most_ents = most_ents.max(f.snap.entities.len());
        }
        if last_print.elapsed() >= Duration::from_secs(1)
            && let Some(f) = s.frame()
        {
            last_print = Instant::now();
            let st = s.stats;
            let c = draw.count(&f, fs);
            println!(
                "t={:>3}s time {} snap {} msgs {} snaps {} delta {} ents {} clients {} \
                 drawn models {} bodies {} eye {:?} weapon {} parse errors {}",
                start.elapsed().as_secs(),
                f.time_ms,
                f.snap.server_time,
                st.messages - last.messages,
                st.snapshots - last.snapshots,
                st.deltas - last.deltas,
                f.snap.entities.len(),
                f.snap.clients.len(),
                c.models,
                c.players,
                f.eye(),
                f.configstrings.weapon_name(f.snap.ps.weapon).unwrap_or("-"),
                st.parse_errors,
            );
            last = st;
        }
        if !timedemo {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let st = s.stats;
    println!(
        "cod2: demo done: {} messages, {} snapshots ({} deltas), {} parse errors, \
         {} gamestates, server time {:?}..{last_time}, most entities {most_ents}, \
         {frames} frames in {:.1} s",
        st.messages,
        st.snapshots,
        st.deltas,
        st.parse_errors,
        s.gamestates(),
        first_time,
        start.elapsed().as_secs_f32(),
    );
    println!("cod2: events {events_all} sound cues {cues_all} effects started {fx_all}");
    if let Some(line) = s.timedemo_report() {
        println!("cod2: {line}");
    }
    if st.parse_errors > 0 {
        bail!("{} messages failed to parse", st.parse_errors);
    }
    if st.snapshots == 0 {
        bail!("the demo holds no snapshot");
    }
    Ok(())
}

/// `vcod --net-probe ADDR --game cod2`: spectate for `secs`, or with
/// `join` answer the script menus and play, walking forward on a slow
/// circle through usercmds; print a line a second (messages, snapshots,
/// deltas, entities, the eye, `pm_type`, parse errors, and what the scene
/// and the player poser built off the frame, CPU side:
/// `cod2_scene::Cod2Draw::count`). The map is read under
/// [`Cod2Session::pump`] as the windowed client does, and so is the map of
/// a map change; `VCOD_COD2_LOAD_STALL=<secs>` stretches the first load,
/// to check the pump against a short `sv_timeout`, and
/// `VCOD_COD2_TIMEOUT=<secs>` shortens `cl_timeout`. Fails on a parse
/// error or a drop.
pub fn probe(addr: &str, secs: u64, fs: &Pk3Fs, join: Option<Join>) -> anyhow::Result<()> {
    let name = if join.is_some() {
        "vcodplay"
    } else {
        "vcodspec"
    };
    let mut s = Cod2Session::connect(addr, &ConnectOpts::new(name, fs), Duration::from_secs(15))?;
    let reopened;
    let fs = match (s.take_reopen(fs_game_of(fs).as_deref()), home_of(fs)) {
        (true, Some(home)) => {
            reopened = open_search(&home, s.fs_game())?;
            println!(
                "cod2: search path opened again, fs_game {:?}",
                s.fs_game().unwrap_or("")
            );
            &reopened
        }
        _ => fs,
    };
    s.join = join;
    s.predict_bodies = std::env::var_os("VCOD_PROBE_NO_BODIES").is_none();
    if let Some(t) = std::env::var("VCOD_COD2_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        s.timeout = Duration::from_secs(t);
    }
    let map = s.map_name().unwrap_or("?").to_string();
    println!("cod2: gamestate for {map}, client {}", s.view.client_num);
    let stall: u64 = std::env::var("VCOD_COD2_LOAD_STALL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut s = probe_load(Box::new(s), &map, fs, stall);
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    let mut most = crate::cod2_scene::DrawCounts::default();
    let start = Instant::now();
    let mut last_print = Instant::now();
    let mut last = s.stats;
    let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
    let scripted = s.join.is_some();
    let still = std::env::var_os("VCOD_PROBE_STILL").is_some();
    let mut script = ProbeScript::default();
    let mut first_eye = None;
    let mut first_origin = None;
    let mut playing_since: Option<u64> = None;
    // The events' sounds and effects, CPU side: cues counted (no audio
    // device), effects spawned into an fx system nothing draws.
    let mut fx = crate::fx::sim::FxSystem::new();
    let mut fx_last = start;
    let (mut cues_sec, mut fx_sec, mut events_sec) = (0usize, 0usize, 0usize);
    let (mut cues_all, mut fx_all, mut events_all) = (0usize, 0usize, 0usize);
    let mut predict_all = PredictLog::default();
    // The obituaries as the HUD resolves them.
    let mut hud = crate::cod2_hud::Cod2Hud::default();
    let loc = vcod_common::localize::Localized::load(fs);
    let hud_dump = std::env::var("VCOD_COD2_HUD_DUMP").ok();
    let mut hud_at: Vec<f32> = std::env::var("VCOD_COD2_HUD_AT")
        .unwrap_or_default()
        .split(',')
        .filter_map(|t| t.trim().parse().ok())
        .collect();
    // A player asks for the board 15 s and 25 s in, as Tab does, and with
    // `VCOD_PROBE_KILL=<secs>` sends `kill` that far in.
    let mut score_asked = 0;
    let mut kill_at = std::env::var("VCOD_PROBE_KILL")
        .ok()
        .and_then(|v| v.parse::<f32>().ok());
    // `VCOD_PROBE_CMDS="<secs>:<command>|..."` sends each command that far
    // in and prints every server command that comes back.
    let mut cmds: Vec<(f32, String)> = std::env::var("VCOD_PROBE_CMDS")
        .map(|v| {
            v.split('|')
                .filter_map(|c| c.split_once(':'))
                .filter_map(|(t, c)| Some((t.trim().parse().ok()?, c.to_string())))
                .collect()
        })
        .unwrap_or_default();
    if !cmds.is_empty() {
        s.command_log = Some(Vec::new());
    }
    // `VCOD_COD2_TRACE_WEAPON=1`: a line per snapshot with the weapon's
    // state, the clip and the reserve.
    let trace_weapon = std::env::var_os("VCOD_COD2_TRACE_WEAPON").is_some();
    let mut traced_time = 0;
    // What the HUD's ammo counter reads, off the level's weapon table.
    let mut player_hud = crate::cod2_player_hud::PlayerHud::default();
    // `VCOD_COD2_RECORD=<dir>/<name>`: `record <name>` into `<dir>` once
    // the first snapshot is in, `stoprecord` as the probe ends.
    let mut record = std::env::var_os("VCOD_COD2_RECORD").map(PathBuf::from);
    let mut first_fly_ms = None;
    let mut first_count = true;
    // `VCOD_PROBE_SNAPS_AT=<secs>:<snaps>`: change the `snaps` userinfo
    // mid-game, as the console's `snaps` does.
    let mut snaps_at = std::env::var("VCOD_PROBE_SNAPS_AT").ok().and_then(|v| {
        let (t, n) = v.split_once(':')?;
        Some((t.parse::<u64>().ok()?, n.to_string()))
    });
    while start.elapsed() < Duration::from_secs(secs) {
        if let Some((_, n)) = snaps_at.take_if(|(t, _)| start.elapsed().as_secs() >= *t) {
            let info = vcod_common::net::Userinfo {
                name: s.opts.name.clone(),
                rate: s.opts.rate.clone(),
                snaps: n,
                password: s.opts.password.clone(),
                www_download: u8::from(s.opts.www_download).to_string(),
            };
            println!(
                "cod2: snaps {} at t={}s",
                info.snaps,
                start.elapsed().as_secs()
            );
            s.set_userinfo(&info);
        }
        if s.view.snap.is_some()
            && let Some(path) = record.take()
        {
            let dir = path.parent().unwrap_or(Path::new("."));
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let name = name.as_deref().map(|n| n.trim_end_matches(".dm_1"));
            println!("cod2: {}", s.start_recording(dir, name));
        }
        if still {
            // `VCOD_PROBE_STILL`: a body that stands where it spawned.
        } else if scripted {
            script.step(&mut s, &mut cam, start.elapsed().as_secs_f32());
        } else {
            s.input.key(Action::Forward, true);
            // About 30 degrees a second, so the spectator circles.
            cam.look([0.0, 0.06]);
        }
        let fly_t0 = Instant::now();
        s.fly(&mut cam, fs);
        if first_fly_ms.is_none() && s.frame().is_some() {
            let ms = fly_t0.elapsed().as_secs_f64() * 1000.0;
            first_fly_ms = Some(ms);
            println!("cod2: first frame's fly in {ms:.1} ms");
        }
        for path in s.take_fetched() {
            println!("cod2: downloaded {}", path.display());
        }
        let events = s.take_events();
        if let (Some(_), Some(f)) = (&hud_dump, s.frame()) {
            hud.on_events(&events, &f, fs, &loc, f.time_ms);
        }
        if let Some(f) = s.frame() {
            for ev in events
                .iter()
                .filter(|e| e.event == vcod_cod2::net::events::EV_OBITUARY)
            {
                match hud.obit(ev, &f, fs) {
                    Some(o) => println!(
                        "cod2: obituary {} parm {:#x}: {:?} [{}] {:?} center {:?}",
                        ev.other_entity,
                        ev.parm,
                        o.line.attacker.as_ref().map(|a| &a.0),
                        o.line.icon,
                        o.line.victim.0,
                        o.center.map(|c| loc.message(&c)),
                    ),
                    None => println!("cod2: obituary {} unresolved", ev.other_entity),
                }
            }
        }
        // `VCOD_COD2_HUD_DUMP=<prefix>` with `VCOD_COD2_HUD_AT=<secs>,...`:
        // the HUD a 1280 x 720 window would draw that far in, world sprites
        // first, as `<prefix>-<secs>.quads` for `renderer/hud_shot.rs`.
        if let (Some(prefix), Some(f)) = (&hud_dump, s.frame())
            && let Some(i) = hud_at
                .iter()
                .position(|&t| start.elapsed().as_secs_f32() >= t)
        {
            let t = hud_at.remove(i);
            draw.count(&f, fs);
            let size = (1280.0, 720.0);
            let (eye, [pitch, yaw, roll]) = s
                .view()
                .unwrap_or_else(|| (Vec3::from(f.eye()), crate::cod2_viewmodel::view_angles(&f)));
            let (yaw, pitch, roll) = (yaw.to_radians(), -pitch.to_radians(), roll.to_radians());
            let fov = crate::camera::view_fov_x(crate::camera::DEFAULT_FOV_DEG, 16.0 / 9.0, true);
            let view_proj = crate::camera::view_proj_from(eye, yaw, pitch, roll, fov, 16.0 / 9.0);
            let up = crate::camera::up_hint(yaw, pitch, roll);
            let fwd = crate::camera::basis(yaw, pitch).0;
            let right = fwd.cross(up).normalize();
            let view = crate::cod2_world_hud::WorldView {
                view_proj,
                right,
                up: right.cross(fwd),
                size,
            };
            let mut quads = crate::cod2_world_hud::build(&f, draw.heads(), &view, f.time_ms);
            hud.crosshair_hit = s.crosshair_entity();
            quads.extend(hud.build(&f, fs, &loc, size, f.time_ms, start.elapsed().as_secs_f32()));
            let path = format!("{prefix}-{t}.quads");
            std::fs::write(&path, crate::cod2_hud::dump_quads(&quads))?;
            println!(
                "cod2: hud dump {path}: {} quads, pm_type {} pm_flags {:#x} team {:?}",
                quads.len(),
                f.snap.ps.pm_type,
                f.snap.ps.pm_flags,
                f.snap
                    .clients
                    .iter()
                    .find(|c| c.number as i32 == f.snap.ps.client_num)
                    .map(|c| c.f[vcod_cod2::net::delta::field_index(
                        &vcod_cod2::net::fields::CLIENT_STATE_FIELDS,
                        "team"
                    )]),
            );
        }
        let (cues, _) = s.sound_cues(&events, fs, cam.pos, draw.muzzles());
        let plays = s.effects(&events, fs, cam.pos, draw.fx_scene(true));
        let now = start.elapsed().as_secs_f32();
        fx.set_view(cam.pos);
        for p in &plays {
            p.spawn(&mut fx, fs, f64::from(now));
        }
        fx.step(fx_last.elapsed().as_secs_f32(), f64::from(now), None);
        fx.spawn_chained(fs, f64::from(now));
        fx_last = Instant::now();
        events_sec += events.len();
        cues_sec += cues.len();
        fx_sec += plays.len();
        if let Some(why) = &s.dropped {
            bail!("dropped: {why}");
        }
        if let Some(m) = s.take_map_change() {
            println!("cod2: map change to {m}");
            s = probe_load(s, &m, fs, 0);
            draw = crate::cod2_scene::Cod2Draw::new();
        }
        if s.take_restart() {
            println!("cod2: fast_restart, serverId {}", s.client.server_id());
            fx.clear();
        }
        while cmds.first().is_some_and(|(t, _)| now >= *t) {
            let (_, c) = cmds.remove(0);
            println!("cod2: t={now:.1} sending {c:?}");
            s.send_command(&c);
        }
        if let Some(log) = s.command_log.as_mut() {
            for c in log.drain(..) {
                println!("cod2: t={now:.1} server command {c:?}");
            }
        }
        if scripted && kill_at.is_some_and(|t| now >= t) {
            s.send_command("kill");
            kill_at = None;
        }
        if scripted && score_asked < 2 && now >= [15.0, 25.0][score_asked] {
            s.send_command("score");
            score_asked += 1;
        }
        if let Some(b) = s.take_scores() {
            println!("cod2: score reply {:?}", b.join(" "));
            match crate::hud::scoreboard::parse_scores(&b) {
                Ok(m) => {
                    // Each row's name and team as the drawn board resolves it.
                    let frame = s.frame();
                    let named = |c: u32| {
                        frame
                            .as_ref()
                            .and_then(|f| crate::cod2_hud::board_client(&f.snap.clients, c))
                    };
                    println!(
                        "cod2: scores rows {} axis {} allies {} {:?}",
                        m.entries.len(),
                        m.axis,
                        m.allies,
                        m.entries
                            .iter()
                            .map(|e| (e.client, e.score, e.ping, e.deaths, named(e.client)))
                            .collect::<Vec<_>>()
                    )
                }
                Err(e) => println!("cod2: bad scores {e}: {b:?}"),
            }
        }
        if trace_weapon
            && let Some(snap) = &s.view.snap
            && snap.server_time != Msec::from_raw(traced_time)
        {
            traced_time = snap.server_time.raw();
            let ps = &snap.ps;
            let nz = |a: &[i32]| -> Vec<(usize, i32)> {
                a.iter()
                    .enumerate()
                    .filter(|e| *e.1 != 0)
                    .map(|(i, &v)| (i, v))
                    .collect()
            };
            let shown = s
                .frame()
                .and_then(|f| player_hud.counter(&f, fs, s.input.weapons().0));
            println!(
                "weap t={} weapon {} weaponstate {} weapAnim {:#x} weaponTime {} clip {:?} ammo {:?} buttons {:#x} hud {:?}",
                snap.server_time,
                ps.weapon,
                ps.raw.i32("weaponstate"),
                ps.raw.i32("weapAnim"),
                ps.raw.i32("weaponTime"),
                nz(&ps.raw.ammoclip),
                nz(&ps.raw.ammo),
                s.sent.back().map_or(0, |c| c.buttons),
                shown,
            );
        }
        if last_print.elapsed() >= Duration::from_secs(1) {
            last_print = Instant::now();
            let st = s.stats;
            let pl = s.take_predict_stats();
            println!(
                "link t={:>3}s {} | interrupted {}",
                start.elapsed().as_secs(),
                s.debug_lines().join(" | "),
                s.interrupted()
            );
            if let Some(f) = s.frame() {
                let eye = f.eye();
                first_eye.get_or_insert(eye);
                let count_t0 = Instant::now();
                let c = draw.count(&f, fs);
                if std::mem::take(&mut first_count) {
                    println!(
                        "cod2: first scene count in {:.1} ms",
                        count_t0.elapsed().as_secs_f64() * 1000.0
                    );
                }
                most.models = most.models.max(c.models);
                most.submodels = most.submodels.max(c.submodels);
                most.players = most.players.max(c.players);
                most.player_parts = most.player_parts.max(c.player_parts);
                most.guns_posed = most.guns_posed.max(c.guns_posed);
                let ps = &f.snap.ps;
                // pm_type 0: PM_NORMAL, the client's own body in the world.
                if ps.pm_type == 0 && ps.followed().is_none() {
                    playing_since.get_or_insert(start.elapsed().as_secs());
                    first_origin.get_or_insert(ps.origin);
                }
                println!(
                    "t={:>3}s serverTime {} msgs {} snaps {} delta {} cmds-sent {} ents {} clients {} \
                     drawn models {} submodels {} bodies {} body-parts {} guns {} \
                     events {} sound-cues {} effects {} fx-particles {} \
                     eye ({:.1} {:.1} {:.1}) origin ({:.1} {:.1} {:.1}) pm_type {} weapon {} ({}) \
                     follow {:?} map {} serverId {} parse-errors {} \
                     predict frames {} misses {} corrected {} over-0.1 {} max-error {:.4} \
                     selected {} pm_flags {:#x} weaponstate {} rounds {} buttons {:#x}",
                    start.elapsed().as_secs(),
                    f.snap.server_time,
                    st.messages - last.messages,
                    st.snapshots - last.snapshots,
                    st.deltas - last.deltas,
                    st.packets_sent - last.packets_sent,
                    f.snap.entities.len(),
                    f.snap.clients.len(),
                    c.models,
                    c.submodels,
                    c.players,
                    c.player_parts,
                    c.guns_posed,
                    events_sec,
                    cues_sec,
                    fx_sec,
                    fx.counts().0,
                    eye[0],
                    eye[1],
                    eye[2],
                    ps.origin[0],
                    ps.origin[1],
                    ps.origin[2],
                    ps.pm_type,
                    ps.weapon,
                    f.configstrings.weapon_name(ps.weapon).unwrap_or("-"),
                    ps.followed(),
                    f.configstrings.map_name().unwrap_or("?"),
                    s.client.server_id(),
                    st.parse_errors,
                    pl.frames,
                    pl.misses,
                    pl.corrected,
                    pl.missed,
                    pl.max_correction,
                    s.input.weapons().0,
                    ps.pm_flags,
                    ps.raw.i32("weaponstate"),
                    ps.raw.ammoclip.iter().sum::<i32>() + ps.raw.ammo.iter().sum::<i32>(),
                    s.sent.back().map_or(0, |c| c.buttons),
                );
                predict_all.frames += pl.frames;
                predict_all.misses += pl.misses;
                predict_all.missed += pl.missed;
                predict_all.corrected += pl.corrected;
                predict_all.max_correction = predict_all.max_correction.max(pl.max_correction);
            }
            last = st;
            events_all += events_sec;
            cues_all += cues_sec;
            fx_all += fx_sec;
            (events_sec, cues_sec, fx_sec) = (0, 0, 0);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if let Some((name, bytes)) = s.recording_status() {
        println!("cod2: RECORDING {name}: {}k", bytes >> 10);
        println!("cod2: {}", s.stop_recording());
    }
    s.disconnect();
    let st = s.stats;
    println!(
        "cod2: {} messages, {} snapshots ({} deltas), {} packets sent, {} parse errors, \
         {} new gamestates, {} fast_restarts, {} menu replies",
        st.messages,
        st.snapshots,
        st.deltas,
        st.packets_sent,
        st.parse_errors,
        st.new_gamestates,
        st.restarts,
        st.menu_replies
    );
    println!(
        "cod2: most drawn in a frame: models {} submodels {} bodies {} body-parts {} guns {}",
        most.models, most.submodels, most.players, most.player_parts, most.guns_posed
    );
    println!(
        "cod2: events {} sound cues {} effects started {}",
        events_all + events_sec,
        cues_all + cues_sec,
        fx_all + fx_sec
    );
    println!(
        "cod2: predicted frames {} misses {} snapshots corrected {} past 0.1 {} \
         longest correction {:.4}",
        predict_all.frames,
        predict_all.misses,
        predict_all.corrected,
        predict_all.missed,
        predict_all.max_correction
    );
    if st.parse_errors > 0 {
        bail!("{} messages failed to parse", st.parse_errors);
    }
    if st.deltas == 0 {
        bail!("no delta snapshot applied");
    }
    let moved = first_eye
        .zip(s.frame().map(|f| f.eye()))
        .is_some_and(|(a, b)| a != b);
    println!("cod2: eye moved: {moved}");
    if s.join.is_some() {
        let walked = first_origin
            .zip(s.frame().map(|f| f.snap.ps.origin))
            .map(|(a, b)| Vec3::from(a).distance(Vec3::from(b)));
        println!(
            "cod2: playing from t={:?}s, walked {:?} units",
            playing_since, walked
        );
    }
    Ok(())
}
