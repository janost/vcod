//! A CoD2 level as the wire carries it (`vcod_cod2::net::fields`): each
//! client's playerstate with its HUD elements, the player entities
//! `BG_PlayerStateToEntityState` builds, and the object table's entities,
//! every one built in CoD2's own numbers ([`COD2_ENTITY_NUMBERS`],
//! cod2-protocol.md 14.1). Until the engine takes `EntityState2` they sit
//! in vcod's `EntityState` over 118's table
//! ([`vcod_cod2::net::Cod2Wire::as_cod1_shim`], identity maps), which
//! [`to_cod2_entity`] copies out.

use std::collections::BTreeMap;

use vcod_cod2::net::PROTOCOL_118;
use vcod_cod2::net::delta::{
    ClientState2, EntityState2, HudElem2, MAX_HUDELEMS, PlayerState2, field_index,
};
use vcod_cod2::net::events as c2ev;
use vcod_cod2::net::fields::{CLIENT_STATE_FIELDS, HUDELEM_FIELDS, PLAYER_STATE_FIELDS};
use vcod_cod2::pmove::{Stance, pm_type};
use vcod_common::net::cod1::flags as v1;
use vcod_common::net::msg::EntityState;
use vcod_gsc::{EntId, Value};

use super::cod2::Cod2Level;
use super::entity::{HUD_OWNER_ALL, HudState};
use super::fields::{Route, route_hud_cod2};
use super::host::GameHost;

/// What `game::wire` builds a CoD2 entity in (cod2-protocol.md 14.1): a
/// script model, script brush model, `script_origin` and brush mover 6
/// (CoD2 has no 1.1 `ET_MOVER`), a turret 9, a looped effect 8; `eFlags`
/// 0x2 the teleport bit, 0x20 `EF_NODRAW`, 0x40 the gun's firing bit
/// (12.1, 12.4); an item's 0, as every retail item carries; the ring's
/// events through [`event_to_cod2`] (15.2); the movers' accelerations 7 and
/// 8, not 1.1's 9 and 10 (12.2, [`tr_type_to_cod2`]).
pub static COD2_ENTITY_NUMBERS: super::wire::EntityNumbers = super::wire::EntityNumbers {
    et_item: c2ev::ET_ITEM,
    et_mover: c2ev::ET_SCRIPTMOVER,
    et_scriptmover: c2ev::ET_SCRIPTMOVER,
    et_fx: c2ev::ET_FX,
    et_turret: c2ev::ET_TURRET,
    ef_teleport: vcod_cod2::net::flags::EF_TELEPORT_BIT,
    ef_nodraw: EF_NODRAW,
    ef_firing: vcod_cod2::net::flags::EF_FIRING,
    item_eflags: 0,
    event: event_to_cod2,
    tr_type: tr_type_to_cod2,
};

/// CoD2's `EF_NODRAW`: every model handler skips the entity (12.1),
/// `G_ExplodeMissile` sets it (0x810d970).
const EF_NODRAW: i32 = 0x20;

/// A `trType` in vcod's (1.1's) numbering as CoD2's `BG_EvaluateTrajectory`
/// (0x80df5d4) numbers it: 1.1's two low gravities are gone, so gravity
/// paused, accelerate and decelerate move from 8, 9 and 10 to 6, 7 and 8.
/// CoD2's own move setup stores the accelerations as 7 and 8
/// (`ScriptMover_Move` 0x81193a8 through 0x8118c4a), and a client meeting
/// a type past 8 drops (`Com_Error(1, ..)`).
pub fn tr_type_to_cod2(t: i32) -> i32 {
    use vcod_common::net::trajectory as v1tr;
    match t {
        v1tr::TR_GRAVITY_PAUSED => vcod_cod2::trajectory::TR_GRAVITY_PAUSED,
        v1tr::TR_ACCELERATE => vcod_cod2::trajectory::TR_ACCELERATE,
        v1tr::TR_DECCELERATE => vcod_cod2::trajectory::TR_DECELERATE,
        t => t,
    }
}

/// `TR_INTERPOLATE` and `TR_LINEAR_STOP` in CoD2's numbering (12.2).
const TR_INTERPOLATE: i32 = 1;
const TR_LINEAR_STOP: i32 = 3;
/// What `g_smoothClients`' extrapolating arm (0x80f56f8) gives a player's
/// `pos.trDuration`.
const SMOOTH_DURATION_MS: i32 = 50;

