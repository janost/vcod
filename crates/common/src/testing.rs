//! Test helpers for every crate: built for this crate's tests and, through
//! the `testing` feature, for every crate that lists it in its
//! dev-dependencies.

use crate::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V6, install_protocol};
use crate::pk3::Pk3Fs;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Comma list of installs whose absence fails a test instead of skipping it.
pub const REQUIRE: &str = "VCOD_REQUIRE_INSTALL";

/// The names [`REQUIRE`] takes: `cod1` is `COD_DIR` set, `cod11` a 1.1
/// install there, `cod15` a 1.5 `main/` (a UO install's too), `uo` a `uo/`
/// beside its `main/`.
const INSTALLS: [&str; 7] = [
    "cod1",
    "cod11",
    "cod15",
    "uo",
    "cod2",
    "cod4",
    "cod4_lnxded",
];

/// What an install env var names.
#[derive(Debug, PartialEq)]
enum Root {
    /// Unset or empty: CI's case.
    Unset,
    Found(PathBuf),
    /// Set but unusable, and why.
    Bad(String),
}

fn resolve_root(var: &str, value: Option<OsString>, sub: &str) -> Root {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Root::Unset;
    };
    let root = PathBuf::from(value);
    if !root.is_absolute() {
        return Root::Bad(format!(
            "{var}={} is relative; tests run in their crate's directory, so set an absolute path",
            root.display()
        ));
    }
    if !root.join(sub).is_dir() {
        return Root::Bad(format!("{var}={} has no {sub}/ directory", root.display()));
    }
    Root::Found(root)
}

/// Whether `list` (the value of [`REQUIRE`]) names `game`; panics on a name
/// it does not know, so a typo cannot turn the switch off.
fn requires(list: Option<&str>, game: &str) -> bool {
    let Some(list) = list else { return false };
    let names: Vec<&str> = list
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    if let Some(bad) = names.iter().find(|n| !INSTALLS.contains(n)) {
        panic!("{REQUIRE}={list}: unknown install {bad:?}; known: {INSTALLS:?}");
    }
    names.contains(&game)
}

/// `$var`'s root, holding `sub/`. `None` when unset, so CI (no game data)
/// passes; panics when the var is set to a relative path or one without
/// `sub/`, since either would make every test that needs it pass vacuously.
pub fn install_root(var: &str, game: &str, sub: &str) -> Option<PathBuf> {
    match resolve_root(var, std::env::var_os(var), sub) {
        Root::Found(root) => Some(root),
        Root::Bad(why) => panic!("{why}"),
        Root::Unset => {
            skip(game, &format!("{var} is unset"));
            None
        }
    }
}

/// Called where a test returns early for want of `game`: panics when
/// [`REQUIRE`] names it, else says so on stderr once per `why` and binary.
pub fn skip(game: &str, why: &str) {
    if requires(std::env::var(REQUIRE).ok().as_deref(), game) {
        panic!("{why}, and {REQUIRE} names {game}");
    }
    static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    if SEEN.lock().unwrap().insert(why.to_string()) {
        let line = format!(
            "vcod testing: {why}, so the tests that need it return early ({REQUIRE} fails them)\n"
        );
        // One write to the handle: libtest swallows a passing test's eprintln!.
        let _ = std::io::stderr().write_all(line.as_bytes());
    }
}

/// `fs` opened from `dir`, or a panic naming `dir`: the var that named it
/// is set, so an open error or an empty search path is a broken install.
fn opened(dir: &Path, fs: anyhow::Result<Pk3Fs>) -> Pk3Fs {
    let fs = fs.unwrap_or_else(|e| panic!("{}: {e:#}", dir.display()));
    assert!(fs.paks().next().is_some(), "{}: no paks", dir.display());
    fs
}

/// `$COD_DIR` once checked as [`install_root`] does, else the test binary's
/// directory, which never holds the game.
pub fn game_dir() -> PathBuf {
    install_root("COD_DIR", "cod1", "main").unwrap_or_else(crate::game_dir::default_game_dir)
}

/// `None` without `COD_DIR`; callers return early so the run stays green.
pub fn game_fs() -> Option<Pk3Fs> {
    let dir = install_root("COD_DIR", "cod1", "main")?.join("main");
    Some(opened(&dir, Pk3Fs::open(&dir)))
}

