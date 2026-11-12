//! CoD2's `ClientCommand` past `say`, `score`, `mr` and `kill`: the cheats
//! behind `sv_cheats`, `where`, `setviewpos`, `give` and `take`, the
//! spectator follow, and the entity dumps (cod2-protocol.md 3.4.2). The
//! votes are `game::vote`'s; the engine routes them.

use glam::Vec3;
use vcod_cod2::pmove::pm_type;
use vcod_common::text::atoi;
use vcod_gsc::Host as _;

use super::cod2::{Cod2Level, PMF_PLAYING};
use super::host::WeaponOp;
use super::vote::Sends;

/// `ent->flags` (gentity+0x174) bits the cheats toggle.
pub const FL_GODMODE: u32 = 1;
pub const FL_DEMI_GODMODE: u32 = 2;
pub const FL_NOTARGET: u32 = 4;
/// `setStableMissile`'s bit, which `fire_rocket` copies onto the rocket
/// (cod2-combat.md 2.6).
pub const FL_STABLE_MISSILE: u32 = 0x2_0000;
/// What `ClientSpawn` (0x80f910e, the store at 0x80f9202) leaves in
/// `ent->flags`: the bits above go.
pub const FL_SPAWN: u32 = 0x1000;

/// `pm_flags` `SpectatorClientEndFrame` writes on a follow copy (Mac
/// 0x1a2bc6): 0x400000 the follow, 0x1000000 one the client may leave,
/// 0x2000000 free flight allowed. VERIFIED, the stores.
pub const PMF_FOLLOW: u32 = 0x40_0000;
const PMF_MAY_LEAVE: u32 = 0x100_0000;
const PMF_FREELOOK: u32 = 0x200_0000;
/// `eFlags` 0x100000: this client's vote is in (`Cmd_CallVote_f`).
pub const EF_VOTED: u32 = 0x10_0000;

/// `allowSpectateTeam("freelook", false)`'s bit in the spectate mask.
const NO_FREELOOK: u8 = 0x10;

/// What a client command sends and prints: the server commands, `None`
/// for everyone, and the lines for the server console (`Com_Printf`).
#[derive(Debug, Default)]
pub struct CmdOut {
    pub sends: Sends,
    pub prints: Vec<String>,
}

impl CmdOut {
    fn reply(slot: usize, text: String) -> CmdOut {
        CmdOut {
            sends: vec![(Some(slot), text)],
            prints: Vec::new(),
        }
    }
}

/// The follow `SpectatorClientEndFrame` landed this frame: the client
/// whose playerstate the spectator is sent, its team, and whether the
/// script forced it (`spectatorclient`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowView {
    pub target: usize,
    pub team: i32,
    pub forced: bool,
}

/// `vtos`: the vector as `(%i %i %i)`, each truncated.
fn vtos(v: [f32; 3]) -> String {
    format!("({} {} {})", v[0] as i32, v[1] as i32, v[2] as i32)
}

impl Cod2Level {
    /// `ClientCommand` (Mac 0x1a7684, lnxded 0x8100e62) for the commands
    /// after `kill`, and `give` .. `ufo` before it; `argv` is the whole
    /// command. Anything unknown is answered
    /// `GAME_UNKNOWNCLIENTCOMMAND`. VERIFIED, the compare chain.
    pub fn client_command(&mut self, slot: usize, argv: &[String]) -> CmdOut {
        let cmd = argv.first().map_or("", String::as_str);
        let lower = cmd.to_ascii_lowercase();
        let toggle = |level: &mut Cod2Level, bit: u32, on: &str, off: &str| {
            if let Err(e) = level.cheats_ok(slot) {
                return e;
            }
            let flags = &mut level.rt.host.cod2_mut().clients[slot].flags;
            *flags ^= bit;
            let m = if *flags & bit != 0 { on } else { off };
            CmdOut::reply(slot, format!("e \"{m}\""))
        };
        match lower.as_str() {
            "give" => self.give(slot, argv),
            "take" => self.take(slot, argv),
            "god" => toggle(self, FL_GODMODE, "GAME_GODMODE_ON", "GAME_GODMODE_OFF"),
            "demigod" => toggle(
                self,
                FL_DEMI_GODMODE,
                "GAME_DEMI_GODMODE_ON",
                "GAME_DEMI_GODMODE_OFF",
            ),
            "notarget" => toggle(self, FL_NOTARGET, "GAME_NOTARGETON", "GAME_NOTARGETOFF"),
            // `gclient+0x27ac` and `+0x27b0`, which `ClientEndFrame` turns
            // into `pm_type` 2 and 3 (Mac 0x1a47a8).
            "noclip" | "ufo" => {
                if let Err(e) = self.cheats_ok(slot) {
                    return e;
                }
                let noclip = lower == "noclip";
                let state = &mut self.rt.host.cod2_mut().clients[slot];
                let flag = if noclip {
                    &mut state.noclip
                } else {
                    &mut state.ufo
                };
                *flag = !*flag;
                let m = match (noclip, *flag) {
                    (true, true) => "GAME_NOCLIPON",
                    (true, false) => "GAME_NOCLIPOFF",
                    (false, true) => "GAME_UFOON",
                    (false, false) => "GAME_UFOOFF",
                };
                CmdOut::reply(slot, format!("e \"{m}\""))
            }
            "follownext" => {
                self.follow_cycle(slot, 1);
                CmdOut::default()
            }
            "followprev" => {
                self.follow_cycle(slot, -1);
                CmdOut::default()
            }
            // The entity's `r.currentOrigin` (lnxded 0x80ffb5c).
            "where" => {
                let o = self.rt.client_origin(slot);
                CmdOut::reply(slot, format!("e \"\u{15}{}\n\"", vtos(o)))
            }
            "setviewpos" => self.set_viewpos(slot, argv),
            // A server console line under `sv_cheats`, nothing to the
            // client (lnxded 0x8100cec).
            "entitycount" => CmdOut {
                sends: Vec::new(),
                prints: if self.cheats() {
                    vec![format!(
                        "entity count = {}",
                        self.rt.host.ents.num_entities()
                    )]
                } else {
                    Vec::new()
                },
            },
            "printentities" => CmdOut {
                sends: Vec::new(),
                prints: self.print_entities(),
            },
            _ => CmdOut::reply(slot, format!("e \"GAME_UNKNOWNCLIENTCOMMAND\u{15}{cmd}\"")),
        }
    }