impl Cod2Level {
    /// `slot`'s playerstate as its snapshot carries it: pmove's fields
    /// (`PlayerState::to_wire`), the slot, the weapons and ammo the
    /// script gave, the health in `stats`, and the HUD elements
    /// `HudElem_UpdateClient` copies in for `team`.
    pub fn wire_playerstate(&self, slot: usize, team: i32) -> Option<PlayerState2> {
        let p = self.players.get(slot)?.as_ref()?;
        let voted = self
            .rt
            .host
            .client_voted
            .get(slot)
            .copied()
            .unwrap_or(false);
        let own_ef = if voted { super::cod2_cmds::EF_VOTED } else { 0 };
        // A follower is sent its target's playerstate with the follow
        // flags, its own vote bit and its own non-archived HUD elements
        // (`SpectatorClientEndFrame`, `HudElem_UpdateClient(.., 2)`).
        // A killcam's copy is the archived record (`SV_GetArchivedClientInfo`).
        if let Some(f) = p.follow.filter(|f| {
            f.target != slot
                && (p.follow_copy.is_some()
                    || self
                        .players
                        .get(f.target)
                        .and_then(Option::as_ref)
                        .is_some_and(|t| t.follow.is_none()))
        }) {
            let mut w = match &p.follow_copy {
                Some(copy) => (**copy).clone(),
                None => self.wire_playerstate(f.target, f.team)?,
            };
            let (pm, ef) = (
                field_index(&PLAYER_STATE_FIELDS, "pm_flags"),
                field_index(&PLAYER_STATE_FIELDS, "eFlags"),
            );
            let mask = self.rt.host.client_no_spectate[slot];
            let (pm_flags, e_flags) =
                Cod2Level::follow_flags(w.f[pm], w.f[ef], own_ef, f.forced, mask);
            w.f[pm] = pm_flags;
            w.f[ef] = e_flags;
            w.hud_current = hud_elems(&self.rt.host, slot, team).1;
            return Some(w);
        }
        let mut w = p.ps.to_wire(&PlayerState2::default());
        let ef = field_index(&PLAYER_STATE_FIELDS, "eFlags");
        w.f[ef] = (w.f[ef] & !super::cod2_cmds::EF_VOTED) | own_ef;
        let mut set = |name: &str, v: u32| w.f[field_index(&PLAYER_STATE_FIELDS, name)] = v;
        let host = &self.rt.host;
        set("clientNum", slot as u32);
        // The turret's view lock and the cursor hint (cod2-turrets.md 4, 6).
        let g = &p.gunner;
        set("viewlocked", g.viewlocked as u32);
        set("viewlocked_entNum", g.viewlocked_ent);
        set("cursorHint", g.hint as u32);
        set("cursorHintString", g.hint_string as u32);
        let weapons = host.client_weapons.get(slot).copied().unwrap_or_default();
        set("weapon", u32::from(weapons.current));
        set("weapons[0]", weapons.held as u32);
        set("weapons[1]", (weapons.held >> 32) as u32);
        // Four slot bytes a word (`ps+0x554`), INFERRED from the layout.
        let pack = |s: &[u8]| s.iter().rev().fold(0u32, |a, &b| a << 8 | u32::from(b));
        set(
            "weaponslots[0]",
            pack(&weapons.slots[..weapons.slots.len().min(4)]),
        );
        if weapons.slots.len() > 4 {
            set(
                "weaponslots[4]",
                pack(&weapons.slots[4..weapons.slots.len().min(8)]),
            );
        }
        set("offHandIndex", u32::from(host.cod2().clients[slot].offhand));
        set(
            "viewmodelIndex",
            host.client_viewmodel.get(slot).copied().unwrap_or(0) as u32,
        );
        // `ClientEndFrame`'s use pass (0x8121bc6, the gunner's hint above)
        // runs in its playing arm only: a spectator keeps `ClientSpawn`'s
        // zeroed `cursorHintString` (cod2-protocol.md 15.6). A dead body's
        // pass resets it to -1 (cod2-turrets.md 14).
        if (pm_type::SPECTATOR..=pm_type::INTERMISSION).contains(&p.ps.pm_type) {
            set("cursorHintString", 0);
        }
        // `PM_Weapon`'s half of the playerstate.
        let ws = &p.ws;
        set("weaponstate", ws.state as u32);
        set("weaponTime", ws.weapon_time as u32);
        set("weaponDelay", ws.weapon_delay as u32);
        set("grenadeTimeLeft", ws.grenade_time_left as u32);
        set("weaponRestrictKickTime", ws.restrict_kick_time as u32);
        set("weapAnim", ws.weap_anim as u32);
        set("fWeaponPosFrac", ws.pos_frac.to_bits());
        set("holdBreathScale", ws.hold_breath_scale.to_bits());
        set("holdBreathTimer", ws.hold_breath_timer as u32);
        set("adsDelayTime", ws.ads_delay_time as u32);
        set("weaponrechamber[0]", ws.rechamber as u32);
        set("weaponrechamber[1]", (ws.rechamber >> 32) as u32);
        set("damageEvent", p.damage_event as u32 & 0xff);
        set("damageYaw", p.damage_yaw as u32 & 0xff);
        set("damagePitch", p.damage_pitch as u32 & 0xff);
        set("damageCount", p.damage_count as u32);
        if let Some(a) = host.client_ammo.get(slot) {
            for (to, from) in w.ammo.iter_mut().zip(a.ammo) {
                *to = i32::from(from);
            }
            for (to, from) in w.ammoclip.iter_mut().zip(a.clip) {
                *to = i32::from(from);
            }
        }
        // `ClientEndFrame` copies the entity's health into `stats[0]`
        // (0x80f7516, VERIFIED); max health in `stats[2]` is 1.1's layout,
        // INFERRED. `stats[3]` and `stats[4]` are the crosshair scan's
        // client and health, -1 (63 in six bits) and 0 for none
        // (cod2-hud.md 19.6); `stats[5]` is `ClientSpawn`'s count
        // (0x80f910e keeps it across its memset and adds one, 15.6).
        let v = self.rt.client_vitals(slot);
        if p.ps.pm_type < pm_type::SPECTATOR {
            w.stats[0] = v.health;
            w.stats[2] = v.max_health;
        }
        if p.ps.pm_type >= pm_type::DEAD {
            w.stats[1] = p.dead_yaw;
        }
        let identify = host.cod2().clients[slot].identify;
        w.stats[3] = identify.map_or(63, |(n, _)| i32::from(n));
        w.stats[4] = identify.map_or(0, |(_, h)| h);
        w.stats[5] = host.cod2().clients[slot].spawn_count;
        let (archived, current) = hud_elems(host, slot, team);
        w.hud_archival = archived;
        w.hud_current = current;
        if let Some(copy) = host.client_objectives.get(slot) {
            for (dst, o) in w.objectives.iter_mut().zip(copy) {
                *dst = objective_wire(o);
            }
        }
        Some(w)
    }

