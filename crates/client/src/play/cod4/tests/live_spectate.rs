//! Live spectating: a spectator's view held, flown and following.

use super::*;

/// At the stock `scr_game_spectatetype` 1 on `war` the team menu's
/// spectator watches no one (`setSpectatePermissions` refuses every
/// team to the spectator team) and stands still holding forward
/// (`ps.speed` 0, cod4-pmove.md 15); attack follows no one. Then the
/// team menu again takes us onto allies and a spawn.
#[test]
fn cod4_live_spectator_at_1_holds_still_then_joins_a_team() {
    use crate::play::input::Action;
    let mut first = None;
    let mut asked = false;
    let Some(r) = live_spectate("war", &[], move |s, _, now| {
        let (of, num, pm, o) = spec_ps(s)?;
        if s.spawn().is_some() {
            return Some(Ok(first));
        }
        if pm != 4 || num != s.gamestate()?.client_num as u32 {
            return None;
        }
        let at = *first.get_or_insert((now, o));
        s.input.key(Action::Forward, true);
        s.input
            .key(Action::Attack, (now - at.0) as i32 / 300 % 2 == 1);
        if of & 2 != 0 {
            return Some(Err(format!("followed {num}")));
        }
        if now - at.0 > 2500.0 && !asked {
            let moved = (0..3).map(|i| (o[i] - at.1[i]).abs()).fold(0.0, f32::max);
            if moved > 0.01 {
                return Some(Err(format!("moved {moved}")));
            }
            s.input.release_all();
            s.open_main_menu();
            let gs = s.gamestate()?.clone();
            let i = s
                .menus
                .open()?
                .responses
                .iter()
                .position(|r| r == "allies")?;
            let c = s.menus.choose(i, &gs)?;
            s.command(&c);
            asked = true;
        }
        None
    }) else {
        return;
    };
    let r = r.expect("no spawn after the team pick");
    assert!(r.unwrap().is_some(), "never a spectator");
}

/// `dm` at `scr_game_spectatetype` 1 lets a spectator watch any team
/// but not fly, so the end frame follows the first player playing
/// (`Cmd_FollowCycle_f(ent, 1)` from `SpectatorClientEndFrame`): the
/// bot, its playerstate ours with the follow bit.
#[test]
fn cod4_live_spectator_at_1_follows_the_bot_on_dm() {
    let Some(r) = live_spectate("dm", &[], |s, bot, _| {
        let (of, num, _, _) = spec_ps(s)?;
        (of & 2 != 0 && num == bot as u32).then_some(())
    }) else {
        return;
    };
    assert!(r.is_some(), "never followed the bot");
}

/// At `scr_game_spectatetype` 2 a spectator flies on its keys; attack
/// follows the bot (`SpectatorThink`'s `Cmd_FollowCycle_f`), melee
/// lets go (`StopFollowing`), and it flies on from there.
#[test]
fn cod4_live_spectator_at_2_flies_follows_and_lets_go() {
    use crate::play::input::Action;
    #[derive(Debug, PartialEq)]
    enum Step {
        Fly,
        Follow,
        Release,
        Again(f64),
    }
    let mut step = Step::Fly;
    let mut from = None;
    let mut log = Vec::new();
    let Some(r) = live_spectate(
        "war",
        &[("scr_game_spectatetype", "2")],
        move |s, bot, now| {
            let (of, num, pm, o) = spec_ps(s)?;
            let me = s.gamestate()?.client_num as u32;
            let i = &mut s.input;
            match step {
                Step::Fly if pm == 4 && num == me => {
                    let o0 = *from.get_or_insert(o);
                    i.key(Action::Forward, true);
                    let d = ((o[0] - o0[0]).powi(2) + (o[1] - o0[1]).powi(2)).sqrt();
                    if d > 100.0 {
                        log.push(format!("flew {d:.0} at {now:.0}"));
                        i.key(Action::Forward, false);
                        i.key(Action::Attack, true);
                        step = Step::Follow;
                    }
                }
                Step::Follow if of & 2 != 0 && num == bot as u32 => {
                    log.push(format!("followed {num} at {now:.0}"));
                    i.key(Action::Attack, false);
                    i.key(Action::Melee, true);
                    step = Step::Release;
                }
                Step::Release if of & 2 == 0 && num == me => {
                    log.push(format!("let go at {now:.0}"));
                    i.key(Action::Melee, false);
                    from = Some(o);
                    step = Step::Again(now);
                }
                Step::Again(at) => {
                    // Let go facing a wall the bot stood at, forward
                    // goes nowhere: back away after a second.
                    let back = now - at > 1000.0;
                    i.key(Action::Forward, !back);
                    i.key(Action::Back, back);
                    let o0 = from.unwrap();
                    let d = ((o[0] - o0[0]).powi(2) + (o[1] - o0[1]).powi(2)).sqrt();
                    if d > 100.0 {
                        return Some(log.clone());
                    }
                }
                _ => {}
            }
            None
        },
    ) else {
        return;
    };
    let log = r.expect("the fly, follow, let go and fly again did not all happen");
    eprintln!("{log:?}");
}
