//! Test helpers for every crate. Always compiled because the client crate's
//! tests call into it.

use crate::net::protocol::{PROTOCOL_V1, install_protocol};
use crate::pk3::Pk3Fs;
use std::path::PathBuf;

/// `$COD_DIR`, else the test binary's directory, which never holds the game.
pub fn game_dir() -> PathBuf {
    crate::game_dir::default_game_dir()
}

/// `None` when the game is not installed; callers return early so the run
/// stays green.
pub fn game_fs() -> Option<Pk3Fs> {
    let dir = game_dir().join("main");
    if !dir.is_dir() {
        return None;
    }
    Pk3Fs::open(&dir).ok()
}

/// A CoD2 install's stock `main/` iwds ([`Pk3Fs::open_iwd_stock`]), from
/// `$COD2_DIR`; `None` without it, so CI (no game data) passes vacuously.
/// The CoD2 tests assert stock 1.3 (its gametype and map scripts, retail's
/// `sv_iwdNames`), which a map pack's iwd in `main/` would override.
pub fn cod2_fs() -> Option<Pk3Fs> {
    let dir = PathBuf::from(std::env::var_os("COD2_DIR")?).join("main");
    if !dir.is_dir() {
        return None;
    }
    Pk3Fs::open_iwd_stock(&dir).ok()
}

/// [`game_fs`] when the install is 1.1's, `None` at a 1.5 or UO install
/// (UO's `main/` is 1.5's, `install_protocol`): tests that assert 1.1's
/// assets or replay a 1.1 retail capture take their paks from here.
pub fn game_fs_11() -> Option<Pk3Fs> {
    let fs = game_fs()?;
    (install_protocol(&fs).transport.version == PROTOCOL_V1.transport.version).then_some(fs)
}

/// [`game_fs`] with the install's stock paks alone (`--vanilla`,
/// [`Pk3Fs::open_search`]): tests that assert the stock assets take it, so
/// a third-party pak in `main/` cannot move them.
pub fn stock_fs() -> Option<Pk3Fs> {
    let dir = game_dir().join("main");
    if !dir.is_dir() {
        return None;
    }
    Pk3Fs::open_search(&dir, None, None, true).ok()
}

/// [`stock_fs`] when the install is 1.1's, as [`game_fs_11`].
pub fn stock_fs_11() -> Option<Pk3Fs> {
    let fs = stock_fs()?;
    (install_protocol(&fs).transport.version == PROTOCOL_V1.transport.version).then_some(fs)
}

/// United Offensive's `uo/` over its sibling `main/`, `None` unless the
/// install is UO's: the UO gates run only with `COD_DIR` at one.
pub fn uo_fs() -> Option<Pk3Fs> {
    let dir = game_dir().join("uo");
    if !dir.is_dir() {
        return None;
    }
    Pk3Fs::open(&dir).ok()
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

/// A committed capture under this crate's `tests/fixtures/`, by relative path.
pub fn fixture(rel: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}

/// A committed gamestate capture (`--save-fixture`), decoded.
pub fn fixture_gamestate(
    p: &crate::net::protocol::Protocol,
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
    p: &crate::net::protocol::Protocol,
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
