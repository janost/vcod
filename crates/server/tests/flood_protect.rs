//! `SV_ClientCommand`'s flood protection: a non-exempt client command opens
//! an 800 ms window in which every further non-exempt one from an active
//! client is dropped before the game sees it (`docs/protocol-1.1.md`,
//! "Client commands are flood-protected"). A bare `score` is not exempt, so
//! two of them 500 ms apart get one `b` answer, and one 900 ms later gets
//! its own.

mod common;

use common::{connect, step, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::NetEvent;
use vcod_server::{Server, ServerConfig};

const FRAME_MS: u64 = 50;

/// No world and no scripts: the scoreboard a bare server answers is empty,
/// and whether it answers at all is the whole measurement.
fn server() -> Server {
    Server::new(
        ServerConfig {
            map: "mp_carentan".into(),
            hostname: "flood".into(),
            max_clients: 2,
            gametype: "dm".into(),
            test_entities: 0,
            trace: false,
            bots: 0,
            bots_shoot: false,
        },
        Instant::now(),
    )
}

/// Runs `frames` frames and returns how many `b` answers arrived.
fn scoreboards(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    cl: &mut vcod_common::net::NetClient<common::ClientEnd>,
    now: &mut Instant,
    frames: u64,
) -> usize {
    let mut n = 0;
    for _ in 0..frames {
        *now += Duration::from_millis(FRAME_MS);
        cl.send_frame(&NULL_USERCMD);
        for e in step(sv, q, cl, *now) {
            if let NetEvent::ServerCommand(t) = e {
                if t.first().map(String::as_str) == Some("b") {
                    n += 1;
                }
            }
        }
    }
    n
}

#[test]
fn a_second_score_inside_the_window_is_dropped_and_one_past_it_is_answered() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    // Into the world: the gate applies to an active client only.
    assert!(scoreboards(&mut sv, &q, &mut cl, &mut now, 10) == 0);

    cl.send_reliable("score");
    assert_eq!(
        scoreboards(&mut sv, &q, &mut cl, &mut now, 2),
        1,
        "the first score is answered"
    );

    // 500 ms after the first: inside the window it opened.
    for _ in 0..8 {
        now += Duration::from_millis(FRAME_MS);
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
    }
    cl.send_reliable("score");
    assert_eq!(
        scoreboards(&mut sv, &q, &mut cl, &mut now, 2),
        0,
        "a score 500 ms later is dropped"
    );

    // The dropped one re-opened the window; 900 ms past it is clear.
    for _ in 0..16 {
        now += Duration::from_millis(FRAME_MS);
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
    }
    cl.send_reliable("score");
    assert_eq!(
        scoreboards(&mut sv, &q, &mut cl, &mut now, 2),
        1,
        "a score 900 ms later is answered"
    );
}
