//! Sound builtins. The three that take an alias allocate a sound-alias
//! configstring, mirroring `G_SoundAliasIndex`
//! (docs/research/cod11-sound-system.md: they share the 525-779 range).
//! `playSound` raises `EV_SOUND_ALIAS` on the receiver's own event ring,
//! `playLocalSound` reaches one client as the reliable command `s <idx>`, and
//! `playLoopSound` writes the `es.loopSound` netfield that `stopLoopSound`
//! clears. `musicPlay`, `musicStop` and `soundFade` are reliable commands
//! to every client.

use crate::configstrings::CsRange;
use crate::game::builtins::client::client_receiver;
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::{GameHost, SimOp};
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("playsound", play_sound),
    ("playloopsound", play_loop_sound),
    ("playlocalsound", play_local_sound),
    ("stoploopsound", stop_loop_sound),
    ("musicplay", music_play),
    ("musicstop", music_stop),
    ("soundfade", sound_fade),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The alias's configstring slot, allocated on first use the way retail's
/// `G_SoundAliasIndex` allocates it.
pub(super) fn alloc_alias(
    host: &mut GameHost,
    cx: &mut Cx,
    args: &[Value],
) -> Result<usize, ErrorKind> {
    let Some(Value::String(alias)) = args.first() else {
        return Err(ErrorKind::BadType("takes a sound alias"));
    };
    let alias = *alias;
    let text = cx.resolve(alias).to_string();
    host.allocators
        .index(&mut host.configstrings, CsRange::SoundAlias, &text)
}

/// `CS_SOUNDS`, what an `s <idx>` index counts from: the alias range starts
/// one slot above it, so the first alias travels as 1
/// (docs/protocol-1.1.md, `s <idx>`).
pub(super) fn sounds_base(host: &GameHost) -> usize {
    host.allocators.layout().sounds()
}

/// `EV_SOUND_ALIAS`, what `G_PlaySoundAlias` appends
/// (docs/research/cod11-sound-system.md, section 9).
use vcod_common::net::event_ids::EV_SOUND_ALIAS;

/// `<ent> playSound(alias)`: `G_SoundAliasIndex` -> `G_PlaySoundAlias`. The
/// two rings that call chooses between are owned by different halves of the
/// server (sound doc, section 9): a client's is the sim's playerstate, so it
/// travels as a `SimOp`, and an entity's is the one `crate::game::wire`
/// writes into its snapshots. Either way the entity turns `SVF_BROADCAST`.
pub fn play_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let parm = (alloc_alias(host, cx, args)? - sounds_base(host)) as i32;
    let (v1, now) = (is_v1(host), host.level_time_ms);
    let Some(ent) = host.ents.get_mut(id) else {
        return Ok(Value::Undefined);
    };
    // 1.1 clears `SVF_NOCLIENT` and sets `SVF_BROADCAST` for good; 1.5
    // (game.mp 0x5d836) and UO (0x68abb) only stamp `r.broadcastTime`.
    if v1 {
        ent.broadcast = true;
    } else {
        ent.broadcast_time = now.wrapping_add(BROADCAST_MS);
    }
    let is_client = ent.client.is_some();
    if is_client {
        host.client_sim_ops.push((
            id.0 as usize,
            SimOp::Event {
                event: EV_SOUND_ALIAS,
                parm,
            },
        ));
    } else if let Some(ent) = host.ents.get_mut(id) {
        ent.events.add(EV_SOUND_ALIAS, parm);
    }
    Ok(Value::Undefined)
}

/// How long 1.5's and UO's `playSound` keeps its entity unculled.
const BROADCAST_MS: i32 = 300;

/// `<ent> playLoopSound(alias)` (`0x5d980`): `es.loopSound = idx`, the index
/// counted from `CS_SOUNDS` the same way `EV_SOUND_ALIAS`'s parm is. The field
/// is state, not an event: the client plays `CS_SOUNDS + loopSound` every
/// frame the entity is on the wire and the engine's looping dedupe collapses
/// that into one voice (sound doc, section 9).
pub fn play_loop_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let idx = (alloc_alias(host, cx, args)? - sounds_base(host)) as i32;
    let v1 = loops_v1(host);
    if let Some(ent) = host.ents.get_mut(id) {
        ent.loop_sound = idx;
        // 1.5 (game.mp 0x5d8a9), UO (0x68b2e) and CoD2 (lnxded 0x8111e0c)
        // broadcast a looping entity for as long as it loops.
        if !v1 {
            ent.broadcast_time = -1;
        }
    }
    Ok(Value::Undefined)
}