    /// The body `slot` puts in other clients' snapshots: `ClientEndFrame`'s
    /// `g_smoothClients` arm (0x80f56f8, the default), in vcod's
    /// `EntityState` over [`PROTOCOL_118`]'s table so the snapshot cull
    /// reads it. `None` for a client not playing, which links no body.
    pub fn player_entity(&self, slot: usize) -> Option<EntityState> {
        if !self.playing(slot) {
            return None;
        }
        let mut e = self.body_entity(slot)?;
        // `.headicon` and `.headiconteam` write the entity's own words
        // (`ClientScr_SetHeadIcon` 0x80f81d4 into `s+0x94`, the team into
        // `s+0x98`), which the playerstate copy leaves alone
        // (cod2-protocol.md 15.6).
        let (icon, team) = self.rt.head_icon(slot);
        for (name, v) in [("iHeadIcon", icon), ("iHeadIconTeam", team)] {
            if let Some(i) = EntityState::field_index(PROTOCOL_118.as_cod1_shim(), name) {
                e.fields[i] = v;
            }
        }
        Some(e)
    }

    /// [`Self::player_entity`] for any client in the world, the dead
    /// included: what `cloneplayer` copies. `None` for a spectator.
    pub fn body_entity(&self, slot: usize) -> Option<EntityState> {
        let ps = &self.players.get(slot)?.as_ref()?.ps;
        if matches!(ps.pm_type, pm_type::SPECTATOR | pm_type::INTERMISSION) {
            return None;
        }
        let p = PROTOCOL_118.as_cod1_shim();
        let mut e = EntityState::null(p);
        e.number = slot as u32;
        let mut seti = |name: &str, v: i32| {
            if let Some(i) = EntityState::field_index(p, name) {
                e.fields[i] = v;
            }
        };
        // `pm_flags` 0x800000 or 0x400000 is a body; [`Cod2Level::playing`]
        // is the first.
        seti("eType", v1::ET_PLAYER);
        seti("pos.trType", TR_LINEAR_STOP);
        seti("pos.trTime", ps.command_time);
        seti("pos.trDuration", SMOOTH_DURATION_MS);
        seti("apos.trType", TR_INTERPOLATE);
        // The snap (`param_3` 1) truncates both bases to whole units
        // toward zero: each `fistp` runs under the control word with 0xc00
        // or-ed in (0x80dfd2b, 0x80dfdc3). VERIFIED, the disassembly and
        // the hitpose capture's strafing target (1929.65 goes out 1929).
        // Through the integer, so -0.95 goes out +0 (`fild` of 0), as the
        // sd capture's spawn at z -0.953 did (cod2-protocol.md 15.7).
        let snap = |v: f32| v as i32 as f32;
        for k in 0..3 {
            seti(
                &format!("pos.trBase[{k}]"),
                snap(ps.origin[k]).to_bits() as i32,
            );
            seti(
                &format!("pos.trDelta[{k}]"),
                ps.velocity[k].to_bits() as i32,
            );
            seti(
                &format!("apos.trBase[{k}]"),
                snap(ps.view_angles[k]).to_bits() as i32,
            );
        }
        seti("angles2[1]", (ps.movement_dir as f32).to_bits() as i32);
        let mut eflags = ps.e_flags as i32;
        if ps.pm_type >= pm_type::DEAD {
            eflags |= 0x20000;
        } else {
            eflags &= !0x20000;
        }
        if ps.pm_flags & vcod_cod2::net::flags::PMF_SIGHT as u32 != 0 {
            eflags |= 0x40000;
        } else {
            eflags &= !0x40000;
        }
        seti("eFlags", eflags);
        if ps.pm_type < pm_type::DEAD {
            seti("solid", pack_solid(ps.mins, ps.maxs));
        }
        seti("clientNum", slot as i32);
        seti("groundEntityNum", ps.ground_entity_num & 0xffff);
        seti("leanf", ps.leanf.to_bits() as i32);
        // `BG_PlayerStateToEntityState` (0x80dfcc2) copies the anims pmove
        // and the death picked, toggles and all. VERIFIED, the stores.
        seti("legsAnim", ps.legs_anim);
        seti("torsoAnim", ps.torso_anim);
        let weapon = self
            .rt
            .host
            .client_weapons
            .get(slot)
            .map_or(0, |w| w.current);
        seti("weapon", i32::from(weapon));
        // The prone pitches; the lerp across a stance change is not done
        // (the factor reads 1).
        if ps.effective_stance() == Stance::Prone {
            seti("fTorsoHeight", ps.prone_valid[0].to_bits() as i32);
            seti("fTorsoPitch", ps.prone_valid[1].to_bits() as i32);
            seti("fWaistPitch", ps.prone_valid[2].to_bits() as i32);
        }
        // The entity's own ring, which `ClientEndFrame` feeds from the
        // playerstate's less the events kept for the client's own ears
        // (`BodyEvents::copy_from`).
        let ring = &self.players.get(slot)?.as_ref()?.body_events;
        seti("eventSequence", ring.sequence & 0xff);
        for k in 0..4 {
            seti(&format!("events[{k}]"), ring.events[k]);
            seti(&format!("eventParms[{k}]"), ring.parms[k]);
        }
        Some(e)
    }

