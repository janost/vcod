//! Our own cmds on a live server: the frame's send, the weapon picks, the
//! location selector and the melee charge.

use anyhow::Result;
use glam::Vec3;
use vcod_cod4::net::clmsg::UserCmd;
use vcod_cod4::net::conn::{ConnState, angle2short, cmd_angles};
use vcod_cod4::net::delta::PlayerState;
use vcod_cod4::net::parse::Client;
use vcod_cod4::pmove::weapon::WeaponDef;
use vcod_cod4::pmove::{self as pm, Cmd};

use super::events::{origin_of, source_client};
use super::scene::weapon_name;
use super::{
    Cod4Session, Dry, ET_PLAYER, INVENTORY_ALT_MODE, Source, client_team, ent_f32, ent_u32, ps_f32,
    weapon_list,
};
use crate::play::cod4_input::Cod4Input;
use crate::play::cod4_predict;

/// `cg.selectedLocation` and `locSelInputState`: where the location
/// selector's cursor is over the full map (0..1 east and south), and a
/// confirm or cancel the next cmd carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selector {
    pub at: [f32; 2],
    pub confirm: bool,
    pub cancel: bool,
    /// `+attack` was down at the last cmd: a press confirms.
    attack_was: bool,
}

impl Default for Selector {
    fn default() -> Self {
        Selector {
            at: [0.5; 2],
            confirm: false,
            cancel: false,
            attack_was: false,
        }
    }
}

/// The cmd bits of a selecting body (`CL_CreateCmd`'s selector arm): always
/// 0x100000, 0x10000 to confirm, 0x20000 to cancel.
pub const BUTTON_SELECTING: u32 = 0x10_0000;
pub const BUTTON_CONFIRM_LOCATION: u32 = 0x1_0000;
pub const BUTTON_CANCEL_LOCATION: u32 = 0x2_0000;
/// `cg_mapLocationSelectionCursorSpeed`'s default 0.6 (the float at
/// `iw3mp.exe` 0x6b7a9c its registration at 0x43bc9c loads).
pub const SELECTOR_SPEED: f32 = 0.6;

impl Cod4Session {
    /// `ps.locationSelectionInfo` of the newest snapshot: non-zero while
    /// the server's `beginLocationSelection` holds us on the full map.
    pub fn selecting_location(&self) -> u32 {
        self.client()
            .snapshots
            .last()
            .map(|s| s.ps.word(vcod_cod4::ps_word!("locationSelectionInfo")))
            .unwrap_or(0)
    }

    /// The mouse's turn, `[pitch, yaw]` degrees, moving the selector's
    /// cursor instead of the view: a degree right moves it
    /// `cg_mapLocationSelectionCursorSpeed` / 100 of the map east, down
    /// as far south (vcod's scale; 1.7 moves it by the mouse's counts).
    pub fn move_selector(&mut self, [pitch, yaw]: [f32; 2]) {
        let k = SELECTOR_SPEED / 100.0;
        let at = &mut self.selector.at;
        at[0] = (at[0] - yaw * k).clamp(0.0, 1.0);
        at[1] = (at[1] + pitch * k).clamp(0.0, 1.0);
    }

