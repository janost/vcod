//! Replays `fixtures/mp_toujane-spectate.bin` (see `replay.rs`) through the
//! client's receive path ([`Client::receive`]) into the snapshot view the
//! renderer reads ([`NetView`]): the live configstrings, the entities and
//! the playerstate, and the render-time lerp between two snapshots.

use vcod_cod2::net::capture::{Capture, Dir};
use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::events::ET_PLAYER;
use vcod_cod2::net::events::ET_SCRIPTMOVER;
use vcod_cod2::net::fields::ENTITY_STATE_FIELDS;
use vcod_cod2::net::oob::{self, Reply};
use vcod_cod2::net::{Client, EntityState2, EntityView, NetView, SnapshotView};
use vcod_cod2::trajectory::TR_LINEAR_STOP;
use vcod_cod2::trajectory::{TR_INTERPOLATE, TR_LINEAR, TR_STATIONARY};

const FIXTURE: &[u8] = include_bytes!("fixtures/mp_toujane-spectate.bin");

/// The fixture through the receive path into a view; also every snapshot's
/// player entity origin.
fn replay() -> (NetView, usize, Vec<glam::Vec3>) {
    let cap = Capture::decode(FIXTURE).unwrap();
    let mut challenge = None;
    let mut client: Option<Client> = None;
    let mut view = NetView::default();
    let mut snaps = 0;
    let mut player_origins = Vec::new();
    for (dir, p) in &cap.packets {
        if let Some(pl) = oob::payload(p) {
            match (dir, oob::parse_reply(pl)) {
                (Dir::FromServer, Reply::Challenge(c)) => challenge = Some(c),
                (Dir::ToServer, Reply::Other(text)) => {
                    if let Some(q) = text
                        .strip_prefix("connect \"")
                        .and_then(|ui| oob::info_value_for_key(ui.trim_end_matches('"'), "qport"))
                    {
                        client = Some(Client::new(q.parse().unwrap(), challenge.unwrap()));
                    }
                }
                _ => {}
            }
            continue;
        }
        if *dir != Dir::FromServer {
            continue;
        }
        let cl = client.as_mut().unwrap();
        let Some(parsed) = cl.receive(p).unwrap() else {
            continue;
        };
        view.update(&cl.parser, &parsed);
        if parsed.snapshot.is_some() {
            snaps += 1;
            let s = view.snap.as_ref().unwrap();
            let e = s.entity(0).expect("the other client's body");
            assert_eq!(e.etype, ET_PLAYER);
            // A client's body rides TR_LINEAR_STOP and still lerps.
            assert_eq!(e.pos.tr_type, TR_LINEAR_STOP);
            assert!(vcod_cod2::net::Frame::interpolates(e));
            player_origins.push(e.pos.base);
        }
    }

    (view, snaps, player_origins)
}

#[test]
fn retail_session_through_the_snapshot_view() {
    let (view, snaps, player_origins) = replay();
    assert_eq!(view.gamestates, 1);
    assert_eq!(snaps, 23);
    let cs = &view.configstrings;
    assert_eq!(cs.map_name(), Some("mp_toujane"));
    assert_eq!(cs.model(1), Some("xmodel/defaultweapon"));
    assert_eq!(cs.material(1), Some("mpflag_british"));
    assert_eq!(cs.weapon_name(1), Some("defaultweapon_mp"));

    let (prev, snap) = (view.prev.as_ref().unwrap(), view.snap.as_ref().unwrap());
    assert_eq!(snap.server_time - prev.server_time, 50);
    assert_eq!(snap.entities.len(), 28);
    // The probe spectates free: its view is its own.
    assert_eq!(snap.ps.client_num, 1);
    assert_eq!(snap.ps.own_client_num, 1);
    assert_eq!(snap.ps.followed(), None);
    assert_eq!(snap.ps.pm_type, 4);
    assert_ne!(player_origins.first(), player_origins.last());

    // Half way between the two snapshots, the walking body and the eye sit
    // half way between their two states.
    let f = view.frame(prev.server_time + 25).unwrap();
    assert_eq!(f.frac, 0.5);
    let (a, b) = (prev.entity(0).unwrap(), snap.entity(0).unwrap());
    let mid = f.entity_origin(b);
    for i in 0..3 {
        let (pa, pb) = (
            a.pos.evaluate(prev.server_time),
            b.pos.evaluate(snap.server_time),
        );
        assert!((mid[i] - (pa[i] + pb[i]) * 0.5).abs() < 1e-3);
    }
    let eye = f.eye();
    assert!((eye[2] - (prev.ps.eye()[2] + snap.ps.eye()[2]) * 0.5).abs() < 1e-3);
    // Past the newest snapshot the view holds it: nothing to lerp to, the
    // time runs on (`CG_ProcessSnapshots`, `CG_SetFrameInterpolation`).
    let f = view.frame(snap.server_time + 500).unwrap();
    assert_eq!((f.frac, f.prev.is_none()), (0.0, true));
    assert_eq!(
        (f.snap.message_num, f.time_ms),
        (snap.message_num, snap.server_time + 500)
    );
}

fn set(es: &mut EntityState2, name: &str, bits: u32) {
    es.f[field_index(&ENTITY_STATE_FIELDS, name)] = bits;
}

fn ent(number: u32, tr_type: i32, at: [f32; 3]) -> EntityState2 {
    let mut e = EntityState2::null(number);
    set(&mut e, "pos.trType", tr_type as u32);
    for (i, v) in at.iter().enumerate() {
        set(&mut e, &format!("pos.trBase[{i}]"), v.to_bits());
    }
    e
}

/// The capture's newest snapshot holding `entities` at `server_time`.
fn snap(base: &SnapshotView, server_time: i32, entities: &[EntityState2]) -> SnapshotView {
    let mut s = base.clone();
    s.server_time = server_time;
    s.message_num = server_time / 50;
    s.entities = entities.iter().map(EntityView::new).collect();
    s.entities.sort_by_key(|e| e.number);
    s
}

fn pair(base: &SnapshotView, a: &[EntityState2], b: &[EntityState2]) -> NetView {
    let mut v = NetView::default();
    v.push(snap(base, 1000, a), (0, 0));
    v.push(snap(base, 1050, b), (0, 0));
    v
}

#[test]
fn interpolated_entities_lerp_and_riders_follow_their_mover() {
    let (view, ..) = replay();
    let base = view.snap.unwrap();
    // TR_INTERPOLATE goes from the older state to the newer by `frac`.
    let v = pair(
        &base,
        &[ent(70, TR_INTERPOLATE, [0.0, 0.0, 0.0])],
        &[ent(70, TR_INTERPOLATE, [100.0, 0.0, 0.0])],
    );
    let f = v.frame(1012).unwrap();
    let e = f.snap.entity(70).unwrap();
    assert!((f.entity_origin(e).x - 24.0).abs() < 1e-4);

    // A mover sliding +x at 100 u/s carries what stands on it.
    let mut mover = ent(80, TR_LINEAR, [0.0, 0.0, 0.0]);
    set(&mut mover, "eType", ET_SCRIPTMOVER as u32);
    set(&mut mover, "pos.trTime", 1000);
    set(&mut mover, "pos.trDelta[0]", 100f32.to_bits());
    let mut rider = ent(90, TR_STATIONARY, [5.0, 0.0, 0.0]);
    set(&mut rider, "groundEntityNum", 80);
    let v = pair(&base, &[mover, rider], &[mover, rider]);
    let f = v.frame(1025).unwrap();
    let origin = f.entity_origin(f.snap.entity(90).unwrap());
    assert!((origin.x - 7.5).abs() < 1e-4);
}