    /// `sv_cheats`, the dvar `g_cheats` reads (lnxded 0x8111795).
    fn cheats(&self) -> bool {
        atoi(self.rt.host.cvars.get("sv_cheats")) != 0
    }

    /// `CheatsOk` (Mac 0x1a5190): `sv_cheats` on and the entity's health
    /// above 0, else the refusal.
    fn cheats_ok(&self, slot: usize) -> Result<(), CmdOut> {
        if !self.cheats() {
            return Err(CmdOut::reply(slot, "e \"GAME_CHEATSNOTENABLED\"".into()));
        }
        if self.rt.client_vitals(slot).health <= 0 {
            return Err(CmdOut::reply(slot, "e \"GAME_MUSTBEALIVECOMMAND\"".into()));
        }
        Ok(())
    }

    /// `G_PrintEntities` (Mac 0x1b5b18): every slot below
    /// `level.num_entities`, its classname and origin.
    fn print_entities(&mut self) -> Vec<String> {
        let n = self.rt.host.ents.num_entities();
        (0..n)
            .map(|i| {
                let (class, o) = match self.rt.host.ents.handle(i) {
                    Some(ent) => {
                        let class = self.rt.field_str(ent, "classname");
                        let o = self.rt.with_host(|h, cx| {
                            let atom = cx.intern_folded("origin");
                            match h.get_field(cx, ent, atom) {
                                vcod_gsc::Value::Vector(v) => v,
                                _ => [0.0; 3],
                            }
                        });
                        (class, o)
                    }
                    None => (String::new(), [0.0; 3]),
                };
                format!(
                    "{i:4}: '{class}', origin: {:.6} {:.6} {:.6}",
                    o[0], o[1], o[2]
                )
            })
            .collect()
    }