    /// A live frame: the new server commands' menus and weapon picks, the
    /// view the server turned, this frame's cmd, and the packet when one
    /// is due.
    pub(super) fn live_frame(&mut self, new: &[Vec<u8>]) -> Result<()> {
        let mut answers = Vec::new();
        // The source alone, beside the menus and the input.
        if let Some(gs) = &source_client(&self.source).gamestate {
            for text in new {
                answers.extend(self.menus.command(text, gs));
                let t = String::from_utf8_lossy(text);
                let mut w = t.split_whitespace();
                let arg = |w: Option<&str>| w.and_then(|x| x.parse::<u8>().ok());
                match w.next() {
                    // `switchToWeapon`, `setSpawnWeapon`: `cg_weaponSelect`.
                    Some("a") => {
                        if let Some(n) = arg(w.next()) {
                            self.input.select(n);
                        }
                    }
                    // The equipped offhand.
                    Some("C") => {
                        if let Some(n) = arg(w.next()) {
                            self.input.set_offhand(n);
                        }
                    }
                    _ => {}
                }
            }
        }
        for a in answers {
            log::info!("cod4: {a}");
            self.command(&a);
        }
        self.take_own_view();
        if let Some(d) = self.ran_dry.take() {
            self.out_of_ammo_change(&d);
        }
        // A packet no scene sent since the last step goes out first, with
        // no predicted origin.
        if self.packet_due {
            self.send_packet(None)?;
        }
        let time = self.cmd_time();
        let Source::Live { conn, .. } = &mut self.source else {
            return Ok(());
        };
        // `CL_CreateNewCommands` makes none before the cgame is up
        // (`iw3mp.exe` 0x463e09).
        if conn.state() == ConnState::Active {
            let floor = self.pending.last().unwrap_or(conn.last_cmd()).server_time;
            // One cmd a frame, never two on one millisecond. Before the
            // first snapshot there is no clock: each frame's takes the next
            // ms, so a primed client's move still enters the world.
            let time = match self.cg_time {
                Some(_) => (time > floor).then_some(time),
                None => Some(floor + 1),
            };
            if let Some(time) = time {
                // `CL_SetUserCmdAimValues`: the recoil rides the cmd's angles.
                let [kp, ky, kr] = self.kick.view_kick();
                let c = own_cmd(
                    &conn.client,
                    time,
                    [self.view[0] + kp, self.view[1] + ky, kr],
                    &self.input,
                    self.talking,
                );
                let mut c = UserCmd {
                    buttons: c.buttons | self.input.take_extra(),
                    ..c
                };
                let world = self.world.as_ref().map(|w| &*w.clip);
                (c.melee_charge_yaw, c.melee_charge_dist) = melee_charge(
                    &conn.client,
                    self.last_predicted.as_ref(),
                    [self.view[0] + kp, self.view[1] + ky],
                    c.buttons,
                    world,
                );
                selector_cmd(&mut self.selector, &mut c, ps_u_last(&conn.client));
                self.pending.push(c);
                // Unpredicted (a linked body), the drawn view is still
                // this cmd's over the newest `delta_angles`.
                self.cmd_angles = Some(c.angles);
                if self.sent_cmds.len() == cod4_predict::CMD_BACKUP {
                    self.sent_cmds.pop_front();
                }
                self.sent_cmds.push_back(Cmd::from(&c));
            }
        }
        // A packet carries every cmd built since the last and
        // `cl_packetdup`'s again.
        if conn.ready_to_send() {
            // The scene sends it once its prediction has run this cmd, as
            // the cgame's frame does; a step with no scene to come (no
            // gamestate or clock yet) sends it now.
            let no_scene = conn.client.gamestate.is_none() || self.cg_time.is_none();
            self.packet_due = true;
            if no_scene {
                self.send_packet(None)?;
            }
        }
        Ok(())
    }

    /// `cl.serverTime`, what a cmd built now is stamped (`iw3mp.exe`
    /// 0x463a73), at most 5 s past the newest snapshot (0x463a85); 0
    /// before the clock starts.
    pub(super) fn cmd_time(&self) -> i32 {
        let newest = self.client().snapshots.last().map(|s| s.server_time.raw());
        match (self.cg_time, newest) {
            (Some(t), Some(n)) => t.min(n.saturating_add(5000)),
            (t, _) => t.unwrap_or(0),
        }
    }

    /// `CL_WritePacket` (0x463f50) with what `CL_SetUserCmdOrigin` last
    /// stored: the cmds built since the last packet with those
    /// `cl_packetdup` resends (`Conn::send`), and `origin`, the
    /// predicted state at `cl.serverTime`, which the move carries and the
    /// archive keeps (cod4-netfields.md 13).
    pub(super) fn send_packet(
        &mut self,
        origin: Option<vcod_cod4::net::demo::Archive>,
    ) -> Result<()> {
        self.packet_due = false;
        let Source::Live { conn, .. } = &mut self.source else {
            return Ok(());
        };
        let sent = conn.send(&self.pending, origin);
        self.pending.clear();
        sent
    }