    /// The frame's entities before any client's cull, in vcod's 1.1
    /// numbering over [`PROTOCOL_118`]'s table: the object table's and
    /// every playing client's body, with whether each is broadcast.
    pub fn world_entities(&mut self, svs: i32) -> BTreeMap<u32, (EntityState, bool)> {
        let level = self;
        level.rt.expire_broadcast_times(svs);
        // `hide` (`ScrCmd_Hide` 0x8112130) sets every client's bit in
        // `r.clientMask`, which keeps the entity out of every snapshot; its
        // `EF_NODRAW` (0x8109f5c) still rides the baseline
        // (cod2-protocol.md 15.6).
        let mut out: BTreeMap<u32, (EntityState, bool)> = level
            .rt
            .packet_entities(PROTOCOL_118.as_cod1_shim())
            .into_iter()
            .filter(|(n, _)| {
                let ents = &level.rt.host.ents;
                !ents
                    .handle(*n)
                    .and_then(|id| ents.get(id))
                    .is_some_and(|e| e.hidden)
            })
            .map(|(n, e)| (n, (e, level.rt.in_snapshot_broadcast(n))))
            .collect();
        for slot in 0..level.players.len() {
            if let Some(e) = level.player_entity(slot) {
                out.insert(slot as u32, (e, false));
            }
        }
        // The corpses, the temp entities (a broadcast one past the cull,
        // the rest by PVS and by their scope, `Cod2Level::snapshot_entities`)
        // and the missiles, which are `SVF_BROADCAST`.
        out.extend(
            level
                .rt
                .body_entities(PROTOCOL_118.as_cod1_shim())
                .map(|(n, e)| (n, (e, false))),
        );
        // A temp entity is raised in 1.1's numbering and built in CoD2's;
        // one CoD2 has no event for stays off the wire.
        let temps: Vec<(u32, EntityState, bool)> = level
            .rt
            .temp_entities()
            .filter_map(|(n, te)| {
                let broadcast = te.scope == crate::game::temp_entity::Scope::Broadcast;
                Some((n, temp_entity(te, n)?, broadcast))
            })
            .collect();
        out.extend(temps.into_iter().map(|(n, e, b)| (n, (e, b))));
        out.extend(
            level
                .missile_entities()
                .into_iter()
                .map(|(n, e)| (n, (e, true))),
        );
        out
    }

    /// `G_GetClientState` for `slot` named `name`: its team, body model
    /// and attachments as its end frame left them.
    pub fn client_state(&mut self, slot: usize, name: &str) -> ClientState2 {
        let mut cs = bare_client_state(slot, name);
        let idx = |n: &str| field_index(&CLIENT_STATE_FIELDS, n);
        cs.f[idx("team")] = self.rt.client_team(slot) as u32;
        cs.f[idx("modelindex")] = self.rt.client_model_index(slot) as u32;
        for (n, (m, tag)) in self.rt.client_attachments(slot).iter().take(6).enumerate() {
            cs.f[idx(&format!("attachModelIndex[{n}]"))] = *m as u32;
            cs.f[idx(&format!("attachTagIndex[{n}]"))] = *tag as u32;
        }
        cs
    }

    /// What `SV_BuildClientSnapshot` (inlined in `SV_SendClientSnapshot`, Mac 0x195f84) sends `slot` of
    /// `world` (the frame's entities, each with its broadcast flag): every
    /// entity but the one `ps.clientNum` names whose link reaches the PVS
    /// of the eye in `ps`, areas connected (`crate::world::in_snapshot`'s
    /// 1.5 rule; INFERRED for CoD2), renumbered for the wire. A client
    /// whose `archivetime` finds an archived frame is sent that frame's
    /// entities instead, their times shifted by its age, and that frame's
    /// roster (the second half, `None` for the live one); the lookup's
    /// trim is written back into `archivetime` (cod2-killcam.md 4).
    pub fn snapshot_entities(
        &mut self,
        world: &std::collections::BTreeMap<u32, (EntityState, bool)>,
        slot: usize,
        ps: &PlayerState2,
    ) -> (Vec<EntityState2>, Option<std::rc::Rc<Vec<ClientState2>>>) {
        let mut age = self.rt.client_archive_ms(slot);
        let frame = if self.rt.archive_on() && age > 0 {
            let found = self.archive.lookup(&mut age);
            if age != self.rt.client_archive_ms(slot) {
                self.rt.set_client_archive_ms(slot, age);
            }
            found.and_then(|i| self.archive.frame(i).cloned())
        } else {
            None
        };
        let client_num = ps.f[field_index(&PLAYER_STATE_FIELDS, "clientNum")];
        let viewer = client_num as usize;
        match frame {
            None => {
                // A temp entity's scope (all but the victim, the victim
                // alone), tested against `ps.clientNum`.
                let hidden: std::collections::HashSet<u32> = self
                    .rt
                    .temp_entities()
                    .filter(|(_, te)| !te.scope.admits(viewer))
                    .map(|(n, _)| n)
                    .collect();
                let ents = world.iter().map(|(n, (e, b))| (*n, e, *b));
                let ents = self.cull(ents, &hidden, client_num, ps, self.level_time(), 0);
                (ents, None)
            }
            Some(f) => {
                let hidden: std::collections::HashSet<u32> = f
                    .scopes
                    .iter()
                    .filter(|(_, scope)| !scope.admits(viewer))
                    .map(|(n, _)| *n)
                    .collect();
                let ents = f.entities.iter().map(|(n, e)| (*n, &e.0, e.1));
                let shift = self.level_time() - f.time;
                let ents = self.cull(ents, &hidden, client_num, ps, f.time, shift);
                (ents, Some(f.roster.clone()))
            }
        }
    }

    fn cull<'a>(
        &self,
        ents: impl Iterator<Item = (u32, &'a EntityState, bool)>,
        hidden: &std::collections::HashSet<u32>,
        client_num: u32,
        ps: &PlayerState2,
        time: i32,
        shift: i32,
    ) -> Vec<EntityState2> {
        let word = |n: &str| f32::from_bits(ps.f[field_index(&PLAYER_STATE_FIELDS, n)]);
        let eye = [
            word("origin[0]"),
            word("origin[1]"),
            word("origin[2]") + word("viewHeightCurrent"),
        ];
        let vis = &self.world.vis;
        ents.filter(|(n, _, _)| *n != client_num && !hidden.contains(n))
            .filter_map(|(_, e, broadcast)| Some((to_cod2_entity(e)?, broadcast)))
            .filter(|(e, broadcast)| {
                // 1.5's rule: an entity touching no cluster is not sent.
                *broadcast
                    || crate::world::link_in_snapshot(vis, eye, &link(vis, e, time), false, None)
            })
            .map(|(mut e, _)| {
                if shift != 0 {
                    super::cod2_archive::shift_entity(&mut e, shift);
                }
                e
            })
            .collect()
    }