/// `<ent> stopLoopSound()` (`0x5d9d8`): `es.loopSound = 0`. It takes no alias
/// and allocates no configstring; index 0 is the range's "none" slot.
pub fn stop_loop_sound(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let (v1, now) = (loops_v1(host), host.level_time_ms);
    if let Some(ent) = host.ents.get_mut(id) {
        ent.loop_sound = 0;
        // 1.5 (0x5d908), UO (0x68b8d) and CoD2 (lnxded 0x8111e5a): 300 ms
        // more, as `playSound`.
        if !v1 {
            ent.broadcast_time = now.wrapping_add(BROADCAST_MS);
        }
    }
    Ok(Value::Undefined)
}

/// Whether the server speaks 1.1, whose sound builtins write `svFlags`
/// rather than `r.broadcastTime`.
fn is_v1(host: &GameHost) -> bool {
    host.defs.proto.is_v1()
}

/// [`is_v1`] for the loop sounds, which CoD2 broadcasts through
/// `r.broadcastTime` as 1.5 does (cod2-protocol.md 15.7).
fn loops_v1(host: &GameHost) -> bool {
    is_v1(host) && !host.profile.loop_sounds_broadcast_time
}

/// `<player> playLocalSound(alias)` (sound doc, section 9): the reliable
/// command `s <idx>` to that client and nobody else, non-positional. The
/// receiver must be a player, as retail's own check is.
pub fn play_local_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let idx = alloc_alias(host, cx, args)? - sounds_base(host);
    host.client_commands.push((slot, format!("s {idx}")));
    Ok(Value::Undefined)
}

fn number(v: &Value) -> Result<f64, ErrorKind> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f as f64),
        _ => Err(ErrorKind::BadType("expected a number")),
    }
}

/// `trap_SendServerCommand(-1, 1, cmd)`: every client.
fn broadcast(host: &mut GameHost, cmd: String) {
    for slot in host.client_slots() {
        host.client_commands.push((slot, cmd.clone()));
    }
}

/// `musicPlay(alias)` (`0x5eb5c`): `o <alias>`
/// (docs/research/cod11-sound-system.md, "Per gsc call").
pub fn music_play(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(alias)) = args.first() else {
        return Err(ErrorKind::BadType("musicPlay needs an alias"));
    };
    let cmd = format!("o {}", cx.resolve(*alias));
    broadcast(host, cmd);
    Ok(Value::Undefined)
}

/// `musicStop([fadetime])` (`0x5eb8c`): `p <ms>`, the seconds times 1000
/// plus 0.5, truncated; 0 without an argument.
pub fn music_stop(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let ms = match args {
        [] => 0,
        [t] => (number(t)? * 1000.0 + 0.5) as i32,
        _ => return Err(ErrorKind::BadType("USAGE: musicStop([fadetime]);")),
    };
    if ms < 0 {
        return Err(ErrorKind::BadType("musicStop: fade time must be >= 0"));
    }
    broadcast(host, format!("p {ms}"));
    Ok(Value::Undefined)
}