    /// The newest snapshot's own playerstate: a spawn or a turn the
    /// server made moves [`Self::view`] to its view, and the input takes
    /// its weapons. A followed one is left alone.
    fn take_own_view(&mut self) {
        let Some(gs) = self.gamestate() else {
            return;
        };
        let own = gs.client_num as u32;
        let Some(ps) = self.client().snapshots.last().map(|s| s.ps.clone()) else {
            return;
        };
        let u = |n: &str| ps.get(n).unwrap_or(0);
        if u("clientNum") != own || u("otherFlags") & 2 != 0 {
            self.own_delta = None;
            return;
        }
        let delta = [
            ps_f32(&ps, "delta_angles[0]"),
            ps_f32(&ps, "delta_angles[1]"),
        ];
        if self.own_delta != Some(delta) {
            // The cmd's angles stay as retail's `cl.viewangles` do, so the
            // view moves by the delta's change: the pitch clamp's push or
            // a spawn's turn. The snapshot's viewangles are stale unless
            // the view is locked (cod4-netfields.md 7); only the first
            // own playerstate takes them.
            // A plain wrap: the short grid's truncation would move the cmd
            // a short off the one the delta was pushed for.
            self.view = match self.own_delta {
                Some(old) => std::array::from_fn(|i| {
                    (self.view[i] + delta[i] - old[i] + 180.0).rem_euclid(360.0) - 180.0
                }),
                None => [ps_f32(&ps, "viewangles[0]"), ps_f32(&ps, "viewangles[1]")],
            };
            self.own_delta = Some(delta);
            self.view_jump = true;
        }
        let held = std::array::from_fn(|i| u(&format!("weapons[{i}]")));
        self.input.sync(
            own,
            ps.stats[4],
            u("weapon") as u8,
            u("offHandIndex") as u8,
            held,
        );
    }

    /// Our own playing body in the newest snapshot (`pm_type` normal, not
    /// following): its origin and weapon, as a line.
    pub fn spawn(&self) -> Option<String> {
        let gs = self.gamestate()?;
        let ps = &self.client().snapshots.last()?.ps;
        let u = |n: &str| ps.get(n).unwrap_or(0);
        if u("clientNum") != gs.client_num as u32 || u("otherFlags") & 2 != 0 || u("pm_type") != 0 {
            return None;
        }
        let w = u("weapon");
        Some(format!(
            "client {} at ({:.0} {:.0} {:.0}) weapon {w} {}",
            gs.client_num,
            ps_f32(ps, "origin[0]"),
            ps_f32(ps, "origin[1]"),
            ps_f32(ps, "origin[2]"),
            weapon_name(&weapon_list(gs), w).unwrap_or("?")
        ))
    }

    /// The mouse's turn this frame (pitch, yaw), degrees, added to
    /// [`Self::view`] as `CL_MouseMove` adds it to `cl.viewangles`: past
    /// the pitch limit it piles up until the server's `delta_angles` push
    /// takes it back, while the drawn view stays clamped.
    pub fn turn(&mut self, by: [f32; 2]) {
        self.view = turned(self.view, by);
    }

    /// The server turned the view since the last call: [`Self::view`] is
    /// the camera's now.
    pub fn take_view_jump(&mut self) -> bool {
        std::mem::take(&mut self.view_jump)
    }

    /// The held weapons `weapnext` walks: those the level's weapon defs
    /// call primaries (`inventoryType` 0), in index order.
    pub fn held_primaries(&self) -> Vec<u8> {
        let (Some(gs), Some(w), Some(snap)) = (
            self.gamestate(),
            &self.world,
            self.client().snapshots.last(),
        ) else {
            return Vec::new();
        };
        let held: [u32; 4] =
            std::array::from_fn(|i| snap.ps.get(&format!("weapons[{i}]")).unwrap_or(0));
        let list = weapon_list(gs);
        (1..128)
            .filter(|&n| crate::play::cod4_input::holds(held, n))
            .filter(|&n| weapon_name(&list, u32::from(n)).is_some_and(|x| w.is_primary(x)))
            .collect()
    }