    /// `SV_ArchiveSnapshot` (Mac 0x194da8) while `setarchive` is on: the
    /// frame's entities and roster as the snapshots read them, and the
    /// own view of every client `GetFollowPlayerState` answers for (in
    /// the world with `pm_flags` 0x800000), its current HUD half cleared.
    /// `SV_Frame` archives after every `G_RunFrame` and after the
    /// snapshots (cod2-killcam.md 2).
    pub fn archive_frame(
        &mut self,
        world: &std::collections::BTreeMap<u32, (EntityState, bool)>,
        roster: &[vcod_cod2::net::delta::ClientState2],
    ) {
        if !self.rt.archive_on() {
            return;
        }
        use super::cod2_archive::{ArchivedPs, Frame};
        let mut clients = Vec::with_capacity(self.players.len());
        for slot in 0..self.players.len() {
            let own = self.players[slot].as_ref().is_some_and(|p| {
                p.follow.is_none() && p.ps.pm_flags & super::cod2::PMF_PLAYING != 0
            }) && roster.iter().any(|cs| cs.number as usize == slot);
            let archived = own
                .then(|| {
                    let team = self.rt.client_team(slot);
                    let ps = self.wire_playerstate(slot, team)?;
                    Some(std::rc::Rc::new(ArchivedPs::new(&ps, team)))
                })
                .flatten();
            clients.push(archived);
        }
        let scopes = self
            .rt
            .temp_entities()
            .map(|(n, te)| (n, te.scope))
            .collect();
        let frame = Frame {
            time: self.level_time(),
            entities: world
                .iter()
                .map(|(n, e)| (*n, std::rc::Rc::new(e.clone())))
                .collect(),
            scopes,
            roster: std::rc::Rc::new(roster.to_vec()),
            clients,
        };
        self.archive.push(frame);
    }
}

/// A client state with no level behind it: a spectator with its name.
pub fn bare_client_state(slot: usize, name: &str) -> ClientState2 {
    let mut cs = ClientState2::null(slot as u32);
    cs.f[field_index(&CLIENT_STATE_FIELDS, "team")] = TEAM_SPECTATOR;
    let mut bytes = vcod_cod2::net::write::latin1_bytes(name);
    bytes.resize(32, 0);
    bytes[31] = 0;
    for k in 0..8 {
        let word = u32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().unwrap());
        cs.f[field_index(&CLIENT_STATE_FIELDS, &format!("name[{}]", k * 4))] = word;
    }
    cs
}

/// `TEAM_SPECTATOR` in a client state's `team`.
const TEAM_SPECTATOR: u32 = 3;

/// `SV_LinkEntity`'s packed box (0x809c63a) for a solid or body: `maxs.x`,
/// `1 - mins.z` and `32 + maxs.z`, each clamped to 1..255, VERIFIED (a
/// standing player reads 0x66010f live, cod2-protocol.md 15.6). Against
/// 1.1 the middle byte is `1 - mins.z`, not `-mins.z`.
fn pack_solid(mins: glam::Vec3, maxs: glam::Vec3) -> i32 {
    let b = |v: f32| (v as i32).clamp(1, 255);
    b(32.0 + maxs.z) << 16 | b(1.0 - mins.z) << 8 | b(maxs.x)
}

/// An entity of [`Cod2Level::world_entities`] as CoD2's wire sends it.
/// Every number in it is already CoD2's ([`COD2_ENTITY_NUMBERS`], the
/// bodies, the missiles, [`temp_entity`]), and vcod's `EntityState` over
/// [`PROTOCOL_118`]'s table is the same table, so this is a copy. `None`
/// for an `eType` CoD2's client has no arm for: past `ET_TURRET` below
/// `ET_EVENTS` (`CG_ProcessEntity`'s `Bad entity type`, 13), or an event
/// past its table.
pub fn to_cod2_entity(es: &EntityState) -> Option<EntityState2> {
    let mut out = EntityState2::null(es.number);
    for (to, from) in out.f.iter_mut().zip(&es.fields) {
        *to = *from as u32;
    }
    let t = out.i32("eType");
    let events = c2ev::ET_EVENTS..c2ev::ET_EVENTS + c2ev::EV_NAMES.len() as i32;
    ((0..=c2ev::ET_TURRET).contains(&t) || events.contains(&t)).then_some(out)
}

/// The temp entity `te` at `number` in CoD2's numbers: its event through
/// [`event_to_cod2`], `None` for one CoD2 has no event for.
pub fn temp_entity(te: &crate::game::temp_entity::TempEntity, number: u32) -> Option<EntityState> {
    let p = PROTOCOL_118.as_cod1_shim();
    let event = event_to_cod2(te.event)?;
    let mut e = crate::game::temp_entity::build(te, number, p);
    let etype = EntityState::field_index(p, "eType")?;
    e.fields[etype] = c2ev::ET_EVENTS + event;
    Some(e)
}

