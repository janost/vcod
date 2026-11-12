//! `getStat` and `setStat`'s refusals on a running mp_crash `war`
//! (`GScr_GetStat` 0x80c55f6, `GScr_SetStat` 0x80c5490; cod4-gsc.md, "The
//! stats, the loadout, the perks"). Needs `COD4_DIR`.

use super::vote_tests::{run_ms, two_playing, two_playing_with};
use super::*;
use vcod_gsc::{EntId, ErrorKind, Target, Value};

/// The 1.7 builtin `name` on the level's host.
fn call(sv: &mut Cod4Server, name: &str, recv: EntId, args: &[Value]) -> Result<Value, ErrorKind> {
    let f = crate::game::builtins::cod4::NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1;
    let level = sv.game.level.as_mut().unwrap();
    level
        .rt
        .with_host(|h, cx| f(h, cx, Some(Target::Entity(recv)), args))
}

fn refused(r: Result<Value, ErrorKind>) -> String {
    match r {
        Err(ErrorKind::Custom(m)) => m,
        other => panic!("{other:?}"),
    }
}

/// `getStat` reads 3498 and 3499 as 0, and refuses an index past 3499
/// unsigned, a receiver with no client and a slot that has gone.
#[test]
fn cod4_get_stat_refuses_as_retail() {
    let Some((mut sv, _, _)) = two_playing() else {
        return;
    };
    let level = sv.game.level.as_ref().unwrap();
    let (p0, p1) = (
        level.rt.client_entity(0).unwrap(),
        level.rt.client_entity(1).unwrap(),
    );
    assert_eq!(
        call(&mut sv, "getstat", p0, &[Value::Int(3498)]),
        Ok(Value::Int(0))
    );
    assert_eq!(
        refused(call(&mut sv, "getstat", p0, &[Value::Int(3500)])),
        "getstat: invalid index 3500"
    );
    assert_eq!(
        refused(call(&mut sv, "getstat", p0, &[Value::Int(-1)])),
        "getstat: invalid index -1"
    );
    let level = sv.game.level.as_mut().unwrap();
    let ent = level.rt.with_host(|h, cx| h.ents.spawn(cx).unwrap());
    assert_eq!(
        refused(call(&mut sv, "getstat", ent, &[Value::Int(0)])),
        "getstat: entity must be a player entity"
    );
    // `sess.connected` at 0: `ClientDisconnect` has run on the slot.
    sv.game.level.as_mut().unwrap().rt.host.client_roster[1] = None;
    assert_eq!(
        refused(call(&mut sv, "getstat", p1, &[Value::Int(0)])),
        "getstat: called on a disconnected player"
    );
}

/// `setStat` refuses a receiver with no client and, ahead of its argument
/// checks, any call while `developer_script` is on.
#[test]
fn cod4_set_stat_refuses_under_developer_script() {
    let Some((mut sv, _, _)) = two_playing() else {
        return;
    };
    let p0 = sv.game.level.as_ref().unwrap().rt.client_entity(0).unwrap();
    let level = sv.game.level.as_mut().unwrap();
    let ent = level.rt.with_host(|h, cx| h.ents.spawn(cx).unwrap());
    assert_eq!(
        refused(call(
            &mut sv,
            "setstat",
            ent,
            &[Value::Int(0), Value::Int(1)]
        )),
        "setstat: entity must be a player entity"
    );
    assert_eq!(
        call(&mut sv, "setstat", p0, &[Value::Int(2317), Value::Int(5)]),
        Ok(Value::Undefined)
    );
    let level = sv.game.level.as_mut().unwrap();
    level.rt.host.cvars.set("developer_script", "1");
    assert_eq!(
        refused(call(&mut sv, "setstat", p0, &[Value::Int(2317)])),
        "setstat: developer_script is false."
    );
    assert_eq!(
        call(&mut sv, "getstat", p0, &[Value::Int(2317)]),
        Ok(Value::Int(5))
    );
}

/// A ranked match (`sv_pure 1`): 300 XP off a fresh profile's 0 carries
/// the player past rank 3's 270 (`mp/ranktable.csv`), whose `feature_cac`
/// `_rank.gsc`'s `unlockFeature` stores as stat 260 = 2, the stat the main
/// menu's Create a Class row reads. Each write reaches the client as `N`.
#[test]
fn cod4_ranked_xp_unlocks_create_a_class() {
    let Some((mut sv, mut probes, mut now)) = two_playing_with(&[("sv_pure", "1")]) else {
        return;
    };
    let p0 = sv.game.level.as_ref().unwrap().rt.client_entity(0).unwrap();
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    rt.start_script_thread_for_test("maps/mp/gametypes/_rank", "giveRankXP", Some(p0), |cx| {
        vec![Value::String(cx.intern_exact("kill")), Value::Int(300)]
    });
    run_ms(&mut sv, &mut probes, &mut now, 500);
    let stats = &sv.game.level.as_ref().unwrap().rt.host.cod4().stats[0];
    let at = |i: u16| stats.get(i).unwrap_or(0);
    // rankxp, rank, minxp, maxxp; 252 the new rank's index; 260 the feature.
    assert_eq!(
        [at(2301), at(2350), at(2351), at(2352), at(252), at(260)],
        [300, 3, 270, 480, 3, 2]
    );
    let seen = probes[0].server_commands();
    for line in ["N 2301 300", "N 260 2"] {
        assert!(seen.iter().any(|c| c == line), "{line}: {seen:?}");
    }
}

/// A `fast_restart` keeps what the scripts' `setStat`s wrote (retail's
/// block lives in the client, `SV_SetClientStat`): the restarted level's
/// `_rank.gsc` reads the earned XP back, not the join's block.
#[test]
fn cod4_fast_restart_keeps_the_scripts_stats() {
    let Some((mut sv, mut probes, mut now)) = two_playing_with(&[("sv_pure", "1")]) else {
        return;
    };
    let p0 = sv.game.level.as_ref().unwrap().rt.client_entity(0).unwrap();
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    rt.start_script_thread_for_test("maps/mp/gametypes/_rank", "giveRankXP", Some(p0), |cx| {
        vec![Value::String(cx.intern_exact("kill")), Value::Int(300)]
    });
    run_ms(&mut sv, &mut probes, &mut now, 500);
    sv.map_restart(true).unwrap();
    run_ms(&mut sv, &mut probes, &mut now, 500);
    let stats = &sv.game.level.as_ref().unwrap().rt.host.cod4().stats[0];
    let at = |i: u16| stats.get(i).unwrap_or(0);
    assert_eq!([at(2301), at(2350), at(260)], [300, 3, 2]);
}