/// A CoD2 install's stock `main/` iwds ([`Pk3Fs::open_iwd_stock`]), from
/// `$COD2_DIR`; `None` without it, so CI (no game data) passes vacuously.
/// The CoD2 tests assert stock 1.3 (its gametype and map scripts, retail's
/// `sv_iwdNames`), which a map pack's iwd in `main/` would override.
pub fn cod2_fs() -> Option<Pk3Fs> {
    let dir = install_root("COD2_DIR", "cod2", "main")?.join("main");
    Some(opened(&dir, Pk3Fs::open_iwd_stock(&dir)))
}

/// `fs` when it is 1.1's install, else `None` (UO's `main/` is 1.5's,
/// `install_protocol`).
fn only_11(fs: Option<Pk3Fs>) -> Option<Pk3Fs> {
    only(fs, &PROTOCOL_V1, "cod11", "COD_DIR is not a 1.1 install")
}

/// `fs` when its paks are `p`'s version's, else a skip under `name`.
fn only(fs: Option<Pk3Fs>, p: &Cod1Wire, name: &str, other: &str) -> Option<Pk3Fs> {
    match fs {
        Some(fs) if install_protocol(&fs).transport.version == p.transport.version => Some(fs),
        Some(_) => {
            skip(name, other);
            None
        }
        None => {
            skip(name, "COD_DIR is unset");
            None
        }
    }
}

/// [`game_fs`] when the install is 1.1's, `None` at a 1.5 or UO install:
/// tests that assert 1.1's assets or replay a 1.1 retail capture take their
/// paks from here.
pub fn game_fs_11() -> Option<Pk3Fs> {
    only_11(game_fs())
}

/// [`game_fs`] when its `main/` is 1.5's, a UO install's included, `None`
/// at a 1.1 install: the 1.5 gates take their paks from here.
pub fn game_fs_15() -> Option<Pk3Fs> {
    only(
        game_fs(),
        &PROTOCOL_V6,
        "cod15",
        "COD_DIR is not a 1.5 install",
    )
}

/// [`game_fs`] with the install's stock paks alone (`--vanilla`,
/// [`Pk3Fs::open_search`]): tests that assert the stock assets take it, so
/// a third-party pak in `main/` cannot move them.
pub fn stock_fs() -> Option<Pk3Fs> {
    let dir = install_root("COD_DIR", "cod1", "main")?.join("main");
    Some(opened(&dir, Pk3Fs::open_search(&dir, None, None, true)))
}

/// [`stock_fs`] when the install is 1.1's, as [`game_fs_11`].
pub fn stock_fs_11() -> Option<Pk3Fs> {
    only_11(stock_fs())
}

/// United Offensive's `uo/` over its sibling `main/`, `None` unless the
/// install is UO's: the UO gates run only with `COD_DIR` at one.
pub fn uo_fs() -> Option<Pk3Fs> {
    let Some(root) = install_root("COD_DIR", "cod1", "main") else {
        skip("uo", "COD_DIR is unset");
        return None;
    };
    let dir = root.join("uo");
    if !dir.is_dir() {
        skip("uo", "COD_DIR has no uo/");
        return None;
    }
    Some(opened(&dir, Pk3Fs::open(&dir)))
}

/// `path` off `fs`, a file every install of the game ships (a stock map):
/// a test that has its install panics when the lookup misses rather than
/// passing with nothing checked.
pub fn stock_file(fs: &Pk3Fs, path: &str) -> Vec<u8> {
    fs.read(path)
        .unwrap_or_else(|| panic!("{path}: every install ships it, and the lookup missed"))
}

/// `maps/mp/mp_pavlov.bsp` from the retail paks.
pub fn real_bsp() -> Option<Vec<u8>> {
    game_fs()?.read("maps/mp/mp_pavlov.bsp")
}

/// An IBSP59 map with `"<key>" "<value>"` added to its worldspawn, as
/// `tools/bsp_worldspawn_key.py` builds it for a retail run: the grown
/// entity lump (29) goes at the end of the file and the directory points
/// there.
pub fn bsp_with_worldspawn_key(bsp: &[u8], key: &str, value: &str) -> Vec<u8> {
    let dir = 8 + 29 * 8;
    let word = |o: usize| u32::from_le_bytes(bsp[o..o + 4].try_into().unwrap()) as usize;
    let (len, off) = (word(dir), word(dir + 4));
    let ents = &bsp[off..off + len];
    let ents = &ents[..ents.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1)];
    let tag = b"\"classname\" \"worldspawn\"";
    let at = ents
        .windows(tag.len())
        .position(|w| w == tag)
        .expect("a worldspawn")
        + tag.len();
    let mut lump = ents[..at].to_vec();
    lump.extend(format!("\n\"{key}\" \"{value}\"").bytes());
    lump.extend(&ents[at..]);
    lump.push(0);
    let mut out = bsp.to_vec();
    out.resize(out.len().next_multiple_of(4), 0);
    let start = out.len();
    out.extend(&lump);
    out[dir..dir + 4].copy_from_slice(&(lump.len() as u32).to_le_bytes());
    out[dir + 4..dir + 8].copy_from_slice(&(start as u32).to_le_bytes());
    out
}

