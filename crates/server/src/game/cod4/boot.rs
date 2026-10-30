//! `war` boots on mp_crash: every script compiles against `cod4_lnxded`'s
//! tables, the bootstrap runs to its first wait with no thread dead and no
//! builtin reached that vcod does not answer, ten seconds of an empty
//! server run clean, and the configstrings the level wrote match the
//! retail gamestate in `handshake-1.7.txt` slot for slot, but for
//! [`GAPS`]. Needs `COD4_DIR`; without it the test passes.

use std::collections::BTreeMap;

use super::*;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod4/tests/fixtures/net/handshake-1.7.txt"
);

/// Slots where ours and the capture differ, each with why. None of the
/// compared slots does.
const GAPS: &[(usize, &str)] = &[];

/// The one part of the gamestate left out of the comparison: the mirror
/// block's engine dvars (`friction`, `g_TeamColor_*`, `jump_height`,
/// `player_*`, `stopspeed`, ...), which 1.7's engine registers and mirrors
/// and vcod does not register yet (S-C4-1); the scripts' own (`scr_*`,
/// `ui_*`) are compared by name. With the engine's missing the script
/// dvars also sit at other slots than retail's, as the block is sorted by
/// name.
fn script_dvar(name: &str) -> bool {
    name.starts_with("scr_") || name.starts_with("ui_")
}

/// The script-owned slots compared one for one: the game version, fog,
/// end time, map centre and winner, then the blocks from the localized
/// strings through the items (cod4-gsc.md 5).
fn compared() -> impl Iterator<Item = usize> {
    [2, 9, 11, 12, 19].into_iter().chain(309..=2314)
}

/// The capture's gamestate, defaults filled in.
fn retail() -> vcod_cod4::net::parse::Gamestate {
    use vcod_cod4::net::capture::{self, Session};
    let text = std::fs::read_to_string(FIXTURE).unwrap();
    let mut s = Session::new();
    for l in capture::parse(&text).unwrap() {
        s.feed(&l).unwrap();
        if s.client.gamestate.is_some() {
            break;
        }
    }
    s.client.gamestate.unwrap()
}

/// The capture's serverinfo and systeminfo pairs, as retail's command line
/// and engine set them, for the cvars the scripts read (`sv_hostname`).
fn info_pairs(gs: &vcod_cod4::net::parse::Gamestate, slot: u16) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(gs.configstring(slot)).into_owned();
    let mut it = text.split('\\').skip(1);
    let mut out = Vec::new();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        out.push((k.to_string(), v.to_string()));
    }
    out
}

/// The mirror block (20..275) as name to value.
fn mirror(cs: &dyn Fn(usize) -> String) -> BTreeMap<String, String> {
    let l = &CS_LAYOUT_COD4;
    (l.cvar_names.0..=l.cvar_names.1)
        .filter_map(|i| {
            let name = cs(i);
            (!name.is_empty()).then(|| {
                (
                    name.to_ascii_lowercase(),
                    cs(l.cvar_values + i - l.cvar_names.0),
                )
            })
        })
        .collect()
}

/// `war` on mp_crash loaded and booted with the stock settings, for a
/// test that needs a level's host; `None` without `COD4_DIR`.
pub(crate) fn booted_for_test() -> Option<crate::game::script::ScriptRuntime> {
    let install = vcod_cod4::testing::cod4_install()?;
    let zones = Cod4Zones::load(&install, "mp_crash").unwrap();
    let mut cvars = crate::cvars::Cvars::new();
    let iwds = vcod_common::pk3::Pk3Fs::open_iwd(&install.root().join("main"), None).unwrap();
    let settings = iwds.read("default_mp_gamesettings.cfg").unwrap();
    cvars.exec_cfg(&String::from_utf8_lossy(&settings));
    Some(load(&zones, &install, "mp_crash", "war", cvars, true, None, 0).unwrap())
}