    /// The weapon an `altMode`
    /// action slot shows, the selection itself when it is an alt mode,
    /// else its alt, else the alt of the first held primary after it that
    /// has one (`Cod4Input::action_slot`'s order); 0 for none. Also
    /// whether the selection is an alt mode (the slot's active state).
    pub fn alt_toggle_weapon(&self) -> (u8, bool) {
        let Some((_, defs)) = &self.level_defs else {
            return (0, false);
        };
        let def = |w: u8| defs.get(usize::from(w));
        let selected = self.input.weapons().0;
        if def(selected).is_some_and(|d| d.inventory_type == INVENTORY_ALT_MODE) {
            return (selected, true);
        }
        let alt = |w: u8| def(w).map_or(0, |d| d.alt_weapon as u8);
        if alt(selected) != 0 {
            return (alt(selected), false);
        }
        let primaries = self.held_primaries();
        let n = primaries.len();
        let at = primaries.iter().position(|&p| p == selected);
        let w = (1..=n)
            .map(|k| primaries[(at.unwrap_or(n.saturating_sub(1)) + k) % n])
            .map(alt)
            .find(|&a| a != 0)
            .unwrap_or(0);
        (w, false)
    }

    /// `+actionslot <n>` (1..=4) over the newest playerstate's slot and
    /// the level's weapon defs (each def's alt): the weapon it selects
    /// goes in the next cmds (`Cod4Input::action_slot`). True when the
    /// selection moved.
    pub fn action_slot(&mut self, n: usize) -> bool {
        let Some(snap) = self.client().snapshots.last() else {
            return false;
        };
        let ps = &snap.ps;
        let u = |name: &str| ps.get(name).unwrap_or(0);
        let k = n.saturating_sub(1).min(3);
        let s = crate::play::cod4_input::SlotState {
            kind: u(&format!("actionSlotType[{k}]")),
            param: u(&format!("actionSlotParam[{k}]")),
            weapon_state: u("weaponstate") as i32,
            pm_flags: u("pm_flags"),
            weap_flags: u("weapFlags"),
            e_flags: u("eFlags"),
            other_flags: u("otherFlags"),
            held: std::array::from_fn(|i| u(&format!("weapons[{i}]"))),
            weapon: u("weapon") as u8,
        };
        let now = snap.server_time;
        let primaries = self.held_primaries();
        let defs = self.level_defs.as_ref().map(|(_, d)| d.clone());
        let alt_of = |w: u8| -> u8 {
            defs.as_ref()
                .and_then(|d| d.get(usize::from(w)))
                .map_or(0, |d| d.alt_weapon as u8)
        };
        self.input.action_slot(&s, &alt_of, &primaries, now.raw())
    }

    /// `CG_OutOfAmmoChange` (`iw3mp.exe` 0x457560) after our own EV_NOAMMO
    /// (called from 0x436376): unless dead or the weapon cancels the auto
    /// holster (+1288), an alt mode with its base gun held selects the
    /// gun when it has ammo (a grenade launcher's last round goes back to
    /// the rifle), else the primary cycle (0x4576d0) selects the next
    /// held primary by index that has ammo; an alt mode whose gun is gone
    /// selects nothing (the alt mode check, 0x457510). The arm
    /// for an empty hand (`weaponLatestPrimaryIdx`) is left out.
    fn out_of_ammo_change(&mut self, d: &Dry) {
        let Some((_, defs)) = &self.level_defs else {
            return;
        };
        let primaries = self.held_primaries();
        if let Some(w) = out_of_ammo_pick(defs, d, self.input.weapons().0, &primaries) {
            self.input.select(w);
        }
    }
}

/// [`Cod4Session::out_of_ammo_change`]'s pick over the level's `defs`,
/// the selection `selected` and the held primaries by index.
pub(super) fn out_of_ammo_pick(
    defs: &[WeaponDef],
    d: &Dry,
    selected: u8,
    primaries: &[u8],
) -> Option<u8> {
    if d.pm_type >= pm::pm_type::DEAD {
        return None;
    }
    let def = defs.get(d.weapon as usize)?;
    if def.cancel_auto_holster_when_empty {
        return None;
    }
    if def.inventory_type == INVENTORY_ALT_MODE {
        // The primary cycle on an alt mode selects its gun as well.
        let alt = def.alt_weapon as u8;
        return Some(match crate::play::cod4_input::holds(d.held, alt) {
            true => alt,
            false => 0,
        });
    }
    let has_ammo = |w: u8| {
        defs.get(usize::from(w)).is_some_and(|x| {
            d.ammoclip.get(x.clip_index).copied().unwrap_or(0)
                + d.ammo.get(x.ammo_index).copied().unwrap_or(0)
                > 0
        })
    };
    primaries
        .iter()
        .copied()
        .filter(|&w| w != selected && has_ammo(w))
        .min_by_key(|&w| (w < selected, w))
}

