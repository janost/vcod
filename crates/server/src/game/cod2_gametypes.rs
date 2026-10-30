//! CoD2's five stock gametypes on the stock maps through `Cod2Level`
//! (cod2-map-cycle.md 2): each loads, two clients join allies and axis,
//! answer the stock menus, spawn on one of the gametype's spawn points and
//! play on with no script error and no builtin unanswered. Needs
//! `COD2_DIR`; without it every test passes.

use std::rc::Rc;

use crate::game::cod2::Cod2Level;
use crate::game::script::MenuRef;
use vcod_common::pk3::Pk3Fs;

pub(crate) const GAMETYPES: [&str; 5] = ["dm", "tdm", "sd", "ctf", "hq"];

/// The spawn classnames `gametype`'s `Callback_StartGameType` reads.
fn spawn_classes(gametype: &str) -> &'static [&'static str] {
    match gametype {
        "dm" => &["mp_dm_spawn"],
        "tdm" | "hq" => &["mp_tdm_spawn"],
        "sd" => &["mp_sd_spawn_attacker", "mp_sd_spawn_defender"],
        "ctf" => &["mp_ctf_spawn_allied", "mp_ctf_spawn_axis"],
        _ => &[],
    }
}

/// The stock maps, sorted.
pub(crate) fn stock_maps(fs: &Pk3Fs) -> Vec<String> {
    let mut maps: Vec<String> = fs
        .names_with_suffix(".d3dbsp")
        .into_iter()
        .filter_map(|p| {
            Some(
                p.strip_prefix("maps/mp/")?
                    .strip_suffix(".d3dbsp")?
                    .to_string(),
            )
        })
        .collect();
    maps.sort();
    maps
}

/// The menu a stock weapon menu's first rifle answers.
fn rifle(menu: &str) -> &'static str {
    match menu {
        "weapon_american" => "m1garand_mp",
        "weapon_british" => "enfield_mp",
        "weapon_russian" => "mosin_nagant_mp",
        _ => "kar98k_mp",
    }
}

/// A level and its clock, driven one 50 ms frame at a time with a cmd per
/// joined client per frame.
pub(crate) struct Drive {
    pub level: Cod2Level,
    pub now: i32,
    pub slots: usize,
    /// Each slot's forward move.
    pub forward: Vec<i8>,
}

impl Drive {
    pub fn load(fs: Rc<Pk3Fs>, map: &str, gametype: &str, set: &[(&str, &str)]) -> Drive {
        // `developer 0`, the server's default: a runtime error ends only
        // its instruction (still listed in `aborts`), as retail's does.
        let mut cvars = crate::cvars::Cvars::new();
        for (k, v) in set {
            cvars.set(k, v);
        }
        let level = Cod2Level::load(fs, map, gametype, cvars, 0)
            .unwrap_or_else(|e| panic!("{map} {gametype}: {e:#}"));
        Drive {
            level,
            now: 0,
            slots: 0,
            forward: Vec::new(),
        }
    }

