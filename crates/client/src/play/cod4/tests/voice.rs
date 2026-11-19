//! Voice chat on a live server.

use vcod_cod4::net::conn::Cp;

use super::*;
use crate::play::cod4_menus::Join as MenuJoin;

/// Three clients on vcod-server's war with `sv_voice 1`: we (allies)
/// hold `+talk` with a tone for a microphone, through the session's
/// batch (`CL_VoiceTransmit`) out of band to the server, which relays
/// each frame to our teammate (allies) and not the enemy (axis); with
/// `voice_global 1` the enemy hears too. The teammate's talker list
/// names us, and the server stamps our `lastVoiceTime`. With the
/// system's libspeex the frames are Speex: 20 bytes at quality 3,
/// which decode back to the tone; without it the test sends fixed
/// frames (cod4-voice.md 5).
#[test]
fn cod4_live_voice_reaches_the_team_and_voice_global_the_enemy() {
    use crate::voice::{Speex, Tone, VoiceChat, VoiceFrame, VoiceInput, may_send};
    const RATE: u32 = vcod_cod4::net::voice::SAMPLE_RATE;
    use crate::play::input::Action;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    struct Steady(Tone);
    impl VoiceInput for Steady {
        fn take(&mut self) -> Vec<i16> {
            // 16 ms of 8000 Hz a frame, as the clock steps.
            self.0.samples(128)
        }
    }
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.set_cvar("sv_voice", "1");
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let global = Arc::new(AtomicBool::new(false));
    let stamped = Arc::new(AtomicI32::new(-1));
    let (g, st) = (global.clone(), stamped.clone());
    let clock = sim::Clock::new();
    #[derive(Debug, Default)]
    struct Seen {
        mate: Vec<(u8, Vec<u8>)>,
        enemy_before: usize,
        enemy_after: Vec<(u8, Vec<u8>)>,
        mate_listed: bool,
        sent: u64,
    }
    let client = sim::spawn(&clock, move || {
        let join = |team: &str| MenuJoin {
            team: Some(team.into()),
            class: Some("assault_mp,0".into()),
        };
        let mut t = Cod4Session::connect(addr, "talker", Cp::Bare, join("allies")).unwrap();
        let mut m = Cod4Session::connect(addr, "mate", Cp::Bare, join("allies")).unwrap();
        let mut e = Cod4Session::connect(addr, "enemy", Cp::Bare, join("axis")).unwrap();
        let speex = Speex::open(RATE);
        let real = speex.is_some();
        let mut chat = VoiceChat::with(RATE, speex, Some(Box::new(Steady(Tone::new(RATE)))));
        let mut seen = Seen::default();
        let t0 = clk_now();
        let mut talk_from = None;
        while since(t0) < Duration::from_secs(60) {
            let now = since(t0).as_secs_f64() * 1000.0;
            for c in [&mut t, &mut m, &mut e] {
                c.step(now).unwrap();
                c.take_commands();
                c.take_fired();
            }
            let all = [&t, &m, &e].iter().all(|c| c.spawn().is_some());
            if all && talk_from.is_none() {
                talk_from = Some(now);
                t.input.key(Action::Talk, true);
            }
            let (sv_voice, quality) = t.voice_settings();
            let held = t.input.held(Action::Talk);
            let f = VoiceFrame {
                sending: may_send(
                    held,
                    t.client().snapshots.last().is_some(),
                    sv_voice,
                    true,
                    25000,
                ),
                quality,
                mic: Default::default(),
            };
            if real {
                chat.frame(&mut t.voice, &f, None);
            } else if f.sending {
                t.voice.queue(vec![0x5a; 20]);
            }
            let mate_heard = m.voice.take_heard();
            if !mate_heard.is_empty() && m.talkers() == vec![0] {
                seen.mate_listed = true;
            }
            seen.mate.extend(mate_heard);
            let enemy_heard = e.voice.take_heard();
            if g.load(Ordering::Relaxed) {
                seen.enemy_after.extend(enemy_heard);
            } else {
                seen.enemy_before += enemy_heard.len();
            }
            match talk_from.map(|t0| now - t0) {
                Some(ms) if ms > 4000.0 => break,
                Some(ms) if ms > 2000.0 => g.store(true, Ordering::Relaxed),
                _ => {}
            }
            clk_sleep(Duration::from_millis(16));
        }
        seen.sent = chat.sent;
        for c in [&mut t, &mut m, &mut e] {
            c.disconnect();
        }
        (seen, real)
    });
    let mut set = false;
    serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
        if global.load(Ordering::Relaxed) && !set {
            set = true;
            sv.set_cvar("voice_global", "1");
        }
        if let Some(t) = sv.game.level.as_ref().and_then(|l| l.last_voice_time(0)) {
            st.store(t, Ordering::Relaxed);
        }
        true
    });
    let (seen, real) = client.join().unwrap();
    assert!(
        seen.mate.len() > 50,
        "{} frames reached the mate",
        seen.mate.len()
    );
    assert!(seen.mate.iter().all(|(t, _)| *t == 0));
    assert!(seen.mate_listed, "the mate's talker list never named us");
    assert_eq!(
        seen.enemy_before, 0,
        "the enemy heard us without voice_global"
    );
    assert!(
        !seen.enemy_after.is_empty(),
        "voice_global relayed nothing to the enemy"
    );
    assert!(stamped.load(Ordering::Relaxed) > 0, "no lastVoiceTime");
    if real {
        assert!(seen.sent as usize >= seen.mate.len());
        assert!(seen.mate.iter().all(|(_, f)| f.len() == 20));
        let mut dec = Speex::open(RATE).unwrap();
        let pcm: Vec<i16> = seen.mate[20..40]
            .iter()
            .flat_map(|(_, f)| dec.decode(f))
            .collect();
        assert_eq!(pcm.len(), 20 * 160);
        let rms =
            (pcm.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / pcm.len() as f64).sqrt();
        assert!(rms > 1000.0, "the tone decoded to rms {rms}");
    }
}