    /// `Cmd_SetViewpos_f` (lnxded 0x8100b88): `sv_cheats` only, no alive
    /// test; four arguments, the eye at x y z looking along yaw.
    /// `TeleportPlayer` (0x810a708) lifts the origin 1, flips the teleport
    /// bit and turns the view. VERIFIED.
    fn set_viewpos(&mut self, slot: usize, argv: &[String]) -> CmdOut {
        if !self.cheats() {
            return CmdOut::reply(slot, "e \"GAME_CHEATSNOTENABLED\"".into());
        }
        if argv.len() != 5 {
            return CmdOut::reply(slot, "e \"GAME_USAGE\u{15}: setviewpos x y z yaw\"".into());
        }
        let f = |i: usize| crate::game::builtins::cvar::atof(&argv[i]);
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return CmdOut::default();
        };
        let origin = Vec3::new(f(1), f(2), f(3) - p.ps.view_height_current + 1.0);
        self.teleport(slot, origin, [0.0, f(4), 0.0]);
        CmdOut::default()
    }

    /// `TeleportPlayer`'s playerstate half: the origin, the teleport bit,
    /// the view.
    fn teleport(&mut self, slot: usize, origin: Vec3, angles: [f32; 3]) {
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        p.ps.origin = origin;
        p.ps.e_flags ^= vcod_cod2::net::flags::EF_TELEPORT_BIT as u32;
        super::cod2::set_view_angles(&mut p.ps, &p.oldcmd, angles);
        self.rt.set_client_origin(slot, origin.to_array());
    }

    /// The count `give` and `take` read: their arguments past the first,
    /// joined, through `atoi`; and the whole argument line.
    fn give_args(argv: &[String]) -> (i32, String) {
        let count = atoi(&argv.get(2..).unwrap_or(&[]).join(" "));
        (count, argv.get(1..).unwrap_or(&[]).join(" "))
    }

    /// `Cmd_Give_f` (Mac 0x1a6ac0, lnxded 0x80fe588): `all`, `health`
    /// (to the maximum, or by the count), `weapons`, `ammo` (998 of each
    /// weapon's with a full clip, or the count onto the weapon in hand),
    /// `allammo <n>`, or an item or weapon by name. VERIFIED, the branch
    /// order.
    fn give(&mut self, slot: usize, argv: &[String]) -> CmdOut {
        if let Err(e) = self.cheats_ok(slot) {
            return e;
        }
        let (count, line) = Self::give_args(argv);
        if line.is_empty() {
            return CmdOut::default();
        }
        let is = |k: &str| line.eq_ignore_ascii_case(k);
        let starts = |k: &str| {
            line.len() >= k.len() && line.as_bytes()[..k.len()].eq_ignore_ascii_case(k.as_bytes())
        };
        let all = is("all");
        if all || starts("health") {
            let v = &mut self.rt.host.client_vitals[slot];
            v.health = if count == 0 {
                v.max_health
            } else {
                v.health + count
            };
        }
        if all || is("weapons") {
            self.give_all_weapons(slot);
        }
        if all || starts("ammo") {
            let weapons = self.rt.host.weapons.clone();
            if count == 0 {
                for i in 1..weapons.defs().len() {
                    self.add_ammo(slot, i, 998, true);
                }
            } else {
                let current = self.rt.host.client_weapons[slot].current;
                if current != 0 {
                    self.add_ammo(slot, usize::from(current), count, true);
                }
            }
        }
        if all {
            return CmdOut::default();
        }
        if starts("allammo") && count != 0 {
            let n = self.rt.host.weapons.defs().len();
            for i in 1..n {
                self.add_ammo(slot, i, count, true);
            }
        } else if !starts("health") && !is("weapons") && !starts("ammo") {
            // `G_FindItem`: a weapon by name, given as its pickup gives it.
            if let Some(i) = self.rt.host.weapon_index(&line) {
                let weapons = self.rt.host.weapons.clone();
                self.rt.host.client_weapons[slot].give_chain(&weapons, i, weapons.slot(i));
                let start = weapons.get(i).map_or(0, |d| d.start_ammo);
                self.add_ammo(slot, i, start as i32, true);
            }
        }
        CmdOut::default()
    }

    /// `give weapons` (0x80fe588): both slot weapons taken, then every
    /// registered weapon in index order through `G_GivePlayerWeapon`, a
    /// slot weapon (`weaponSlot` 1 or 2, 0x80ebeb6) only past the old
    /// second slot's index and while a slot is free (0x80ebef2), then the
    /// slot weapons from 1 again while one is. VERIFIED, the loops.
    fn give_all_weapons(&mut self, slot: usize) {
        let weapons = self.rt.host.weapons.clone();
        let n = self.rt.host.cod2().weapons.len();
        let is_slot = |i: usize| matches!(weapons.slot(i), 1 | 2);
        let w = &mut self.rt.host.client_weapons[slot];
        let second = usize::from(w.slots[2]);
        for taken in [w.slots[1], w.slots[2]] {
            let taken = usize::from(taken);
            if taken != 0 && w.holds(taken) {
                w.take_chain(&weapons, taken);
                // `BG_TakePlayerWeapon` clears `ps.weapon` when it held it.
                if usize::from(w.current) == taken {
                    w.current = 0;
                }
            }
        }
        let free = |w: &crate::weapons::PlayerWeapons| w.slots[1] == 0 || w.slots[2] == 0;
        for i in 1..=n {
            let w = &self.rt.host.client_weapons[slot];
            if !is_slot(i) || (second < i && free(w)) {
                self.give_player_weapon(slot, i);
            }
        }
        for i in 1..=n {
            if !free(&self.rt.host.client_weapons[slot]) {
                break;
            }
            if is_slot(i) {
                self.give_player_weapon(slot, i);
            }
        }
    }

    /// `G_GivePlayerWeapon` (Mac 0x1c930a, lnxded 0x8120b26): nothing for
    /// a weapon held or one of `weaponClass` turret or non-player (7, 8);
    /// an `item` (9, the binoculars) only held; an offhand weapon held and
    /// through the offhand arm ([`super::builtins::cod2::give_offhand`]);
    /// any other held with its `altWeapon` modes, a slot weapon in the
    /// first empty of the two slots. VERIFIED, the branches and the class
    /// table (0x8186a20).
    fn give_player_weapon(&mut self, slot: usize, i: usize) {
        let weapons = self.rt.host.weapons.clone();
        let Some(def) = weapons.get(i) else {
            return;
        };
        let class = def.weapon_class.as_str();
        if matches!(class, "turret" | "non-player") {
            return;
        }
        let host = &mut self.rt.host;
        let offhand = host.cod2().offhands.get(i - 1).copied().unwrap_or(0) != 0;
        let w = &mut host.client_weapons[slot];
        if w.holds(i) {
            return;
        }
        if class == "item" || offhand {
            w.give(i, 0);
            if class != "item" {
                super::builtins::cod2::give_offhand(host, slot, i);
            }
            return;
        }
        let before = w.slots;
        w.give_chain(&weapons, i, 0);
        w.slots = before;
        if matches!(weapons.slot(i), 1 | 2)
            && let Some(s) = crate::game::pickup::empty_slot_for(w, 1)
        {
            w.slots[s] = i as u8;
        }
    }

    /// `Add_Ammo` (Mac 0x1b0e30): `count` into the reserve, a clip-only
    /// weapon given first (`G_GivePlayerWeapon`), the clip filled from the
    /// reserve when `fill_clip` or clip-only, both capped, then the shared
    /// cap. VERIFIED, and the arithmetic is 1.1's (`pickup::add_ammo`).
    fn add_ammo(&mut self, slot: usize, weapon: usize, count: i32, fill_clip: bool) {
        let weapons = self.rt.host.weapons.clone();
        if weapons.get(weapon).is_some_and(|d| d.clip_only) {
            self.give_player_weapon(slot, weapon);
        }
        let host = &mut self.rt.host;
        let before = super::item::inventory(host, slot);
        let mut after = before;
        crate::game::pickup::add_ammo(&mut after, &weapons, weapon, count, fill_clip);
        super::item::write_back(host, slot, &before, &after);
    }

    /// `Cmd_Take_f` (Mac 0x1a701a, lnxded 0x80fe9ca): `all`, `health`
    /// (to 1, or by the count down to 1), `weapons` (every weapon and its
    /// rounds, the hand emptied), `ammo` (every round, or the count off
    /// the weapon in hand, the clip taking what the reserve lacks), and
    /// `allammo <n>`. VERIFIED.
    fn take(&mut self, slot: usize, argv: &[String]) -> CmdOut {
        if let Err(e) = self.cheats_ok(slot) {
            return e;
        }
        let (count, line) = Self::give_args(argv);
        if line.is_empty() {
            return CmdOut::default();
        }
        let is = |k: &str| line.eq_ignore_ascii_case(k);
        let starts = |k: &str| {
            line.len() >= k.len() && line.as_bytes()[..k.len()].eq_ignore_ascii_case(k.as_bytes())
        };
        let all = is("all");
        let n = self.rt.host.weapons.defs().len();
        if all || starts("health") {
            let v = &mut self.rt.host.client_vitals[slot];
            v.health = if count == 0 {
                1
            } else {
                (v.health - count).max(1)
            };
        }
        if all || is("weapons") {
            for i in 1..n {
                self.rt.host.client_weapons[slot].take(i);
                self.set_rounds(slot, i, Some(0), Some(0));
            }
            self.rt.host.client_weapons[slot].current = 0;
        }
        if all || starts("ammo") {
            if count == 0 {
                for i in 1..n {
                    self.set_rounds(slot, i, Some(0), Some(0));
                }
            } else {
                let current = usize::from(self.rt.host.client_weapons[slot].current);
                if current != 0 {
                    self.take_rounds(slot, current, count);
                }
            }
        }
        if !all && starts("allammo") && count != 0 {
            for i in 1..n {
                self.take_rounds(slot, i, count);
            }
        }
        CmdOut::default()
    }

    /// `count` rounds off the reserve, the clip covering what it lacks,
    /// neither below 0.
    fn take_rounds(&mut self, slot: usize, weapon: usize, count: i32) {
        let Some(def) = self.rt.host.weapons.get(weapon).cloned() else {
            return;
        };
        let Some(a) = self.rt.host.client_ammo.get(slot) else {
            return;
        };
        let ammo = i32::from(a.ammo[def.ammo_index]) - count;
        if ammo >= 0 {
            self.set_rounds(slot, weapon, Some(ammo), None);
        } else {
            let clip = (i32::from(a.clip[def.clip_index]) + ammo).max(0);
            self.set_rounds(slot, weapon, Some(0), Some(clip));
        }
    }

    fn set_rounds(&mut self, slot: usize, weapon: usize, ammo: Option<i32>, clip: Option<i32>) {
        let Some(def) = self.rt.host.weapons.get(weapon).cloned() else {
            return;
        };
        if let Some(rounds) = ammo {
            self.rt.host.weapon_op(
                slot,
                WeaponOp::SetAmmo {
                    ammo_index: def.ammo_index,
                    rounds: rounds as i16,
                },
            );
        }
        if let Some(rounds) = clip {
            self.rt.host.weapon_op(
                slot,
                WeaponOp::SetClip {
                    clip_index: def.clip_index,
                    rounds: rounds as i16,
                },
            );
        }
    }

    /// The spectator's own `sessionstate` and the script's forced follow
    /// (`spectatorclient`, gclient+0x26ac) and spectate mask.
    fn spectator_session(&mut self, slot: usize) -> Option<(i32, u8)> {
        let s = self.rt.client_session(slot)?;
        (s.state == crate::follow::SessionState::Spectator).then_some((s.spectator_client, {
            self.rt.host.client_no_spectate[slot]
        }))
    }

    /// Whether `slot` may follow `target`: `SV_GetArchivedClientInfo` at no
    /// delay (Mac 0x195a06) wants the target in the world with `pm_flags`
    /// 0x800000, the mark `ClientEndFrame` leaves on a playing client
    /// (`GetFollowPlayerState`), and `G_ClientCanSpectateTeam` its team.
    /// VERIFIED.
    fn followable(&mut self, mask: u8, target: usize) -> bool {
        let active = self
            .rt
            .host
            .client_roster
            .get(target)
            .and_then(Option::as_ref)
            .is_some_and(|r| r.active);
        let playing = self
            .players
            .get(target)
            .and_then(Option::as_ref)
            .is_some_and(|p| p.ps.pm_flags & PMF_PLAYING != 0);
        active && playing && crate::follow::can_spectate(mask, self.rt.client_team(target))
    }

    /// `Cmd_FollowCycle_f` (Mac 0x1a53b8, lnxded 0x80ff4ac): a spectator
    /// not forced onto anyone steps from the client it follows (slot 0
    /// for none) by `dir`, wrapping at `sv_maxclients`, to the first it
    /// may follow, the start tried last. VERIFIED.
    pub fn follow_cycle(&mut self, slot: usize, dir: i32) -> bool {
        let Some((forced, mask)) = self.spectator_session(slot) else {
            return false;
        };
        if forced >= 0 {
            return false;
        }
        let Some(current) = self.players.get(slot).and_then(Option::as_ref) else {
            return false;
        };
        let current = usize::try_from(current.spectator_client).ok();
        let n = self.max_clients();
        let mut ok = vec![false; n];
        for (t, ok) in ok.iter_mut().enumerate() {
            *ok = self.followable(mask, t);
        }
        let Some(t) = crate::follow::cycle(current, dir, n, |t| ok[t]) else {
            return false;
        };
        if let Some(p) = self.players[slot].as_mut() {
            p.spectator_client = t as i32;
        }
        true
    }

    /// `SpectatorThink`'s buttons (Mac 0x1a2a08): melee pressed or let go
    /// stops a follow the client may leave, attack's press cycles forward
    /// and the sight button's back. True when the spectator flies this cmd,
    /// false while it follows. VERIFIED.
    pub fn spectator_buttons(&mut self, slot: usize, buttons: i32, old: i32) -> bool {
        let Some((forced, mask)) = self.spectator_session(slot) else {
            return true;
        };
        let following = self
            .players
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|p| p.spectator_client >= 0);
        if forced < 0 && mask & NO_FREELOOK == 0 && following && (buttons ^ old) & 4 != 0 {
            self.stop_following(slot);
        }
        let pressed = buttons & !old;
        if pressed & 1 != 0 {
            self.follow_cycle(slot, 1);
        } else if pressed & 0x1000 != 0 {
            self.follow_cycle(slot, -1);
        }
        self.players
            .get(slot)
            .and_then(Option::as_ref)
            .is_none_or(|p| p.follow.is_none())
    }

    /// `StopFollowing` (Mac 0x1a686c, lnxded 0x80ff252): the follow
    /// dropped; a spectator the last end frame put on a copy is left 40
    /// behind and 10 above the followed eye, swept with an 8-unit capsule,
    /// looking 15 lower. VERIFIED (`-40`, `10`, `15` read off the
    /// constants).
    pub fn stop_following(&mut self, slot: usize) {
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        p.spectator_client = -1;
        let copy = p.follow_copy.take();
        let Some(f) = p.follow.take() else {
            return;
        };
        // The eye of the copy the last end frame left, archived or live.
        let followed = match copy {
            Some(w) => vcod_cod2::pmove::PlayerState::from_wire(&w),
            None => match self.players.get(f.target).and_then(Option::as_ref) {
                Some(t) => t.ps.clone(),
                None => return,
            },
        };
        let eye = Cod2Level::view_origin(&followed).to_array();
        let (spot, view) =
            crate::follow::stop_spot(Some(&self.world.collision), eye, followed.view_angles);
        self.teleport_spectator(slot, Vec3::from(spot), view);
    }

    fn teleport_spectator(&mut self, slot: usize, origin: Vec3, view: [f32; 3]) {
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        p.ps.origin = origin;
        p.ps.velocity = Vec3::ZERO;
        p.ps.e_flags &= !0x300;
        p.ps.pm_flags &= !(PMF_FOLLOW | 0x40);
        super::cod2::set_view_angles(&mut p.ps, &p.oldcmd, view);
        self.rt.set_client_origin(slot, origin.to_array());
    }

    /// `SpectatorClientEndFrame` (Mac 0x1a2bc6): a forced follow takes
    /// the script's client out of the archive `archivetime -
    /// psoffsettime` back, the age 50 ms younger each time it finds
    /// nothing down to 0, where the live client answers; one that never
    /// does writes -1 to `spectatorclient`. With free flight refused a
    /// spectator not following cycles forward; a follow whose target
    /// answers at `archivetime + psoffsettime` copies it (the wire half is
    /// [`Self::wire_playerstate`]), any other stops. VERIFIED, the stores
    /// (cod2-killcam.md 3).
    pub fn spectator_end_frame(&mut self, slot: usize) {
        let Some((forced, mask)) = self.spectator_session(slot) else {
            if let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) {
                p.spectator_client = -1;
                p.follow = None;
                p.follow_copy = None;
            }
            return;
        };
        let offset = self
            .rt
            .client_field(slot, "psoffsettime")
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);
        if let Ok(t) = usize::try_from(forced) {
            if let Some(p) = self.players[slot].as_mut() {
                p.spectator_client = forced;
            }
            loop {
                let mut archive = self.rt.client_archive_ms(slot);
                if archive < 0 {
                    archive = 0;
                    self.rt.set_client_archive_ms(slot, 0);
                }
                if let Some((copy, team)) = self.client_info(t, archive - offset)
                    && crate::follow::can_spectate(mask, team)
                {
                    self.land_follow(slot, t, team, true, copy);
                    return;
                }
                if archive == 0 {
                    break;
                }
                self.rt.set_client_archive_ms(slot, archive - 50);
            }
            self.rt.set_client_spectator_client(slot, -1);
            if let Some(p) = self.players[slot].as_mut() {
                p.spectator_client = -1;
            }
        }
        let following = self.players[slot]
            .as_ref()
            .is_some_and(|p| p.spectator_client >= 0);
        if !following && mask & NO_FREELOOK != 0 {
            self.follow_cycle(slot, 1);
        }
        let target = self.players[slot]
            .as_ref()
            .and_then(|p| usize::try_from(p.spectator_client).ok());
        if let Some(t) = target {
            let age = self.rt.client_archive_ms(slot) + offset;
            if let Some((copy, team)) = self.client_info(t, age)
                && crate::follow::can_spectate(mask, team)
            {
                self.land_follow(slot, t, team, false, copy);
                return;
            }
        }
        self.stop_following(slot);
    }

    /// `SV_GetArchivedClientInfo` (Mac 0x195a06) for `target` at `age`:
    /// the archived record with its team, or with no archived frame and
    /// an age of 0 or less the live client if it is active and playing
    /// (`None` inside for a live copy).
    fn client_info(
        &mut self,
        target: usize,
        mut age: i32,
    ) -> Option<(Option<Box<vcod_cod2::net::PlayerState2>>, i32)> {
        use crate::game::cod2_archive::ClientInfo;
        let on = self.rt.archive_on();
        let now = self.level_time();
        match self.archive.client_info(on, target, &mut age, now) {
            ClientInfo::Archived(ps, team) => Some((Some(ps), team)),
            ClientInfo::Live if self.followable(0, target) => {
                Some((None, self.rt.client_team(target)))
            }
            _ => None,
        }
    }

    fn land_follow(
        &mut self,
        slot: usize,
        target: usize,
        team: i32,
        forced: bool,
        copy: Option<Box<vcod_cod2::net::PlayerState2>>,
    ) {
        if let Some(p) = self.players[slot].as_mut() {
            p.follow = Some(FollowView {
                target,
                team,
                forced,
            });
            p.follow_copy = copy;
        }
    }

    /// The follow copy's flags (`SpectatorClientEndFrame`): `pm_flags`
    /// 0x800000 off and 0x400000 on; a forced follow clears 0x1000000 and
    /// 0x2000000, any other sets 0x1000000 and 0x2000000 only with free
    /// flight allowed; `eFlags` 0x100000 the spectator's own. VERIFIED.
    pub fn follow_flags(
        pm_flags: u32,
        e_flags: u32,
        own_e_flags: u32,
        forced: bool,
        mask: u8,
    ) -> (u32, u32) {
        let base = (pm_flags & !PMF_PLAYING) | PMF_FOLLOW;
        let pm = if forced {
            base & !(PMF_MAY_LEAVE | PMF_FREELOOK)
        } else if mask & NO_FREELOOK != 0 {
            (base & !PMF_FREELOOK) | PMF_MAY_LEAVE
        } else {
            base | PMF_MAY_LEAVE | PMF_FREELOOK
        };
        (pm, (e_flags & !EF_VOTED) | (own_e_flags & EF_VOTED))
    }

    /// `sv_maxclients`, where the follow cycle wraps (`level.maxclients`).
    fn max_clients(&self) -> usize {
        match atoi(self.rt.host.cvars.get("sv_maxclients")) {
            n if n > 0 => (n as usize).min(self.players.len()),
            _ => self.players.len(),
        }
    }

    /// Whether `slot` stands at the intermission (`pm_type` 5), where
    /// `ClientCommand` answers only `say`, `say_team` and `score`.
    pub fn at_intermission(&self, slot: usize) -> bool {
        self.players
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|p| p.ps.pm_type == pm_type::INTERMISSION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        crate::game::say::tokenize(line)
    }

    fn reply(out: &CmdOut) -> &str {
        out.sends.first().map_or("", |(_, t)| t.as_str())
    }

    /// mp_toujane `dm` with client 0 spawned on allies and client 1 left a
    /// spectator. `None` without `COD2_DIR`.
    fn level() -> Option<(Cod2Level, i32)> {
        use crate::game::script::MenuRef;
        let fs = vcod_common::testing::cod2_fs()?;
        let mut level = Cod2Level::load(
            std::rc::Rc::new(fs),
            "mp_toujane",
            "dm",
            crate::cvars::Cvars::new(),
            0,
        )
        .expect("mp_toujane dm");
        let mut now = 0;
        level.connect(0, "a");
        level.connect(1, "b");
        for step in 0..12 {
            now += 50;
            for slot in 0..2 {
                let cmd = vcod_cod2::net::UserCmd2 {
                    server_time: now,
                    ..Default::default()
                };
                level.client_think(slot, &cmd);
            }
            level.run_frame(now);
            match step {
                0 => {
                    level.begin(0);
                    level.begin(1);
                }
                1 => level.menu_response(0, &MenuRef::Name("team_britishgerman".into()), "allies"),
                5 => level.menu_response(0, &MenuRef::Name("weapon_british".into()), "enfield_mp"),
                _ => {}
            }
        }
        assert!(level.playing(0), "client 0 spawned");
        assert!(!level.playing(1), "client 1 spectates");
        Some((level, now))
    }

    /// `give weapons`' set (0x80fe588): every weapon but the turrets, and
    /// of the slot weapons the first two past the old second slot's index
    /// (`second`), one a slot; less those `taken` says went again.
    fn assert_given_weapons(level: &Cod2Level, second: usize, taken: &dyn Fn(usize) -> bool) {
        let host = &level.rt.host;
        let weapons = host.weapons.clone();
        let w = host.client_weapons[0];
        let n = host.cod2().weapons.len();
        let given: Vec<usize> = (second + 1..=n)
            .filter(|&i| matches!(weapons.slot(i), 1 | 2))
            .take(2)
            .collect();
        assert_eq!(
            [usize::from(w.slots[1]), usize::from(w.slots[2])],
            [given[0], given[1]]
        );
        for i in 1..=n {
            let class = weapons.get(i).map_or("", |d| d.weapon_class.as_str());
            let expect = if matches!(class, "turret" | "non-player") {
                false
            } else if matches!(weapons.slot(i), 1 | 2) {
                given.contains(&i)
            } else {
                !taken(i)
            };
            assert_eq!(
                w.holds(i),
                expect,
                "{} ({class})",
                host.cod2().weapons[i - 1]
            );
        }
    }

    /// `give weapons` fills the class's rifle in slot 1 and its pistol in
    /// slot 2 with the next two slot weapons.
    #[test]
    fn give_weapons_fills_the_two_slots_past_the_old_second() {
        let Some((mut level, _)) = level() else {
            return;
        };
        level.rt.host.cvars.set("sv_cheats", "1");
        let second = usize::from(level.rt.host.client_weapons[0].slots[2]);
        assert_ne!(second, 0);
        level.client_command(0, &argv("give weapons"));
        assert_given_weapons(&level, second, &|_| false);
    }

    /// `give all` (0x80fe588): the maximum health, `give weapons`' set, then
    /// `Add_Ammo(998, fill)` on every weapon in index order: each held
    /// one's clip full from its reserve and the reserve at `maxAmmo`. A
    /// grenade past the first of its `sharedAmmoCap` overflows the cap
    /// with its full clip and is taken again (`Add_Ammo` Mac 0x1b0e30), so
    /// one frag and one smoke stay. The spawn's grenade stays the offhand,
    /// as it holds rounds.
    #[test]
    fn give_all_fills_health_weapons_and_ammo() {
        let Some((mut level, _)) = level() else {
            return;
        };
        level.rt.host.cvars.set("sv_cheats", "1");
        let second = usize::from(level.rt.host.client_weapons[0].slots[2]);
        let offhand = level.rt.host.cod2().clients[0].offhand;
        level.rt.host.client_vitals[0].health = 7;
        level.client_command(0, &argv("give all"));
        let weapons = level.rt.host.weapons.clone();
        let cap = |i: usize| weapons.get(i).and_then(|d| d.shared_cap_index);
        let first_of_cap = |i: usize| (1..i).all(|j| cap(j) != cap(i));
        assert_given_weapons(&level, second, &|i| cap(i).is_some() && !first_of_cap(i));
        let host = &level.rt.host;
        let v = host.client_vitals[0];
        assert_eq!(v.health, v.max_health);
        assert_eq!(host.cod2().clients[0].offhand, offhand);
        let rounds = host.client_ammo[0];
        for i in 1..=host.cod2().weapons.len() {
            let Some(d) = host.weapons.get(i) else {
                continue;
            };
            if !host.client_weapons[0].holds(i) || d.shared_cap_index.is_some() {
                continue;
            }
            let name = &host.cod2().weapons[i - 1];
            assert_eq!(rounds.clip[d.clip_index], d.clip_size as i16, "{name} clip");
            if !d.clip_only {
                assert_eq!(rounds.ammo[d.ammo_index], d.max_ammo as i16, "{name} ammo");
            }
        }
    }

    /// The cheats behind `sv_cheats`, as retail answered them live
    /// (cod2-protocol.md 3.4.2).
    #[test]
    fn the_cheats_answer_as_retail() {
        let Some((mut level, mut now)) = level() else {
            return;
        };
        let cmd = |level: &mut Cod2Level, slot: usize, line: &str| {
            let out = level.client_command(slot, &argv(line));
            reply(&out).to_string()
        };
        assert_eq!(cmd(&mut level, 0, "god"), "e \"GAME_CHEATSNOTENABLED\"");
        assert_eq!(
            cmd(&mut level, 0, "Foo bar"),
            "e \"GAME_UNKNOWNCLIENTCOMMAND\u{15}Foo\""
        );
        let o = level.rt.client_origin(0);
        assert_eq!(
            cmd(&mut level, 0, "where"),
            format!("e \"\u{15}{}\n\"", vtos(o))
        );
        level.rt.host.cvars.set("sv_cheats", "1");
        assert_eq!(cmd(&mut level, 0, "god"), "e \"GAME_GODMODE_ON\"");
        assert_eq!(cmd(&mut level, 0, "god"), "e \"GAME_GODMODE_OFF\"");
        assert_eq!(cmd(&mut level, 0, "demigod"), "e \"GAME_DEMI_GODMODE_ON\"");
        assert_eq!(cmd(&mut level, 0, "notarget"), "e \"GAME_NOTARGETON\"");
        // A spectator's health is 0.
        assert_eq!(cmd(&mut level, 1, "god"), "e \"GAME_MUSTBEALIVECOMMAND\"");
        assert_eq!(cmd(&mut level, 0, "noclip"), "e \"GAME_NOCLIPON\"");
        now += 50;
        level.run_frame(now);
        assert_eq!(
            level.players[0].as_ref().unwrap().ps.pm_type,
            pm_type::NOCLIP
        );
        assert_eq!(cmd(&mut level, 0, "noclip"), "e \"GAME_NOCLIPOFF\"");
        now += 50;
        level.run_frame(now);
        assert_eq!(
            level.players[0].as_ref().unwrap().ps.pm_type,
            pm_type::NORMAL
        );
        assert_eq!(
            cmd(&mut level, 0, "setviewpos 1 2"),
            "e \"GAME_USAGE\u{15}: setviewpos x y z yaw\""
        );
        let eye = level.players[0].as_ref().unwrap().ps.view_height_current;
        assert_eq!(cmd(&mut level, 0, "setviewpos 100 200 300 90"), "");
        let ps = &level.players[0].as_ref().unwrap().ps;
        assert_eq!(ps.origin, Vec3::new(100.0, 200.0, 300.0 - eye + 1.0));
        assert_eq!(ps.view_angles[1], 90.0);
        assert_eq!(cmd(&mut level, 0, "take health"), "");
        assert_eq!(level.rt.client_vitals(0).health, 1);
        assert_eq!(cmd(&mut level, 0, "give health"), "");
        let v = level.rt.client_vitals(0);
        assert_eq!(v.health, v.max_health);
    }

    /// A spectator's `follownext` lands on the one player, and its
    /// snapshot carries that player's playerstate under the follow flags.
    #[test]
    fn a_spectator_follows_the_player() {
        let Some((mut level, now)) = level() else {
            return;
        };
        // The player cannot cycle.
        assert!(!level.follow_cycle(0, 1));
        level.client_command(1, &argv("follownext"));
        assert_eq!(level.players[1].as_ref().unwrap().spectator_client, 0);
        level.run_frame(now + 50);
        let follow = level.players[1].as_ref().unwrap().follow;
        assert_eq!(follow.map(|f| f.target), Some(0));
        let w = level.wire_playerstate(1, 3).unwrap();
        let f = |n: &str| {
            w.f[vcod_cod2::net::delta::field_index(&vcod_cod2::net::fields::PLAYER_STATE_FIELDS, n)]
        };
        assert_eq!(f("clientNum"), 0);
        assert_eq!(f("pm_flags") & 0x3c0_0000, 0x340_0000);
        // Melee lets go, the spectator stays behind the player's eye.
        level.spectator_buttons(1, 4, 0);
        assert_eq!(level.players[1].as_ref().unwrap().spectator_client, -1);
        let w = level.wire_playerstate(1, 3).unwrap();
        let f = |n: &str| {
            w.f[vcod_cod2::net::delta::field_index(&vcod_cod2::net::fields::PLAYER_STATE_FIELDS, n)]
        };
        assert_eq!(f("clientNum"), 1);
    }

    /// A follower's cmds still reach `useButtonPressed`, the stock
    /// killcam's skip key: `+activate` held, or tapped and let go inside
    /// one frame (the latch), reads 1 until the end frame.
    #[test]
    fn a_follower_s_use_reaches_use_button_pressed() {
        let Some((mut level, mut now)) = level() else {
            return;
        };
        level.client_command(1, &argv("follownext"));
        now += 50;
        level.run_frame(now);
        assert!(level.players[1].as_ref().unwrap().follow.is_some());
        let think = |level: &mut Cod2Level, t: i32, buttons: i32| {
            let cmd = vcod_cod2::net::UserCmd2 {
                server_time: t,
                buttons,
                ..Default::default()
            };
            level.client_think(1, &cmd);
        };
        let use_held = |level: &Cod2Level| {
            level.rt.host.client_buttons[1] & vcod_common::net::msg::BUTTON_USE != 0
        };
        think(&mut level, now + 16, 0x8);
        assert!(use_held(&level), "a held +activate");
        think(&mut level, now + 33, 0);
        assert!(use_held(&level), "the tap stays latched for the frame");
        now += 50;
        level.run_frame(now);
        assert!(!use_held(&level), "the end frame clears the latch");
        think(&mut level, now + 16, 0x20);
        assert!(use_held(&level), "+usereload is 0x20");
    }

    /// `G_RunFrame`'s objective pass reaches the wire: a current record
    /// goes out whole, one for the other team as its state 0 alone.
    #[test]
    fn objectives_reach_the_playerstate() {
        let Some((mut level, now)) = level() else {
            return;
        };
        let o = vcod_common::net::msg::Objective {
            state: 4,
            origin: [1.0f32, 2.0, 3.0].map(|v| v.to_bits() as i32),
            icon: 5,
            ent_num: 1023,
            team_num: 0,
        };
        level.rt.host.objectives[0] = o;
        level.rt.host.objectives[1] = vcod_common::net::msg::Objective { team_num: 9, ..o };
        level.run_frame(now + 50);
        let w = level.wire_playerstate(0, 0).unwrap();
        let field = |n: &str| {
            vcod_cod2::net::fields::OBJECTIVE_FIELDS
                .iter()
                .position(|f| f.name == n)
                .unwrap()
        };
        assert_eq!(w.objectives[0].state, 4);
        assert_eq!(w.objectives[0].f[field("icon")], 5);
        assert_eq!(f32::from_bits(w.objectives[0].f[field("origin[2]")]), 3.0);
        assert_eq!(w.objectives[0].f[field("entNum")], 1023);
        assert_eq!(w.objectives[1].state, 0);
    }

    #[test]
    fn vtos_truncates_each_component() {
        assert_eq!(vtos([1.9, -2.7, 300.0]), "(1 -2 300)");
    }

    /// The three `pm_flags` shapes `SpectatorClientEndFrame` leaves on a
    /// copy of a playing client's 0x800000.
    #[test]
    fn the_follow_copy_flags() {
        let playing = 0x80_0000 | 0x1;
        let (pm, ef) = Cod2Level::follow_flags(playing, 0x10_0002, 0, false, 0);
        assert_eq!(pm, 0x340_0001);
        assert_eq!(ef, 0x2);
        let (pm, ef) = Cod2Level::follow_flags(playing, 0x2, EF_VOTED, false, 0x10);
        assert_eq!(pm, 0x140_0001);
        assert_eq!(ef, 0x10_0002);
        let (pm, _) = Cod2Level::follow_flags(playing | 0x300_0000, 0, 0, true, 0);
        assert_eq!(pm, 0x40_0001);
    }
}