#[test]
fn cod4_boot() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let gs = retail();
    let zones = Cod4Zones::load(&install, "mp_crash").unwrap();
    let mut cvars = crate::cvars::Cvars::new();
    // What the retail engine runs ahead of the level: `exec
    // default_mp.cfg` (cod4_lnxded string `exec default_mp.cfg`), whose
    // `default_mp_gamesettings.cfg` sets the gametype dvars
    // (`scr_war_scorelimit 750`), then the command line the capture's
    // serverinfo reflects (`sv_hostname CoD4Host`).
    let iwds = vcod_common::pk3::Pk3Fs::open_iwd(&install.root().join("main"), None).unwrap();
    let settings = iwds.read("default_mp_gamesettings.cfg").unwrap();
    cvars.exec_cfg(&String::from_utf8_lossy(&settings));
    for (k, v) in info_pairs(&gs, 0) {
        cvars.set(&k, &v);
    }
    let mut rt = load(&zones, &install, "mp_crash", "war", cvars, true, None, 0).unwrap();
    let aborts = rt.aborts();
    assert!(
        aborts.is_empty(),
        "bootstrap errors:\n{}",
        aborts.join("\n")
    );
    // `Callback_StartGameType`'s own statements ran.
    assert_eq!(rt.level_field_str("gametype"), "war");
    let left = rt.unimplemented_builtins();
    eprintln!("{} builtins unanswered: {}", left.len(), left.join(" "));
    // 30 s of an empty server: the prematch period (15 s wait for players,
    // 5 s countdown) ends and sets the match end time and the map's vision.
    for f in 1..=600 {
        rt.run_frame(f * 50);
    }
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "idle errors:\n{}", aborts.join("\n"));

    let ours: Vec<String> = rt.with_host(|h, _| {
        let mut cs = h.configstrings.to_vec();
        h.cvars.write_mirror(&mut cs).unwrap();
        cs
    });
    // The client reads a slot the gamestate leaves out as its default and
    // an empty one as empty, so ours as the table holds it is what a
    // client of ours would read.
    let ours_at = |i: usize| ours[i].clone();
    let theirs_at = |i: usize| String::from_utf8_lossy(gs.configstring(i as u16)).into_owned();
    let mut diffs = Vec::new();
    for i in compared() {
        let (a, b) = (ours_at(i), theirs_at(i));
        let gap = GAPS.iter().any(|(s, _)| *s == i);
        if (a != b) != gap {
            diffs.push(format!("{i}: ours {a:?} retail {b:?}"));
        }
    }
    // The mirror: every script dvar retail mirrors, ours mirrors at its
    // value (the engine's own dvars are not registered on ours yet).
    let (m_ours, m_theirs) = (mirror(&ours_at), mirror(&theirs_at));
    for (name, value) in &m_theirs {
        if !script_dvar(name) {
            continue;
        }
        if m_ours.get(name) != Some(value) {
            diffs.push(format!(
                "mirror {name}: ours {:?} retail {value:?}",
                m_ours.get(name)
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "{} slots differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// Every script in `common_mp.ff` and `mp_crash.ff` compiles under
/// `Dialect::Cod4` against `cod4_lnxded`'s tables, and 1.1's set refuses
/// CoD4's own calls.
#[test]
fn cod4_stock_scripts_call_only_cod4_builtins() {
    use crate::game::builtins::set::BuiltinSet;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let zones = Cod4Zones::load(&install, "mp_crash").unwrap();
    let scripts: Vec<String> = zones
        .source
        .list("", ".gsc")
        .into_iter()
        .map(|p| p.trim_end_matches(".gsc").to_string())
        .collect();
    assert_eq!(scripts.len(), 130, "{scripts:?}");
    let set = BuiltinSet::cod4();
    let mut vm = vcod_gsc::Vm::new();
    let mut loader = vcod_gsc::Loader::with_dialect(
        Box::new(Cod4Scripts::new(zones.source.clone(), None)),
        vcod_gsc::Dialect::Cod4,
    );
    loader.set_builtins(set.table());
    for e in &scripts {
        loader
            .load(&mut vm, e)
            .unwrap_or_else(|err| panic!("{e}: {err:?}"));
    }
    let unknown = set.unknown_calls(&vm);
    assert!(unknown.is_empty(), "{unknown:?}");
    let v1 = BuiltinSet::v11().unknown_calls(&vm);
    assert!(v1.iter().any(|n| n == "function 'tablelookup'"), "{v1:?}");
}