/// An IBSP59 map with `blocks` appended to its entity lump, as
/// `tools/bsp_append_entities.py` builds it for a retail run: the grown
/// lump (29) goes at the end of the file and the directory points there.
pub fn bsp_with_entities(bsp: &[u8], blocks: &str) -> Vec<u8> {
    let dir = 8 + 29 * 8;
    let word = |o: usize| u32::from_le_bytes(bsp[o..o + 4].try_into().unwrap()) as usize;
    let (len, off) = (word(dir), word(dir + 4));
    let ents = &bsp[off..off + len];
    let mut lump = ents[..ents.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1)].to_vec();
    lump.extend(blocks.bytes());
    lump.push(0);
    let mut out = bsp.to_vec();
    out.resize(out.len().next_multiple_of(4), 0);
    let start = out.len();
    out.extend(&lump);
    out[dir..dir + 4].copy_from_slice(&(lump.len() as u32).to_le_bytes());
    out[dir + 4..dir + 8].copy_from_slice(&(start as u32).to_le_bytes());
    out
}

/// [`bsp_with_entities`] after dropping every stock block whose `"model"
/// "*N"` one of `blocks` also names, the blocks
/// `tools/bsp_append_entities.py --replace` keeps: a stock submodel under a
/// new classname.
pub fn bsp_replacing_models(bsp: &[u8], blocks: &str) -> Vec<u8> {
    let models = |text: &str| -> Vec<String> {
        crate::bsp::entity_block_pairs(text)
            .iter()
            .filter_map(|b| crate::bsp::field_value(b, "model").map(str::to_string))
            .filter(|m| m.starts_with('*'))
            .collect()
    };
    let taken = models(blocks);
    let dir = 8 + 29 * 8;
    let word = |o: usize| u32::from_le_bytes(bsp[o..o + 4].try_into().unwrap()) as usize;
    let (len, off) = (word(dir), word(dir + 4));
    let ents = String::from_utf8_lossy(&bsp[off..off + len]);
    let ents = ents.trim_end_matches('\0');
    // The tool's `\{[^{}]*\}\s*`: a block and the whitespace after it.
    let mut kept = String::new();
    let mut rest = ents;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|c| open + c + 1) else {
            break;
        };
        let end = close + (rest[close..].len() - rest[close..].trim_start().len());
        kept.push_str(&rest[..open]);
        if !models(&rest[open..close]).iter().any(|m| taken.contains(m)) {
            kept.push_str(&rest[open..end]);
        }
        rest = &rest[end..];
    }
    kept.push_str(rest);
    // An empty stock lump, so `bsp_with_entities` writes exactly `kept`.
    let mut out = bsp.to_vec();
    out[dir..dir + 4].copy_from_slice(&0u32.to_le_bytes());
    bsp_with_entities(&out, &(kept + blocks))
}

/// A committed capture under this crate's `tests/fixtures/`, by relative path.
pub fn fixture(rel: &str) -> Vec<u8> {
    let path = PathBuf::from(crate::manifest_dir!())
        .join("tests/fixtures")
        .join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}

/// A committed gamestate capture (`--save-fixture`), decoded.
pub fn fixture_gamestate(
    p: &crate::net::protocol::Cod1Wire,
    gamestate: &str,
) -> crate::net::gamestate::Gamestate {
    use crate::net::huffman::Huffman;
    use crate::net::msg::MsgReader;
    let gs_data = fixture(gamestate);
    crate::net::gamestate::parse(&mut MsgReader::new(&gs_data[4..], &Huffman::new()), p)
        .unwrap_or_else(|e| panic!("{gamestate}: {e:?}"))
}