/// `locationSelectionInfo` of a client's newest snapshot.
fn ps_u_last(c: &Client) -> u32 {
    c.snapshots
        .last()
        .map(|s| s.ps.word(vcod_cod4::ps_word!("locationSelectionInfo")))
        .unwrap_or(0)
}

/// `CL_CreateCmd`'s location selector arm, INFERRED: while
/// `loc` is set the cmd carries only its stance, 0x100000, and a confirm
/// (the `+attack` press, with the cursor in `selectedLocation` as
/// `round(at * 255) + 0x80`) or a cancel (Esc); without it the cursor
/// goes back to the map's middle (`CG_DrawActiveFrame`).
fn selector_cmd(sel: &mut Selector, c: &mut UserCmd, loc: u32) {
    let attack = c.buttons & vcod_cod4::pmove::button::ATTACK != 0;
    if loc == 0 {
        *sel = Selector {
            attack_was: attack,
            ..Selector::default()
        };
        return;
    }
    if attack && !sel.attack_was {
        sel.confirm = true;
    }
    sel.attack_was = attack;
    use vcod_cod4::pmove::button::{CROUCH, PRONE};
    c.buttons = c.buttons & (CROUCH | PRONE) | BUTTON_SELECTING;
    if std::mem::take(&mut sel.confirm) {
        c.buttons |= BUTTON_CONFIRM_LOCATION;
        c.selected_location = sel
            .at
            .map(|v| ((v * 255.0 + 0.5).floor() as i32 + 0x80) as u8);
    } else if std::mem::take(&mut sel.cancel) {
        c.buttons |= BUTTON_CANCEL_LOCATION;
    }
}

/// `j_head` and `j_helmet` over the origin by stance (standing, crouched,
/// prone): the bind pose's, as the pick reads them without the posed
/// bodies (cod4-combat.md 10.4).
const AIM_HEIGHTS: [(f32, f32); 3] = [(64.0, 70.0), (44.0, 50.0), (12.0, 18.0)];

/// The cmd's melee charge (`CL_CreateCmd`'s aim assist step, called at
/// `iw3mp.exe` 0x463821, cod4-pmove.md): `charge::client_pick` over the newest snapshot's
/// players, the enemies (anyone without teams) seen from the predicted
/// eye at `cg_fov` 65 on 4:3, as the yaw's short and the distance.
fn melee_charge(
    c: &Client,
    predicted: Option<&pm::PlayerState>,
    view: [f32; 2],
    buttons: u32,
    clip: Option<&vcod_cod4::collision::ClipMap>,
) -> (u16, u8) {
    use vcod_cod4::pmove::charge::{AimBody, AimEye, client_pick};
    let (Some(ps), Some(s)) = (predicted, c.snapshots.last()) else {
        return (0, 0);
    };
    if buttons & pm::button::MELEE == 0 {
        return (0, 0);
    }
    // The pick skips a dead, spectating or linked body.
    if ps.other_flags & 2 != 0
        || !matches!(ps.pm_type, pm::pm_type::NORMAL | pm::pm_type::NORMAL_LINKED)
    {
        return (0, 0);
    }
    let team = |n: u32| {
        s.clients
            .iter()
            .find(|c| c.0[0] == n)
            .map_or(0, client_team)
    };
    let own = team(ps.client_num);
    let t = 32.5f32.to_radians().tan();
    let eye = AimEye {
        origin: ps.origin + Vec3::Z * ps.view_height_current,
        angles: Vec3::new(view[0], view[1], 0.0),
        tan_half_fov: [t, t * 0.75],
        fov_scale_inv: 1.0,
    };
    let bodies: Vec<AimBody> = s
        .entities
        .iter()
        .filter(|e| e.etype() == ET_PLAYER && e.number() != ps.client_num)
        .filter(|e| own == 0 || team(e.number()) != own)
        .filter_map(|e| {
            let origin = origin_of(e);
            let flags = ent_u32(e, "lerp.eFlags");
            let (head, top) = AIM_HEIGHTS[if flags & pm::ef::PRONE != 0 {
                2
            } else if flags & pm::ef::CROUCH != 0 {
                1
            } else {
                0
            }];
            // The visibility test: the eye to `j_head` under
            // 0x803003, the world only.
            let head = origin + Vec3::Z * head;
            if let Some(clip) = clip {
                let tr = clip.trace(eye.origin, head, Vec3::ZERO, Vec3::ZERO, 0x80_3003);
                if tr.fraction < 1.0 {
                    return None;
                }
            }
            Some(AimBody {
                origin,
                yaw: ent_f32(e, "lerp.apos.trBase[1]"),
                aim: origin,
                top,
            })
        })
        .collect();
    let (yaw, dist) = client_pick(buttons, ps.pm_flags, ps.origin, &eye, &bodies);
    (angle2short(yaw), dist)
}