    pub fn frame(&mut self) {
        self.now += 50;
        for slot in 0..self.slots {
            let cmd = vcod_cod2::net::UserCmd2 {
                server_time: self.now,
                forward_move: self.forward[slot],
                angles: [0, (self.now * 8 + slot as i32 * 20000) & 0xffff, 0],
                ..Default::default()
            };
            self.level.client_think(slot, &cmd);
        }
        self.level.run_frame(self.now);
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    /// The menus the level precached (configstrings from 1246).
    fn menus(&self) -> Vec<String> {
        let cs = self.level.rt.configstrings();
        (1246..1246 + 32)
            .map(|i| cs[i].clone())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Two clients connect, begin, pick `allies` and `axis` off the team
    /// menu and the first rifle off their weapon menu.
    pub fn join_two(&mut self) {
        self.slots = 2;
        self.forward = vec![0; 2];
        self.level.connect(0, "a");
        self.level.connect(1, "b");
        self.frame();
        self.level.begin(0);
        self.level.begin(1);
        self.frame();
        let menus = self.menus();
        let team = menus
            .iter()
            .find(|m| m.starts_with("team_"))
            .cloned()
            .expect("a team menu");
        for (slot, t) in ["allies", "axis"].into_iter().enumerate() {
            self.level
                .menu_response(slot, &MenuRef::Name(team.clone()), t);
        }
        self.frames(4);
        let weapon_menus: Vec<&String> =
            menus.iter().filter(|m| m.starts_with("weapon_")).collect();
        for slot in 0..2 {
            let menu = weapon_menus.get(slot).copied().cloned().unwrap_or_default();
            self.level
                .menu_response(slot, &MenuRef::Name(menu.clone()), rifle(&menu));
        }
        self.frames(4);
    }

    /// The script errors the level raised.
    pub fn aborts(&self) -> Vec<String> {
        self.level.rt.aborts()
    }

    pub fn session(&mut self, slot: usize) -> String {
        self.level
            .rt
            .client_field(slot, "sessionstate")
            .unwrap_or_default()
    }
}

/// One gametype on one map: `Err` names what failed.
fn census_one(fs: &Rc<Pk3Fs>, map: &str, gametype: &str, secs: usize) -> Result<(), String> {
    let bytes = fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap();
    let world = vcod_cod2::world::from_bytes(&bytes).expect("parse map");
    let blocks = vcod_common::bsp::entity_blocks(&world.entities);
    let spots: Vec<[f32; 3]> = blocks
        .iter()
        .filter(|b| {
            b.get("classname")
                .is_some_and(|c| spawn_classes(gametype).contains(&c.as_str()))
        })
        .filter_map(|b| vcod_common::bsp::parse_vec3(b.get("origin")?))
        .collect();
    let mut d = Drive::load(fs.clone(), map, gametype, &[]);
    let left = d.level.rt.unimplemented_builtins();
    if !left.is_empty() {
        return Err(format!("unanswered {left:?}"));
    }
    d.join_two();
    let mut errs = Vec::new();
    for slot in 0..2 {
        if d.session(slot) != "playing" {
            errs.push(format!("slot {slot} {}", d.session(slot)));
            continue;
        }
        let o = d.level.players[slot].as_ref().unwrap().ps.origin;
        // `PlaceSpawnpoint` drops the spot to the ground: xy match only.
        if !spots
            .iter()
            .any(|s| (s[0] - o.x).abs() < 1.0 && (s[1] - o.y).abs() < 1.0)
        {
            errs.push(format!("slot {slot} spawned at {o} on no spawn point"));
        }
    }
    d.forward = vec![127; 2];
    d.frames(secs * 20);
    let aborts = d.aborts();
    if !aborts.is_empty() {
        errs.push(aborts.join("\n"));
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

/// Every stock gametype on mp_toujane and mp_breakout (a map with sound
/// effects): 20 s with two clients.
#[test]
fn cod2_every_gametype_plays_on_two_maps() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let failures: Vec<String> = ["mp_toujane", "mp_breakout"]
        .iter()
        .flat_map(|map| GAMETYPES.iter().map(move |gt| (map, gt)))
        .filter_map(|(map, gt)| {
            census_one(&fs, map, gt, 20)
                .err()
                .map(|e| format!("{map} {gt}: {e}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The whole census: every stock gametype on every stock map, 10 s each
/// with two clients (about 80 loads; run in release).
#[test]
#[ignore]
fn cod2_gametype_census() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let mut failures = Vec::new();
    for map in stock_maps(&fs) {
        for gt in GAMETYPES {
            let r = census_one(&fs, &map, gt, 10);
            eprintln!("{map} {gt}: {r:?}");
            if let Err(e) = r {
                failures.push(format!("{map} {gt}: {e}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