/// Every frame of a committed snapshot capture (the `--save-snapshots`
/// format: message number, length, message), decoded against its gamestate
/// capture, with the server commands between them.
pub fn fixture_frames(
    p: &crate::net::protocol::Cod1Wire,
    gamestate: &str,
    snapshots: &str,
) -> Vec<Frame> {
    use crate::net::huffman::Huffman;
    use crate::net::msg::MsgReader;
    use crate::net::snapshot::{SVC_EOF, SVC_NOP, SVC_SERVER_COMMAND, SVC_SNAPSHOT, SnapshotRing};
    let h = Huffman::new();
    let gs = fixture_gamestate(p, gamestate);
    let mut ring = SnapshotRing::new();
    ring.set_baselines(gs.baselines.clone());
    let data = fixture(snapshots);
    let mut out = Vec::new();
    let mut commands = Vec::new();
    let mut off = 0usize;
    while off + 8 <= data.len() {
        let message_num = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        let len = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap()) as usize;
        off += 8;
        let mut r = MsgReader::new(&data[off + 4..off + len], &h);
        off += len;
        loop {
            match r.read_byte() {
                SVC_EOF => break,
                SVC_NOP => {}
                SVC_SERVER_COMMAND => {
                    r.read_long();
                    commands.push(r.read_big_string());
                }
                SVC_SNAPSHOT => {
                    let snap = ring.parse_into(&mut r, p, message_num).unwrap().clone();
                    out.push(Frame {
                        commands: std::mem::take(&mut commands),
                        snapshot: snap,
                    });
                }
                op => panic!("{snapshots}: unhandled svc op {op}"),
            }
            assert!(
                !r.is_overflowed(),
                "{snapshots}: message {message_num} overran"
            );
        }
    }
    out
}

/// One frame of [`fixture_frames`]: the server commands that arrived since
/// the last one, then the snapshot.
pub struct Frame {
    pub commands: Vec<String>,
    pub snapshot: crate::net::snapshot::Snapshot,
}

/// One line of a `--save-slope` capture (the client probe's writer): a
/// usercmd as it went on the wire, or the playerstate a snapshot carried.
pub enum SlopeLine {
    Cmd(crate::net::msg::UserCmd),
    Snap(Box<SlopeSnap>),
}

/// A `--save-slope` `!snap` line.
pub struct SlopeSnap {
    /// `t`, the snapshot's serverTime.
    pub server_time: i32,
    /// `msg`, its message number.
    pub message_num: u32,
    /// `wire`, `ammo` and `clip` over a null playerstate.
    pub ps: crate::net::msg::PlayerState,
    /// `vang`, the seat's vehicle's angles, on a seated UO capture.
    pub vang: Option<[f32; 3]>,
    /// `ev`, the `(event, parm)` pairs the snapshot brought, in 1.1's ids.
    pub events: Vec<(i32, i32)>,
}

/// Every `!cmd` and `!snap` line of a `--save-slope` capture, in order.
pub fn parse_slope(p: &crate::net::protocol::Cod1Wire, text: &str) -> Vec<SlopeLine> {
    use crate::net::msg::{NULL_USERCMD, PlayerState, UserCmd};
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((kind, rest)) = line.split_once(' ') else {
            continue;
        };
        let kv: std::collections::BTreeMap<&str, &str> = rest
            .split_whitespace()
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let int = |k: &str| -> i32 { kv[k].parse().unwrap_or_else(|_| panic!("{k} in {line}")) };
        let pairs = |k: &str| -> Vec<(&str, i32)> {
            kv.get(k)
                .into_iter()
                .flat_map(|v| v.split(','))
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let (a, b) = s.rsplit_once(':').expect("name:value");
                    (a, b.parse().expect("an int"))
                })
                .collect()
        };
        match kind {
            "!cmd" => {
                let a: Vec<i32> = kv["angles"]
                    .split(',')
                    .map(|x| x.parse().unwrap())
                    .collect();
                out.push(SlopeLine::Cmd(UserCmd {
                    server_time: int("st"),
                    buttons: int("buttons") as u8,
                    wbuttons: int("wbuttons") as u8,
                    weapon: int("weapon") as u8,
                    angles: [a[0], a[1], a[2]],
                    forward: int("forward") as i8,
                    right: int("right") as i8,
                    up: int("up") as i8,
                    ..NULL_USERCMD
                }));
            }
            "!snap" => {
                let mut ps = PlayerState::null(p);
                for (name, v) in pairs("wire") {
                    ps.fields[PlayerState::field_index(p, name).expect(name)] = v;
                }
                for (k, v) in pairs("ammo") {
                    ps.arrays.ammo[k.parse::<usize>().unwrap()] = v as i16;
                }
                for (k, v) in pairs("clip") {
                    ps.arrays.ammoclip[k.parse::<usize>().unwrap()] = v as i16;
                }
                let vang = kv.get("vang").map(|v| {
                    let a: Vec<f32> = v.split(',').map(|x| x.parse().unwrap()).collect();
                    [a[0], a[1], a[2]]
                });
                let events = pairs("ev")
                    .into_iter()
                    .map(|(e, parm)| (e.parse().expect("an event id"), parm))
                    .collect();
                out.push(SlopeLine::Snap(Box::new(SlopeSnap {
                    server_time: int("t"),
                    message_num: int("msg") as u32,
                    ps,
                    vang,
                    events,
                })));
            }
            _ => {}
        }
    }
    out
}