/// Where a CoD2 entity links, in its own numbers, and its clusters: a
/// script mover (1.1's brush movers are one too) at its trajectories
/// evaluated at `time` (CoD2's `BG_EvaluateTrajectory`), anything else at
/// its `trBase`s; a brush model's bounds, else its spawn's box by `eType`
/// (`game::wire::link_box_of`).
fn link(
    vis: &vcod_common::mapworld::Visibility,
    e: &EntityState2,
    time: i32,
) -> (vcod_common::mapworld::LinkClusters, [f32; 3], [f32; 3]) {
    use super::wire::LinkBox;
    use vcod_cod2::trajectory::Trajectory2;
    let etype = e.i32("eType");
    let base = |g: &str| [0, 1, 2].map(|k| e.f32(&format!("{g}.trBase[{k}]")));
    let (o, a) = if etype == c2ev::ET_SCRIPTMOVER {
        let at = |g: &str| Trajectory2::read(e, g).evaluate(time).to_array();
        (at("pos"), at("apos"))
    } else {
        (base("pos"), base("apos"))
    };
    let bmodel =
        (e.i32("solid") == vcod_common::net::flags::SOLID_BMODEL).then(|| e.i32("index") as usize);
    let class = match etype {
        c2ev::ET_TURRET => LinkBox::Turret,
        c2ev::ET_SCRIPTMOVER => LinkBox::ScriptMover,
        c2ev::ET_PLAYER | c2ev::ET_CORPSE => LinkBox::Body,
        _ => LinkBox::Point,
    };
    crate::world::link_at(vis, o, a, bmodel, super::wire::link_box_of(class))
}

/// An event in vcod's 1.1 numbering as CoD2's table numbers it
/// (`vcod_cod2::net::events`): the per-surface blocks and 139..143 sit at
/// the same numbers in both, the small and large bullet hits follow
/// cod2-efx.md 6.2, and every other event goes by its `EV_*` name
/// (cod2-protocol.md 15.2). `None` for one CoD2 has no event for, which
/// is not sent.
pub fn event_to_cod2(event: i32) -> Option<i32> {
    use vcod_cod2::net::events as c2;
    use vcod_common::net::event_ids as v1;
    if (1..=v1::EV_STEP_VIEW).contains(&event) {
        return Some(event);
    }
    // vcod names 181..183 by what the cgame plays (cod2-efx.md 6.2).
    match event {
        v1::EV_BULLET_HIT_SMALL => return Some(c2::EV_BULLET_HIT_SMALL),
        v1::EV_BULLET_HIT_LARGE => return Some(c2::EV_BULLET_HIT_LARGE),
        _ => {}
    }
    macro_rules! by_name {
        ($($n:ident),* $(,)?) => {
            match event {
                $(v1::$n => stringify!($n),)*
                _ => return None,
            }
        };
    }
    let name = by_name!(
        EV_WATER_TOUCH,
        EV_WATER_LEAVE,
        EV_ITEM_PICKUP,
        EV_ITEM_PICKUP_QUIET,
        EV_AMMO_PICKUP,
        EV_NOAMMO,
        EV_EMPTYCLIP,
        EV_RELOAD,
        EV_RELOAD_FROM_EMPTY,
        EV_RELOAD_START,
        EV_RELOAD_END,
        EV_RAISE_WEAPON,
        EV_PUTAWAY_WEAPON,
        EV_WEAPON_ALT,
        EV_PULLBACK_WEAPON,
        EV_FIRE_WEAPON,
        EV_FIRE_WEAPONB,
        EV_FIRE_WEAPON_LASTSHOT,
        EV_RECHAMBER_WEAPON,
        EV_EJECT_BRASS,
        EV_MELEE_SWIPE,
        EV_FIRE_MELEE,
        EV_MELEE_HIT,
        EV_MELEE_MISS,
        EV_FIRE_WEAPON_MG42,
        EV_FIRE_QUADBARREL_1,
        EV_FIRE_QUADBARREL_2,
        EV_BULLET_TRACER,
        EV_SOUND_ALIAS,
        EV_BULLET_HIT_CLIENT_SMALL,
        EV_BULLET_HIT_CLIENT_LARGE,
        EV_GRENADE_BOUNCE,
        EV_GRENADE_EXPLODE,
        EV_ROCKET_EXPLODE,
        EV_ROCKET_EXPLODE_NOMARKS,
        EV_MOLOTOV_EXPLODE,
        EV_MOLOTOV_EXPLODE_NOMARKS,
        EV_CUSTOM_EXPLODE,
        EV_CUSTOM_EXPLODE_NOMARKS,
        EV_RAILTRAIL,
        EV_BULLET,
        EV_PAIN,
        EV_CROUCH_PAIN,
        EV_DEATH,
        EV_DEBUG_LINE,
        EV_PLAY_FX,
        EV_PLAY_FX_DIR,
        EV_PLAY_FX_ON_TAG,
        EV_FLAMEBARREL_BOUNCE,
        EV_EARTHQUAKE,
        EV_DROPWEAPON,
        EV_ITEM_RESPAWN,
        EV_ITEM_POP,
        EV_PLAYER_TELEPORT_IN,
        EV_PLAYER_TELEPORT_OUT,
        EV_OBITUARY,
    );
    c2::EV_NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| i as i32)
}

/// `HudElem_UpdateClient` (0x8104882): the element pool walked from slot
/// 0, a team element kept for its team, an owned one for its owner, each
/// copied into the archived or the current array by its `archived` flag,
/// 31 a side. INFERRED, the walk and the filter.
pub fn hud_elems(
    host: &GameHost,
    slot: usize,
    team: i32,
) -> ([HudElem2; MAX_HUDELEMS], [HudElem2; MAX_HUDELEMS]) {
    let mut archived = [[0; HUDELEM_FIELDS.len()]; MAX_HUDELEMS];
    let mut current = archived;
    let (mut na, mut nc) = (0, 0);
    for (id, _) in host.ents.iter_hud_elems() {
        let Some(state) = host.ents.get(id).and_then(|e| e.hud) else {
            continue;
        };
        if state.elem_type == 0
            || (state.team != 0 && state.team != team)
            || (state.owner != HUD_OWNER_ALL && state.owner != slot as u32)
        {
            continue;
        }
        let (out, n) = if hud_i32(host, id, "archived") != 0 {
            (&mut archived, &mut na)
        } else {
            (&mut current, &mut nc)
        };
        if *n < MAX_HUDELEMS {
            out[*n] = build_hud_elem(host, id, &state);
            *n += 1;
        }
    }
    (archived, current)
}