/// This frame's cmd at `server_time` ([`Cod4Session::cmd_time`]), `view`
/// over the playerstate's `delta_angles`, the keys, the selected weapon
/// and offhand (`play::cod4_input`).
fn own_cmd(
    c: &Client,
    server_time: i32,
    view: [f32; 3],
    input: &Cod4Input,
    talking: bool,
) -> UserCmd {
    let none;
    let ps = match c.snapshots.last() {
        Some(s) => &s.ps,
        None => {
            none = PlayerState::default();
            &none
        }
    };
    let delta: [f32; 3] = std::array::from_fn(|i| ps_f32(ps, &format!("delta_angles[{i}]")));
    let [forwardmove, rightmove] = input.moves(talking);
    let (weapon, offhand) = input.weapons();
    let pm_type = ps.word(vcod_cod4::ps_word!("pm_type")) as i32;
    UserCmd {
        server_time,
        buttons: input.buttons(pm_type, talking),
        weapon,
        offhand,
        angles: cmd_angles(view, delta),
        forwardmove,
        rightmove,
        ..UserCmd::default()
    }
}

/// `view` turned `by`, wrapped to -180..180 as a plain float: the short
/// grid's truncation would drop a turn under a short a frame and creep a
/// negative view toward 0 (cod4-client-view.md 14.1). An angle in range
/// is kept as it is, so the frames' sums lose nothing to the wrap.
fn turned(view: [f32; 2], by: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|i| match view[i] + by[i] {
        a if (-180.0..=180.0).contains(&a) => a,
        a => (a + 180.0).rem_euclid(360.0) - 180.0,
    })
}

#[cfg(test)]
mod turn_tests {
    /// A slow mouse turns the view less than one short a frame; the turns
    /// still add up, and a still mouse leaves a negative view where it is.
    #[test]
    fn a_turn_under_a_short_a_frame_adds_up() {
        let mut view = [-10.0, 10.0];
        for _ in 0..1000 {
            view = super::turned(view, [0.0, 0.001]);
        }
        assert!((view[1] - 11.0).abs() < 1e-3, "yaw {}", view[1]);
        assert!((view[0] + 10.0).abs() < 1e-4, "pitch {}", view[0]);
        let mut view = [-170.0, 179.9995];
        for _ in 0..3 {
            view = super::turned(view, [-0.0004, 0.0004]);
        }
        assert!((view[0] + 170.0012).abs() < 1e-4, "pitch {}", view[0]);
        assert!(
            (view[1] + 179.9993).abs() < 1e-4,
            "yaw wraps past 180: {}",
            view[1]
        );
    }

    /// The recoil's roll rides the cmd with its pitch and yaw: 1.7's cmd
    /// loop converts all three angles (0x463aa9..0x463acf).
    #[test]
    fn the_cmd_carries_the_kick_roll() {
        let c = super::own_cmd(
            &vcod_cod4::net::parse::Client::default(),
            0,
            [0.0, 0.0, -1.5],
            &crate::play::cod4_input::Cod4Input::default(),
            false,
        );
        let roll = f32::from(c.angles[2] as i16) * (360.0 / 65536.0);
        assert!((roll + 1.5).abs() < 0.01, "roll {roll}");
    }
}