/// Runs `parse` over `rounds` seeded mutations of `good` (bit flips, edge
/// bytes and words, truncation, splices; [`mutate`]) and fails naming the
/// round whose input panicked. A hostile-input test asserts only that the
/// parser returns, so it needs no expected values and runs with no install.
pub fn fuzz(name: &str, good: &[u8], rounds: u32, mut parse: impl FnMut(&[u8])) {
    let mut seed = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| {
        (h ^ c as u64).wrapping_mul(0x0100_0000_01b3)
    });
    for round in 0..rounds {
        let input = mutate(good, &mut seed);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse(&input)));
        if let Err(e) = r {
            let why = e
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| e.downcast_ref::<&str>().copied())
                .unwrap_or("?");
            panic!(
                "{name}: round {round} panicked: {why}\ninput ({} bytes): {:02x?}",
                input.len(),
                &input[..input.len().min(256)]
            );
        }
    }
}

/// One to four random edits of `good`, drawn from `seed`.
pub fn mutate(good: &[u8], seed: &mut u64) -> Vec<u8> {
    use crate::rng::xorshift;
    const BYTES: [u8; 6] = [0, 1, 0x7f, 0x80, 0xfe, 0xff];
    const WORDS: [u32; 6] = [0, 1, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 0x0001_0000];
    let mut b = good.to_vec();
    let mut pick = |n: usize| (xorshift(seed) % n.max(1) as u64) as usize;
    for _ in 0..1 + pick(4) {
        let at = pick(b.len());
        match pick(7) {
            _ if b.is_empty() => b.push(BYTES[pick(BYTES.len())]),
            0 | 1 => b[at] ^= 1 << pick(8),
            2 => b[at] = BYTES[pick(BYTES.len())],
            3 => {
                let w = WORDS[pick(WORDS.len())].to_le_bytes();
                let end = (at + 4).min(b.len());
                b[at..end].copy_from_slice(&w[..end - at]);
            }
            4 => b.truncate(at),
            5 => {
                let len = pick(64).min(b.len() - at);
                let piece = b[at..at + len].to_vec();
                let to = pick(b.len());
                b.splice(to..to, piece);
            }
            _ => {
                let len = pick(b.len() - at + 1);
                b.drain(at..at + len);
            }
        }
    }
    b
}

/// A one-entry stored pk3 (`maps/mp/foo.bsp`), for tests that need a
/// transfer to pass validation.
pub fn pk3_bytes() -> Vec<u8> {
    pk3_with(&[("maps/mp/foo.bsp", b"IBSP")])
}

/// A stored pk3 holding `entries`, in order.
pub fn pk3_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_var_resolves_only_to_an_absolute_root_holding_its_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("main")).unwrap();
        let at = |v: &str| resolve_root("COD_DIR", Some(v.into()), "main");
        assert_eq!(resolve_root("COD_DIR", None, "main"), Root::Unset);
        assert_eq!(at(""), Root::Unset);
        assert_eq!(
            at(tmp.path().to_str().unwrap()),
            Root::Found(tmp.path().into())
        );
        let bad = |v: &str| match at(v) {
            Root::Bad(why) => why,
            r => panic!("{v}: {r:?}"),
        };
        assert!(bad("Games/CoD11").starts_with("COD_DIR=Games/CoD11 is relative"));
        let missing = tmp.path().join("nope");
        assert!(bad(missing.to_str().unwrap()).ends_with("has no main/ directory"));
        let zone = resolve_root("COD4_DIR", Some(tmp.path().into()), "zone");
        assert!(matches!(zone, Root::Bad(why) if why.starts_with("COD4_DIR=")));
    }

    #[test]
    fn the_require_switch_names_installs_and_refuses_a_typo() {
        assert!(!requires(None, "cod1"));
        assert!(requires(Some("cod1, uo"), "uo"));
        assert!(!requires(Some("cod1,uo"), "cod2"));
        assert!(!requires(Some(""), "cod1"));
        let typo = std::panic::catch_unwind(|| requires(Some("cod1,cod 2"), "cod1"));
        assert!(typo.is_err());
    }
}