/// `soundFade(volume [, time])` (`0x5ec40`): `q %f %i\n`, the time in
/// seconds times 1000, truncated, 0 without one. Extra arguments and a
/// negative time pass unchecked.
pub fn sound_fade(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let volume = number(
        args.first()
            .ok_or(ErrorKind::BadType("soundFade needs a volume"))?,
    )?;
    let ms = match args.get(1) {
        Some(t) => (number(t)? * 1000.0) as i32,
        None => 0,
    };
    broadcast(host, format!("q {volume:.6} {ms}\n"));
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;
    use vcod_common::net::protocol::CS_SOUNDS;
    use vcod_gsc::Host;

    /// `playSound` allocates a sound-alias configstring, mirroring
    /// `G_SoundAliasIndex`.
    #[test]
    fn playsound_allocates_a_sound_alias_configstring() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let a = Value::String(cx.intern_exact("minefield_click"));
            play_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.configstrings[525], "minefield_click");
        });
    }

    /// `playSound` on a player queues the event for that client's sim: the
    /// playerstate ring is the sim's, so the builtin cannot write it itself.
    #[test]
    fn playsound_on_a_player_queues_the_event_for_its_sim() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 3, None).unwrap();
            let a = Value::String(cx.intern_exact("MP_bomb_plant"));
            play_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.configstrings[525], "MP_bomb_plant");
            assert_eq!(
                host.client_sim_ops,
                vec![(
                    3,
                    crate::game::host::SimOp::Event {
                        event: EV_SOUND_ALIAS,
                        parm: 1,
                    }
                )]
            );
        });
    }

    /// `playSound` on a plain entity rides that entity's own event ring, and
    /// the ring reaches the wire: the slot is written first and the sequence
    /// after it, so the first event sits below sequence 1.
    #[test]
    fn playsound_on_an_entity_rides_its_own_event_ring() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            for (name, value) in [("classname", "script_model"), ("model", "xmodel/barrels")] {
                let atom = cx.intern_folded(name);
                let v = Value::String(cx.intern_exact(value));
                host.set_field(cx, e, atom, v).unwrap();
            }
            host.allocators
                .index(&mut host.configstrings, CsRange::Model, "xmodel/barrels")
                .unwrap();

            let a = Value::String(cx.intern_exact("Explo_plant_no_tick"));
            play_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert!(host.client_sim_ops.is_empty(), "not a client's playerstate");

            let p = &vcod_common::net::protocol::PROTOCOL_V1;
            let ents = crate::game::wire::packet_entities(&mut host, cx, p);
            let es = &ents[&e.0];
            assert_eq!(es.field_i32(p, "eventSequence"), 1);
            assert_eq!(es.field_i32(p, "events[0]"), EV_SOUND_ALIAS);
            let alias = host
                .configstrings
                .iter()
                .position(|s| s == "Explo_plant_no_tick")
                .unwrap();
            assert_eq!(es.field_i32(p, "eventParms[0]"), (alias - CS_SOUNDS) as i32);
        });
    }

    /// A `script_origin` rides no snapshot until `playSound` makes it
    /// broadcast, and every one after: retail's, `client-probes/
    /// probe_originsound` (sound doc, section 9).
    #[test]
    fn playsound_puts_a_script_origin_on_the_wire() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let atom = cx.intern_folded("classname");
            let v = Value::String(cx.intern_exact("script_origin"));
            host.set_field(cx, e, atom, v).unwrap();
            let p = &vcod_common::net::protocol::PROTOCOL_V1;
            assert!(!crate::game::wire::packet_entities(&mut host, cx, p).contains_key(&e.0));

            let a = Value::String(cx.intern_exact("MP_bomb_plant"));
            play_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            let ents = crate::game::wire::packet_entities(&mut host, cx, p);
            let es = &ents[&e.0];
            assert_eq!(es.field_i32(p, "eType"), 8);
            assert_eq!(es.field_i32(p, "events[0]"), EV_SOUND_ALIAS);
            assert!(host.ents.get(e).unwrap().broadcast);
        });
    }

    /// United Offensive's sound builtins stamp `r.broadcastTime` and leave
    /// `svFlags` alone: `playSound` 300 ms past the level time, so a
    /// `script_origin` stays off the wire; `playLoopSound` -1, for as long
    /// as it loops; `stopLoopSound` 300 ms again (game.mp.uo 0x68abb,
    /// 0x68b2e, 0x68b8d).
    #[test]
    fn uo_sounds_broadcast_for_a_while_not_for_good() {
        let (mut vm, mut host) = fixture();
        host.set_profile(&crate::game::profile::COD1_UO);
        host.level_time_ms = 5000;
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let atom = cx.intern_folded("classname");
            let v = Value::String(cx.intern_exact("script_origin"));
            host.set_field(cx, e, atom, v).unwrap();
            let a = Value::String(cx.intern_exact("MP_bomb_plant"));
            play_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            let ent = host.ents.get(e).unwrap();
            assert!(!ent.broadcast);
            assert_eq!(ent.broadcast_time, 5300);
            let p = &vcod_common::net::protocol::PROTOCOL_V22;
            assert!(!crate::game::wire::packet_entities(&mut host, cx, p).contains_key(&e.0));

            play_loop_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.ents.get(e).unwrap().broadcast_time, -1);
            host.level_time_ms = 6000;
            stop_loop_sound(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
            assert_eq!(host.ents.get(e).unwrap().broadcast_time, 6300);
        });
    }

    /// A `script_model` with no model rides every snapshot from its spawn,
    /// `eType` 8 and `index` 0, and carries its `playSound`: 1.5's and UO's
    /// minefields sound their explosion on one (`client-probes/
    /// probe_minefield`, sound doc, section 9).
    #[test]
    fn a_modelless_script_model_is_on_the_wire_and_sounds() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let atom = cx.intern_folded("classname");
            let v = Value::String(cx.intern_exact("script_model"));
            host.set_field(cx, e, atom, v).unwrap();
            let p = &vcod_common::net::protocol::PROTOCOL_V1;
            let ents = crate::game::wire::packet_entities(&mut host, cx, p);
            let es = &ents[&e.0];
            assert_eq!(es.field_i32(p, "eType"), 8);
            assert_eq!(es.field_i32(p, "index"), 0);

            let a = Value::String(cx.intern_exact("explo_mine"));
            play_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            let ents = crate::game::wire::packet_entities(&mut host, cx, p);
            assert_eq!(ents[&e.0].field_i32(p, "events[0]"), EV_SOUND_ALIAS);
        });
    }

    /// `playLoopSound` writes `es.loopSound` and it reaches the wire; the
    /// index counts from `CS_SOUNDS`, and `stopLoopSound` puts back 0 without
    /// touching the alias's slot.
    #[test]
    fn playloopsound_writes_the_loopsound_netfield() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            for (name, value) in [("classname", "script_model"), ("model", "xmodel/barrels")] {
                let atom = cx.intern_folded(name);
                let v = Value::String(cx.intern_exact(value));
                host.set_field(cx, e, atom, v).unwrap();
            }
            host.allocators
                .index(&mut host.configstrings, CsRange::Model, "xmodel/barrels")
                .unwrap();

            let a = Value::String(cx.intern_exact("bomb_tick"));
            play_loop_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.configstrings[525], "bomb_tick");

            let p = &vcod_common::net::protocol::PROTOCOL_V1;
            let ents = crate::game::wire::packet_entities(&mut host, cx, p);
            assert_eq!(ents[&e.0].field_i32(p, "loopSound"), 1);

            stop_loop_sound(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
            let ents = crate::game::wire::packet_entities(&mut host, cx, p);
            assert_eq!(ents[&e.0].field_i32(p, "loopSound"), 0);
            assert_eq!(host.configstrings[525], "bomb_tick");
        });
    }

    /// The music and fade builtins reach every client with retail's text,
    /// trailing newline on `q` included (`.rodata` 0x77510, 0x77563,
    /// 0x77568).
    #[test]
    fn music_and_fade_builtins_broadcast_retail_text() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            host.ents.spawn_client(cx, 0, None).unwrap();
            host.ents.spawn_client(cx, 2, None).unwrap();
            let a = Value::String(cx.intern_exact("music_victory"));
            music_play(&mut host, cx, None, &[a]).unwrap();
            music_stop(&mut host, cx, None, &[]).unwrap();
            music_stop(&mut host, cx, None, &[Value::Float(1.2345)]).unwrap();
            sound_fade(&mut host, cx, None, &[Value::Float(0.3), Value::Float(2.5)]).unwrap();
            sound_fade(&mut host, cx, None, &[Value::Int(1)]).unwrap();
            let to_two: Vec<&str> = host
                .client_commands
                .iter()
                .filter(|(slot, _)| *slot == 2)
                .map(|(_, c)| c.as_str())
                .collect();
            assert_eq!(
                to_two,
                [
                    "o music_victory",
                    "p 0",
                    "p 1235",
                    "q 0.300000 2500\n",
                    "q 1.000000 0\n"
                ]
            );
            assert_eq!(host.client_commands.len(), 10);
            assert!(music_stop(&mut host, cx, None, &[Value::Float(-1.0)]).is_err());
            assert!(music_stop(&mut host, cx, None, &[Value::Int(1), Value::Int(2)]).is_err());
        });
    }

    /// `playLocalSound` is `s <idx>` to the receiving client alone, and the
    /// index counts from `CS_SOUNDS` rather than from the alias range: the
    /// first alias travels as 1.
    #[test]
    fn playlocalsound_sends_s_idx_to_that_client_only() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 3, None).unwrap();
            let a = Value::String(cx.intern_exact("MP_announcer_allies_win"));
            play_local_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.configstrings[525], "MP_announcer_allies_win");
            assert_eq!(host.client_commands, vec![(3, "s 1".to_string())]);
            // A second call on the same alias reuses the slot.
            play_local_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.client_commands.len(), 2);
            assert_eq!(host.client_commands[1], (3, "s 1".to_string()));
            // Not a player: retail's own `ent->client` check.
            let plain = host.ents.spawn(cx).unwrap();
            assert!(play_local_sound(&mut host, cx, Some(Target::Entity(plain)), &[a]).is_err());
        });
    }
}
