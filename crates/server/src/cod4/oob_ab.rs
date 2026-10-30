//! `cod4::oob_ab`: retail's out-of-band half of `handshake-1.7.txt`
//! (`getinfo`, `getstatus`, `getchallenge`, a refused and an accepted
//! `connect`, the seven `stats`) replayed on a bare vcod server configured as
//! the capture's (`CoD4Host`, 8 slots, mp_crash `war`), every reply compared
//! byte for byte with retail's (cod4-handshake.md 2, 3).

use super::*;
use vcod_cod4::net::capture;
use vcod_cod4::net::wire::map_center;

/// Replies that differ by nature, each with its reason.
const GAPS: &[(&str, &str)] = &[(
    "challengeResponse",
    "the challenge is a rand() draw: only the prefix and the number's form compare",
)];

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod4/tests/fixtures/net/handshake-1.7.txt"
);

fn oob(d: &[u8]) -> Option<&[u8]> {
    d.strip_prefix(&[0xff; 4])
}

#[test]
fn oob_replies_match_retail_byte_for_byte() {
    let lines = capture::parse(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let mut server = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        1,
    );
    server.load(None).unwrap();
    let from: SocketAddr = "127.0.0.1:4660".parse().unwrap();
    let now = Instant::now();
    let mut ours: Vec<Vec<u8>> = Vec::new();
    let mut theirs: Vec<Vec<u8>> = Vec::new();
    let (mut retail_challenge, mut our_challenge) = (String::new(), String::new());
    for l in &lines {
        let Some(text) = oob(&l.data) else {
            // The handshake's out-of-band half ends at the first netchan packet.
            if l.dir == "c2s" {
                break;
            }
            continue;
        };
        match l.dir.as_str() {
            "s2c" => {
                if let Some(c) = text.strip_prefix(b"challengeResponse ") {
                    retail_challenge = String::from_utf8_lossy(c).into_owned();
                }
                theirs.push(l.data.clone());
            }
            "c2s" => {
                let mut pkt = l.data.clone();
                // Our challenge in place of retail's.
                if text.starts_with(b"connect ") {
                    let s = String::from_utf8_lossy(text).replace(
                        &format!("\\challenge\\{retail_challenge}"),
                        &format!("\\challenge\\{our_challenge}"),
                    );
                    pkt = [&[0xff; 4][..], s.as_bytes()].concat();
                }
                server.handle_packet(from, &pkt, now);
                for (to, p) in server.take_outgoing() {
                    assert_eq!(to, from);
                    if let Some(c) = oob(&p).and_then(|t| t.strip_prefix(b"challengeResponse ")) {
                        our_challenge = String::from_utf8_lossy(c).into_owned();
                    }
                    ours.push(p);
                }
            }
            _ => {}
        }
    }
    assert_eq!(theirs.len(), 12, "the capture's replies");
    assert_eq!(ours.len(), theirs.len());
    for (o, t) in ours.iter().zip(&theirs) {
        let gap = GAPS
            .iter()
            .find(|(prefix, _)| oob(t).is_some_and(|t| t.starts_with(prefix.as_bytes())));
        if gap.is_some() {
            let num = |d: &[u8]| String::from_utf8_lossy(&d[4 + 18..]).parse::<i32>().is_ok();
            assert!(num(o) && num(t), "{:?}", String::from_utf8_lossy(o));
            continue;
        }
        assert_eq!(
            String::from_utf8_lossy(o),
            String::from_utf8_lossy(t),
            "a reply differs from retail's"
        );
    }
}

/// The retail gamestate of `handshake-1.7.txt`.
fn retail_gamestate() -> Gamestate {
    let lines = capture::parse(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let mut s = capture::Session::new();
    for l in &lines {
        s.feed(l).unwrap();
        if let Some(gs) = &s.client.gamestate {
            return gs.clone();
        }
    }
    panic!("no gamestate in the capture")
}

/// Engine configstrings the bare level leaves to the gsc boot (S-C4-2) or
/// gets only partly, with their reasons.
const CS_GAPS: &[(u16, &str)] = &[
    (9, "fog: the map script's setExpFog"),
    (
        11,
        "CS_GAMEENDTIME: _globallogic's setGameEndTime (619800 = 19800 + war's 10 min)",
    ),
    (
        12,
        "map centre: z is _spawnlogic's over the spawns' ground drop, which the bare level \
         has no script for (the level's matches, cod4::wire_ab)",
    ),
    (19, "\\winner\\0: the game module's"),
];

/// The engine's configstrings 0, 1 (with `$COD4_DIR`'s iwds and zones),
/// 2, 12's x and y and the dvar mirror against retail's gamestate; every
/// other slot of retail's below the localized strings is named in
/// [`CS_GAPS`].
#[test]
fn engine_configstrings_match_retail() {
    let retail = retail_gamestate();
    let mut server = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        1,
    );
    let install = vcod_cod4::testing::cod4_install();
    server.load(install.as_ref()).unwrap();
    // The capture joined past the prematch (its game end time 619800 is
    // set): 30 s of the level, as `cod4_boot` runs it.
    for _ in 0..600 {
        server.core.sv_time_ms += FRAME_MS;
        server.run_level_frame();
    }
    server.write_engine_configstrings();
    let ours = Gamestate::from_table(0, &server.core.configstrings, &BTreeMap::new(), 0, 0);
    let text = |gs: &Gamestate, i: u16| String::from_utf8_lossy(gs.configstring(i)).into_owned();
    let mut checked = vec![0u16, 2];
    if install.is_some() {
        // `default_mp.cfg` ran, its `exec default_mp_gamesettings.cfg`
        // too, the later of `iw_00.iwd`'s two: the playerwait is the
        // script's default, as retail's rcon reads both.
        assert_eq!(server.cvar("scr_war_scorelimit"), "750");
        assert_eq!(server.cvar("scr_game_playerwaittime"), "15");
        checked.push(1);
        let c = |gs: &Gamestate| map_center(gs.configstring(12));
        assert_eq!(c(&ours)[..2], c(&retail)[..2], "map centre x, y");
    }
    checked.extend(20..276);
    for i in checked {
        assert_eq!(text(&ours, i), text(&retail, i), "configstring {i}");
    }
    for &i in retail.configstrings.keys().filter(|&&i| i < 309) {
        let ok = matches!(i, 0..=2 | 20..=275) || CS_GAPS.iter().any(|(g, _)| *g == i);
        assert!(ok, "retail's configstring {i} is neither checked nor a gap");
    }
}