/// vcod's element type (1.1's `he_type_t`) as CoD2 numbers it: the
/// player name, map name and game type strings take 3..5, so the shader
/// is 6 and the four timers and two clocks 7..12 (`HECmd_SetShader`
/// 0x8104022 stores 6, `HECmd_SetTimer` .. `HECmd_SetTenthsTimerUp` pass
/// 7..10 to 0x8104124, `HECmd_SetClock` and `HECmd_SetClockUp` 11 and 12
/// to 0x81041d8). A CoD2-only type (`setWaypoint`'s 13) is already CoD2's.
fn cod2_elem_type(t: i32) -> i32 {
    match t {
        3 => 6,
        4..=9 => t + 3,
        t => t,
    }
}

fn build_hud_elem(host: &GameHost, id: EntId, s: &HudState) -> HudElem2 {
    let mut w = [0u32; HUDELEM_FIELDS.len()];
    let mut set = |name: &str, v: u32| w[field_index(&HUDELEM_FIELDS, name)] = v;
    let f = |v: f32| v.to_bits();
    set("type", cod2_elem_type(s.elem_type) as u32);
    set("x", f(hud_f32(host, id, "x")));
    set("y", f(hud_f32(host, id, "y")));
    set("z", f(hud_f32(host, id, "z")));
    set("fontScale", f(hud_f32(host, id, "fontscale")));
    set("font", hud_i32(host, id, "font") as u32);
    // `alignx << 2 | aligny` and `horzalign << 3 | vertalign`
    // (`vcod_cod2::hudelem`).
    let org = (hud_i32(host, id, "alignx") << 2 | hud_i32(host, id, "aligny")) as u32;
    let screen = (hud_i32(host, id, "horzalign") << 3 | hud_i32(host, id, "vertalign")) as u32;
    set("alignOrg", org);
    set("alignScreen", screen);
    set("fromAlignOrg", org);
    set("fromAlignScreen", screen);
    set("color.rgba", hud_i32(host, id, "color") as u32);
    set("fromColor.rgba", s.from_color as u32);
    set("fadeStartTime", s.fade_start as u32);
    set("fadeTime", s.fade_ms as u32);
    set("label", hud_i32(host, id, "label") as u32);
    set("width", s.width as u32);
    set("height", s.height as u32);
    set("materialIndex", s.shader as u32);
    set("fromWidth", s.from_width as u32);
    set("fromHeight", s.from_height as u32);
    set("scaleStartTime", s.scale_start as u32);
    set("scaleTime", s.scale_ms as u32);
    set("fromX", f(s.from_x as f32));
    set("fromY", f(s.from_y as f32));
    set("moveStartTime", s.move_start as u32);
    set("moveTime", s.move_ms as u32);
    set("time", s.time as u32);
    set("duration", s.duration as u32);
    set("value", f(s.value));
    set("text", s.text as u32);
    set("sort", f(hud_f32(host, id, "sort")));
    set(
        "foreground",
        u32::from(hud_i32(host, id, "foreground") != 0),
    );
    w
}

fn hud_value(host: &GameHost, id: EntId, name: &str) -> Option<Value> {
    let Route::Engine { slot, .. } = route_hud_cod2(name) else {
        return None;
    };
    host.ents.get(id).map(|e| e.engine[slot])
}

fn hud_i32(host: &GameHost, id: EntId, name: &str) -> i32 {
    match hud_value(host, id, name) {
        Some(Value::Int(i)) => i,
        Some(Value::Float(f)) => f as i32,
        _ => 0,
    }
}

fn hud_f32(host: &GameHost, id: EntId, name: &str) -> f32 {
    match hud_value(host, id, name) {
        Some(Value::Int(i)) => i as f32,
        Some(Value::Float(f)) => f,
        _ => 0.0,
    }
}

