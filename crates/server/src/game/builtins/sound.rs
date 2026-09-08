//! Sound builtins. The three that take an alias allocate a sound-alias
//! configstring, mirroring `G_SoundAliasIndex`
//! (docs/research/cod11-sound-system.md: they share the 525-779 range).
//! `playSound` raises `EV_SOUND_ALIAS` on the receiver's own event ring,
//! `playLocalSound` reaches one client as the reliable command `s <idx>`, and
//! `playLoopSound` writes the `es.loopSound` netfield that `stopLoopSound`
//! clears.

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
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The alias's configstring slot, allocated on first use the way retail's
/// `G_SoundAliasIndex` allocates it.
fn alloc_alias(host: &mut GameHost, cx: &mut Cx, args: &[Value]) -> Result<usize, ErrorKind> {
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
const CS_SOUNDS: usize = 524;

/// `EV_SOUND_ALIAS`, what `G_PlaySoundAlias` appends
/// (docs/research/cod11-sound-system.md, section 9).
const EV_SOUND_ALIAS: i32 = 172;

/// `<ent> playSound(alias)`: `G_SoundAliasIndex` -> `G_PlaySoundAlias`. The
/// two rings that call chooses between are owned by different halves of the
/// server (sound doc, section 9): a client's is the sim's playerstate, so it
/// travels as a `SimOp`, and an entity's is the one `crate::game::wire`
/// writes into its snapshots.
pub fn play_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let parm = (alloc_alias(host, cx, args)? - CS_SOUNDS) as i32;
    let Some(is_client) = host.ents.get(id).map(|e| e.client.is_some()) else {
        return Ok(Value::Undefined);
    };
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
    let idx = (alloc_alias(host, cx, args)? - CS_SOUNDS) as i32;
    if let Some(ent) = host.ents.get_mut(id) {
        ent.loop_sound = idx;
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
    if let Some(ent) = host.ents.get_mut(id) {
        ent.loop_sound = 0;
    }
    Ok(Value::Undefined)
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
    let idx = alloc_alias(host, cx, args)? - CS_SOUNDS;
    host.client_commands.push((slot, format!("s {idx}")));
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;
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