/// One `objective_t` as CoD2's six netfield words (`OBJECTIVE_FIELDS`):
/// the origin's floats, the entity, the team and the icon.
fn objective_wire(o: &vcod_common::net::msg::Objective) -> vcod_cod2::net::delta::Objective2 {
    let mut w = vcod_cod2::net::delta::Objective2 {
        state: o.state as u32,
        ..Default::default()
    };
    for (i, f) in vcod_cod2::net::fields::OBJECTIVE_FIELDS.iter().enumerate() {
        w.f[i] = match f.name {
            "origin[0]" => o.origin[0] as u32,
            "origin[1]" => o.origin[1] as u32,
            "origin[2]" => o.origin[2] as u32,
            "icon" => o.icon as u32,
            "entNum" => o.ent_num as u32,
            "teamNum" => o.team_num as u32,
            _ => 0,
        };
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_events_go_out_in_cod2s_numbers() {
        use vcod_common::net::event_ids as v1;
        assert_eq!(event_to_cod2(v1::EV_SOUND_ALIAS), Some(179));
        assert_eq!(event_to_cod2(v1::EV_BULLET_HIT_SMALL), Some(182));
        assert_eq!(event_to_cod2(v1::EV_BULLET_HIT_CLIENT_LARGE), Some(186));
        assert_eq!(event_to_cod2(v1::EV_OBITUARY), Some(198));
        assert_eq!(event_to_cod2(v1::EV_PLAY_FX_DIR), None);
        // The rest go by name.
        use vcod_cod2::net::events as c2;
        for (from, to) in [
            (v1::EV_PLAY_FX_ON_TAG, "EV_PLAY_FX_ON_TAG"),
            (v1::EV_FIRE_WEAPON_MG42, "EV_FIRE_WEAPON_MG42"),
            (v1::EV_GRENADE_BOUNCE, "EV_GRENADE_BOUNCE"),
            (v1::EV_ITEM_PICKUP, "EV_ITEM_PICKUP"),
            (v1::EV_FOOTSTEP_RUN_BASE + 3, "EV_FOOTSTEP_RUN_CARPET"),
        ] {
            assert_eq!(c2::event_name(event_to_cod2(from).unwrap()), Some(to));
        }
    }

    /// The object table builds a CoD2 entity in CoD2's numbers: a hidden
    /// script model is `eType` 6 with `EF_NODRAW` 0x20, and its ring takes
    /// CoD2's events, one CoD2 lacks going out as none with its parm.
    #[test]
    fn a_cod2_entity_is_built_in_cod2s_numbers() {
        use vcod_common::net::event_ids as ev;
        use vcod_gsc::{Host, Value};
        let (mut vm, mut host) = crate::game::testing::fixture();
        host.set_profile(&crate::game::profile::COD2);
        let p = PROTOCOL_118.as_cod1_shim();
        let c = vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let atom = cx.intern_folded("classname");
            let v = Value::String(cx.intern_exact("script_model"));
            host.set_field(cx, e, atom, v).unwrap();
            let ent = host.ents.get_mut(e).unwrap();
            ent.hidden = true;
            ent.events.events[0] = ev::EV_FIRE_WEAPON_MG42;
            ent.events.parms[0] = 7;
            ent.events.events[1] = ev::EV_UO_OVERHEATING;
            ent.events.parms[1] = 3;
            ent.events.seq = 2;
            let ents = crate::game::wire::packet_entities(&mut host, cx, p);
            to_cod2_entity(&ents[&e.0]).unwrap()
        });
        assert_eq!((c.i32("eType"), c.i32("eFlags")), (6, 0x20));
        assert_eq!(
            (c.i32("events[0]"), c.i32("eventParms[0]")),
            (vcod_cod2::net::events::EV_FIRE_WEAPON_MG42, 7)
        );
        assert_eq!((c.i32("events[1]"), c.i32("eventParms[1]")), (0, 0));
    }

    /// The movers' accelerations go out as CoD2's 7 and 8, which its own
    /// move setup stores (0x8118c4a); 1.1's 9 and 10 drop a CoD2 client.
    #[test]
    fn tr_types_take_cod2s_numbers() {
        use vcod_common::net::trajectory as v1tr;
        assert_eq!(tr_type_to_cod2(v1tr::TR_ACCELERATE), 7);
        assert_eq!(tr_type_to_cod2(v1tr::TR_DECCELERATE), 8);
        assert_eq!(tr_type_to_cod2(v1tr::TR_GRAVITY_PAUSED), 6);
        for t in 0..=5 {
            assert_eq!(tr_type_to_cod2(t), t);
        }
    }

    /// A CoD2 `moveTo` with an acceleration and a deceleration goes out
    /// with `pos.trType` 7, then 3 (`TR_LINEAR_STOP`), then 8, as CoD2's
    /// `ScriptMover_Move` stores them (0x8118c4a).
    #[test]
    fn a_cod2_mover_ramps_in_cod2s_tr_types() {
        use vcod_gsc::{Host, Value};
        let (mut vm, mut host) = crate::game::testing::fixture();
        host.set_profile(&crate::game::profile::COD2);
        let p = PROTOCOL_118.as_cod1_shim();
        let types = vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let atom = cx.intern_folded("classname");
            let v = Value::String(cx.intern_exact("script_model"));
            host.set_field(cx, e, atom, v).unwrap();
            let ramp = crate::game::mover::Ramp::new(3.0, 1.0, 1.0);
            host.movers.move_to(
                e,
                0,
                glam::Vec3::ZERO,
                glam::Vec3::new(300.0, 0.0, 0.0),
                ramp,
            );
            [500, 1500, 2500].map(|t| {
                host.level_time_ms = t;
                let ents = crate::game::wire::packet_entities(&mut host, cx, p);
                to_cod2_entity(&ents[&e.0]).unwrap().i32("pos.trType")
            })
        });
        assert_eq!(types, [7, 3, 8]);
    }

    #[test]
    fn hud_types_take_cod2s_numbers() {
        assert_eq!(cod2_elem_type(1), 1);
        assert_eq!(cod2_elem_type(2), 2);
        assert_eq!(cod2_elem_type(3), 6);
        assert_eq!(cod2_elem_type(4), 7);
        assert_eq!(cod2_elem_type(9), 12);
        assert_eq!(cod2_elem_type(13), 13);
    }

    /// The wire takes a built entity whole, `eFlags` above its 24 bits
    /// included, and refuses an `eType` CoD2's client has no arm for.
    #[test]
    fn a_built_entity_goes_out_whole() {
        let p = PROTOCOL_118.as_cod1_shim();
        let mut e = EntityState::null(p);
        e.number = 80;
        let i = |n| EntityState::field_index(p, n).unwrap();
        e.fields[i("eType")] = c2ev::ET_MISSILE;
        e.fields[i("eFlags")] = 0x0100_0000;
        e.fields[i("scale")] = 4;
        let c = to_cod2_entity(&e).unwrap();
        assert_eq!(
            c.f,
            e.fields.iter().map(|&v| v as u32).collect::<Vec<_>>()[..]
        );
        e.fields[i("eType")] = c2ev::ET_EVENTS + c2ev::EV_OBITUARY;
        assert!(to_cod2_entity(&e).is_some());
        e.fields[i("eType")] = c2ev::ET_EVENTS + 199;
        assert!(to_cod2_entity(&e).is_none());
    }
}
