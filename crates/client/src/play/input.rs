//! Keys and mouse -> [`UserCmd`], retail's default binds
//! (`docs/protocol-1.1.md`, "Usercmd input bits"). Position is
//! server-authoritative; this only ever fills in axes, buttons, angles and
//! the weapon byte.

use std::collections::HashSet;
use vcod_common::net::flags::{EF_IN_VEHICLE_V22, EF_MOUNTED, PMF_OWN_VIEW};
use vcod_common::net::msg::{self, PlayerState, UserCmd};
use vcod_common::net::protocol::Protocol;
use vcod_common::text::atoi_msvc;
use vcod_common::weapon::WeaponDef;

/// `eFlags` bits the server writes for a crouched/prone player
/// (`crates/server/src/spectate.rs`, `to_wire`), the inverse the caller reads
/// to build [`Held::stance`] from a playerstate.
pub use vcod_common::net::flags::{EF_CROUCH, EF_PRONE};

/// ANGLE2SHORT units per degree.
const SHORT_PER_DEG: f32 = 65536.0 / 360.0;

/// Degrees of view per mouse count, `CL_MouseMove` (CoDMP.exe 0x40b240;
/// docs/research/cod11-console.md, section 5), and where the motion goes:
/// `cl_freelook` 0 turns the vertical motion into `forwardmove` unless
/// `+mlook` is held, `+strafe` the horizontal one into `rightmove`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseLook {
    pub sensitivity: f32,
    pub m_yaw: f32,
    pub m_pitch: f32,
    pub m_forward: f32,
    pub m_side: f32,
    pub freelook: bool,
}

impl Default for MouseLook {
    /// Retail's defaults: 5 x 0.022 = 0.11 degrees a count.
    fn default() -> Self {
        MouseLook {
            sensitivity: 5.0,
            m_yaw: 0.022,
            m_pitch: 0.022,
            m_forward: 0.25,
            m_side: 0.25,
            freelook: true,
        }
    }
}

/// One frame's mouse motion as `CL_MouseMove` spends it: the turn in
/// degrees `[pitch, yaw]` and the counts added to the cmd's `forwardmove`
/// and `rightmove`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouseMove {
    pub turn: [f32; 2],
    pub forward: i32,
    pub side: i32,
}

impl MouseLook {
    /// The motion scaled by the rate: `sensitivity * zoom` on both axes, or
    /// on a mounted gun 2.5 on yaw and 2 on pitch with no sensitivity.
    fn scaled(&self, dx: f32, dy: f32, zoom: f32, mounted: bool) -> (f32, f32) {
        if mounted {
            (dx * 2.5, dy * 2.0)
        } else {
            let s = self.sensitivity * zoom;
            (dx * s, dy * s)
        }
    }

    /// `[pitch, yaw]` degrees for a motion of `(dx, dy)` counts: pitch
    /// down-positive, yaw left-positive, as the view angles run. `zoom` is
    /// cgame's sensitivity scale, the view's fov over `cg_fov`.
    pub fn degrees(&self, dx: f32, dy: f32, zoom: f32, mounted: bool) -> [f32; 2] {
        let (mx, my) = self.scaled(dx, dy, zoom, mounted);
        [my * self.m_pitch, -mx * self.m_yaw]
    }

    /// The whole of `CL_MouseMove`: `+strafe` (`strafe`) sends the
    /// horizontal motion to `rightmove` through `m_side`, and without
    /// `cl_freelook` or `+mlook` (`mlook`), or under `+strafe`, the
    /// vertical motion goes off `forwardmove` through `m_forward`, each
    /// truncated (`_ftol`, 0x538be0).
    pub fn moves(
        &self,
        dx: f32,
        dy: f32,
        zoom: f32,
        mounted: bool,
        mlook: bool,
        strafe: bool,
    ) -> MouseMove {
        let (mx, my) = self.scaled(dx, dy, zoom, mounted);
        let mut out = MouseMove::default();
        if strafe {
            out.side = (mx * self.m_side) as i32;
        } else {
            out.turn[1] = -mx * self.m_yaw;
        }
        if (mlook || self.freelook) && !strafe {
            out.turn[0] = my * self.m_pitch;
        } else {
            out.forward = -((my * self.m_forward) as i32);
        }
        out
    }
}

/// `CL_MouseEvent`'s two motion slots and `CL_MouseMove`'s read of them
/// (CoDMP.exe 0x40b240): a frame takes its own slot, or with `m_filter`
/// the mean of its slot and the last frame's, then flips to the other slot
/// and clears it.
#[derive(Clone, Copy, Debug, Default)]
pub struct MouseAccum {
    dx: [f32; 2],
    dy: [f32; 2],
    index: usize,
}

impl MouseAccum {
    pub fn add(&mut self, dx: f32, dy: f32) {
        self.dx[self.index] += dx;
        self.dy[self.index] += dy;
    }

    /// Once a frame, whether or not the mouse moved.
    pub fn take(&mut self, filter: bool) -> (f32, f32) {
        let i = self.index;
        let out = if filter {
            (
                (self.dx[0] + self.dx[1]) * 0.5,
                (self.dy[0] + self.dy[1]) * 0.5,
            )
        } else {
            (self.dx[i], self.dy[i])
        };
        self.index ^= 1;
        self.dx[self.index] = 0.0;
        self.dy[self.index] = 0.0;
        out
    }

    pub fn clear(&mut self) {
        *self = MouseAccum::default();
    }
}

/// A retail default bind. `Slot(n)` is a number key, `n` the weapon slot it
/// requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    /// `+gostand`.
    Jump,
    /// `gocrouch` and `goprone`.
    Crouch,
    Prone,
    /// `+moveup`: a stance up, or the jump standing.
    MoveUp,
    /// `+movedown` and `+prone`: crouched or prone while held, over the
    /// stance the other binds keep.
    MoveDown,
    HoldProne,
    /// `togglecrouch`, `toggleprone`, `lowerstance`, `raisestance`.
    ToggleCrouch,
    ToggleProne,
    LowerStance,
    RaiseStance,
    LeanLeft,
    LeanRight,
    Attack,
    Ads,
    Melee,
    Use,
    Reload,
    Slot(usize),
    NextWeapon,
    PrevWeapon,
    /// `weapalt`; its outcome is [`PlayInput::alt_weapon`]'s, which the
    /// caller runs itself to print the refusal.
    AltWeapon,
    /// United Offensive's `+sprint` (`CL_CmdButtons`, CoDUOMP.exe
    /// 0x40c700), sent only to a UO server.
    Sprint,
    /// `+mlook` (CoDMP.exe 0x40a450): the mouse pitches the view whatever
    /// `cl_freelook` says.
    MLook,
    /// `+strafe` (0x40a8c0): the mouse's horizontal motion strafes.
    Strafe,
    /// CoD2's own binds (`play::cod2_input`): `+frag`, `+smoke`,
    /// `+binoculars`, `+holdbreath`, `+melee_breath`, `+usereload`, and the
    /// impulses `toggleads` and `leaveads`. CoD1 ignores them.
    Frag,
    Smoke,
    Binoculars,
    HoldBreath,
    MeleeBreath,
    UseReload,
    ToggleAds,
    LeaveAds,
    /// CoD4's `+actionslot <n>` (1..=4): the playerstate's
    /// `actionSlotType[n - 1]` says what it does (`play::cod4_input`).
    UseSlot(usize),
    /// CoD4's `+breath_sprint` (hold breath and sprint, one key) and
    /// `+toggleads_throw` (the press flips the sight toggle, the hold is
    /// the throw key); `play::cod4_input`.
    BreathSprint,
    ToggleAdsThrow,
    /// CoD4's `+talk`: push to talk, which only voice reads; vcod has
    /// none, so it holds nothing.
    Talk,
}

/// The client's own stance, held level on the wire
/// (`docs/protocol-1.1.md`, "Usercmd input bits").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Stance {
    #[default]
    Stand,
    Crouch,
    Prone,
}

/// What a stance bind's press does to the kept stance (`cl_stance`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StancePress {
    Set(Stance),
    /// Presses the jump: `+gostand` or `+moveup` standing.
    Jump,
    Nothing,
}

/// The stance binds' presses, alike in CoD1 1.1, 1.5, UO, CoD2 and CoD4
/// (docs/research/cod11-console.md 9). `held` is `+prone` or `+movedown`
/// down, which stops every change to the kept stance; `+gostand` still
/// jumps standing.
pub fn stance_press(action: Action, stance: Stance, held: bool) -> StancePress {
    use Stance::{Crouch, Prone, Stand};
    let set = match (action, stance) {
        (Action::Jump, Stand) => return StancePress::Jump,
        (Action::Jump, _) => Stand,
        (Action::MoveUp, _) if held => return StancePress::Nothing,
        (Action::MoveUp, Stand) => return StancePress::Jump,
        (Action::MoveUp | Action::RaiseStance, Prone) => Crouch,
        (Action::MoveUp | Action::RaiseStance, Crouch) => Stand,
        (Action::Crouch, _) => Crouch,
        (Action::Prone, _) => Prone,
        (Action::ToggleCrouch, Crouch) => Stand,
        (Action::ToggleCrouch, _) => Crouch,
        (Action::ToggleProne, Prone) => Stand,
        (Action::ToggleProne, _) => Prone,
        (Action::LowerStance, Stand) => Crouch,
        (Action::LowerStance, Crouch) => Prone,
        _ => return StancePress::Nothing,
    };
    if held {
        StancePress::Nothing
    } else {
        StancePress::Set(set)
    }
}

/// The stance a held `+prone` (first) or `+movedown` puts the cmd in, if
/// either is down.
pub fn held_stance(down: &HashSet<Action>) -> Option<Stance> {
    if down.contains(&Action::HoldProne) {
        Some(Stance::Prone)
    } else if down.contains(&Action::MoveDown) {
        Some(Stance::Crouch)
    } else {
        None
    }
}

impl Stance {
    /// The inverse of `spectate.rs`'s `to_wire` write of [`EF_CROUCH`] /
    /// [`EF_PRONE`], for the caller building [`Held`] from a playerstate.
    pub fn from_eflags(e_flags: i32) -> Stance {
        if e_flags & EF_PRONE != 0 {
            Stance::Prone
        } else if e_flags & EF_CROUCH != 0 {
            Stance::Crouch
        } else {
            Stance::Stand
        }
    }
}

/// The bit of the newest playerstate a cmd and the weapon selection read:
/// the weapon held, the eight weapon slots (`weaponslots[0]`/`weaponslots[4]`
/// unpacked little-endian, the inverse of
/// `vcod_common::pmove::weapon::pack_slots`), the stance
/// ([`Stance::from_eflags`]), what is carried and loaded, and whose view and
/// spawn it is. The default (nothing held, standing, no view) stands in
/// before the first snapshot.
#[derive(Clone, Default)]
pub struct Held {
    pub weapon: u8,
    pub slots: [u8; 8],
    pub stance: Stance,
    /// `ps.weapons`: bit `n` while weapon `n` is carried.
    pub owned: u64,
    /// Bit `n` while weapon `n` has a round in its clip or reserve,
    /// `ammo[ammoIndex] + ammoclip[clipIndex] != 0` (cgame 0x300381b0).
    pub loaded: u64,
    /// Configstring 7's `altWeapon` links, index = weapon number, 0 none
    /// (the def's `+0x2fc`). Its length less one is the weapon count.
    pub alt: Vec<u8>,
    /// `pm_flags` 0x40000: the client looks out of its own live body, the
    /// gate on every weapon bind (0x30037f90..0x300382f0) and the cycle.
    /// Retail reads `cg.snap`'s ([`Held::cg_snap`]).
    pub own_view: bool,
    /// `(clientNum, stats[5])`: a new value is a respawn or a new followed
    /// client (0x300300b8). `None` before the first snapshot.
    pub spawn: Option<(i32, i32)>,
    /// `eFlags & 0xc000`: on a mounted gun, where `weapalt` refuses.
    pub mounted: bool,
    /// The server speaks United Offensive's protocol, whose cmd carries the
    /// sprint button.
    pub sprint_button: bool,
    /// United Offensive's seven weapon slots (`satchel` 6, `binocular` 7),
    /// against 1.1's and 1.5's five (cod11-hud-protocol.md, "Weapon slots").
    pub uo_slots: bool,
    /// In a UO seat whose rider may not fire his own weapon: `weaponslot`
    /// refuses (uo cgame 0x3004776c, `BG_AllowPlayerWeaponAtVehiclePos`
    /// inlined). The seat bit is the predicted `eFlags`, whose base is the
    /// newest snapshot; `vehType` and `vehPos` are `cg.snap`'s
    /// ([`Held::cg_snap`]).
    pub seat_refuses_slot: bool,
    /// The same test for `weapalt` (0x30047429), which reads the predicted
    /// `vehType` and `vehPos` too.
    pub seat_refuses_alt: bool,
}

/// UO's seat gate: the seat bit off `eflags_ps`, `vehType` 1 and `vehPos` 3
/// (the jeep passenger, who fires his own weapon) off `seat_ps`.
fn seat_refuses(p: &Protocol, eflags_ps: &PlayerState, seat_ps: &PlayerState) -> bool {
    p.is_uo()
        && eflags_ps.field_i32(p, "eFlags") & EF_IN_VEHICLE_V22 != 0
        && !(seat_ps.field_i32(p, "vehType") == 1 && seat_ps.field_i32(p, "vehPos") == 3)
}

impl Held {
    pub fn from_ps(ps: &PlayerState, p: &Protocol, weapons: &[Option<WeaponDef>]) -> Held {
        let lo = ps.field_i32(p, "weaponslots[0]").to_le_bytes();
        let hi = ps.field_i32(p, "weaponslots[4]").to_le_bytes();
        let mut slots = [0u8; 8];
        slots[..4].copy_from_slice(&lo);
        slots[4..].copy_from_slice(&hi);
        let owned = u64::from(ps.field_i32(p, "weapons[0]") as u32)
            | u64::from(ps.field_i32(p, "weapons[1]") as u32) << 32;
        let count = |a: &[i16; 64], i: usize| a.get(i).copied().map_or(0, i32::from);
        let mut loaded = 0;
        let mut alt = vec![0u8; weapons.len()];
        for (n, def) in weapons.iter().enumerate().take(64) {
            let Some(def) = def else { continue };
            if count(&ps.arrays.ammo, def.ammo_index) + count(&ps.arrays.ammoclip, def.clip_index)
                != 0
            {
                loaded |= 1 << n;
            }
            alt[n] = u8::try_from(def.alt_index).unwrap_or(0);
        }
        Held {
            weapon: ps.field_i32(p, "weapon") as u8,
            slots,
            stance: Stance::from_eflags(ps.field_i32(p, "eFlags")),
            owned,
            loaded,
            alt,
            own_view: ps.field_i32(p, "pm_flags") & PMF_OWN_VIEW != 0,
            spawn: Some((ps.field_i32(p, "clientNum"), ps.arrays.stats[5])),
            mounted: ps.field_i32(p, "eFlags") & EF_MOUNTED != 0,
            sprint_button: p.is_uo(),
            uo_slots: p.is_uo(),
            seat_refuses_slot: seat_refuses(p, ps, ps),
            seat_refuses_alt: seat_refuses(p, ps, ps),
        }
    }

    /// The fields the weapon binds read off `cg.snap`, the older snapshot
    /// of the drawn pair, where the rest of `Held` is the newest one's:
    /// the own-view bit (1.1 0x3003830c, UO 0x30047792) and `weaponslot`'s
    /// `vehType` and `vehPos` (UO 0x30047778). The two differ for a
    /// snapshot at a mount, a seat change or the start of a follow.
    pub fn cg_snap(&mut self, newest: &PlayerState, snap: &PlayerState, p: &Protocol) {
        self.own_view = snap.field_i32(p, "pm_flags") & PMF_OWN_VIEW != 0;
        self.seat_refuses_slot = seat_refuses(p, newest, snap);
    }

    /// The weapon, slots, carried and loaded bits off the predicted
    /// playerstate, which the out-of-ammo switch reads (cgame 0x300381b0
    /// reads `0x3020715c`, the predicted ps, not the snapshot's).
    pub fn predicted(
        &mut self,
        ps: &vcod_common::pmove::PlayerState,
        weapons: &[Option<WeaponDef>],
    ) {
        self.weapon = ps.weapon;
        self.slots = ps.weapon_slots;
        self.owned = ps.weapons_held;
        self.loaded = 0;
        for (n, def) in weapons.iter().enumerate().take(64) {
            let Some(def) = def else { continue };
            let count = |a: &[i16; 64], i: usize| a.get(i).copied().map_or(0, i32::from);
            if count(&ps.ammo, def.ammo_index) + count(&ps.ammoclip, def.clip_index) != 0 {
                self.loaded |= 1 << n;
            }
        }
    }

    /// The weapon slots, 1..=5 (`0x300382f0`) or UO's 1..=7 (uo cgame
    /// 0x30047750); 0 is `none`.
    fn slot_range(&self) -> std::ops::RangeInclusive<usize> {
        1..=self.slot_count()
    }

    fn slot_count(&self) -> usize {
        if self.uo_slots { 7 } else { 5 }
    }

    pub fn owns(&self, w: u8) -> bool {
        w < 64 && self.owned & 1 << w != 0
    }

    fn usable(&self, w: u8, need_ammo: bool) -> bool {
        !need_ammo || (w < 64 && self.loaded & 1 << w != 0)
    }

    fn alt_of(&self, w: u8) -> u8 {
        self.alt.get(usize::from(w)).copied().unwrap_or(0)
    }

    /// The highest weapon number, the cgame's weapon count (`0x300eef38`).
    fn count(&self) -> u8 {
        u8::try_from(self.alt.len().saturating_sub(1)).unwrap_or(u8::MAX)
    }

    /// The cgame's slot lookup with the alt chain followed (0x3000f5c0): the
    /// slot (1..=5, UO 1..=7) holding `w` or a weapon `w`'s `altWeapon` links lead to,
    /// 0 for none or for a weapon not carried. Stock files leave
    /// `slotStackable` 0, so only the slot bytes are read.
    fn slot_of(&self, w: u8) -> usize {
        if !self.owns(w) {
            return 0;
        }
        let mut at = w;
        loop {
            if let Some(n) = self.slot_range().find(|&n| self.slots[n] == at) {
                return n;
            }
            at = self.alt_of(at);
            if at == 0 || at == w {
                return 0;
            }
        }
    }

    /// Whether `of` sits further along `w`'s `altWeapon` chain (the walk at
    /// 0x30038661, which skips the selection's other modes).
    fn alt_mode_of(&self, w: u8, of: u8) -> bool {
        let mut at = self.alt_of(w);
        while at != 0 && at != w {
            if at == of {
                return true;
            }
            at = self.alt_of(at);
        }
        false
    }
}

/// What `weapalt` did with the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AltWeapon {
    Selected,
    /// The selection has no `altWeapon`: the cgame prints
    /// `CGAME_THIS_WEAPON_HAS_NO_ALTERNATE` as a game message.
    NoAlternate,
    /// Off our own view, on a mounted gun, or the mode is not carried.
    Refused,
}

/// The `cl_run` cvar. The ads bit is set when it equals the `+speed` key's
/// state (CoDMP.exe 0x40aefb), so 1, the default, aims while the key is held
/// and 0 while it is not: retail's `toggle cl_run` is its ads toggle.
#[derive(Clone, Copy, Debug)]
pub struct ClRun(pub i32);

impl Default for ClRun {
    fn default() -> Self {
        ClRun(1)
    }
}

/// Accumulates held keys, mouse motion and the weapon selection between
/// `build` calls, and turns them into one [`UserCmd`] per call.
#[derive(Default)]
pub struct PlayInput {
    down: HashSet<Action>,
    /// `cl_stance`, the stance the client keeps; level, and resynced to
    /// `Held::stance` whenever the server changes it without a matching key
    /// press (a forced stand at spawn, say) so the client does not fight
    /// it. A held `+prone` or `+movedown` overrides it in the cmd.
    wanted_stance: Stance,
    last_seen_stance: Stance,
    /// Retail's `cg_weaponSelect` (`0x301cbb0c`), the weapon every cmd
    /// asks for. Only a respawn, a bind, the `a` command, a pickup out of
    /// nothing selected and the out-of-ammo switch move it; the playerstate
    /// never does, so the ladder's empty hands and a mounted gun keep it
    /// (docs/research/cod11-combat.md, 9.8).
    selected: u8,
    /// The `Held::spawn` the selection was last reset for; `None` until the
    /// first snapshot of a gamestate.
    spawn: Option<(i32, i32)>,
    /// The newest playerstate seen, which a bind resolves against.
    held: Held,
    /// A select or a respawn reset `cl_run` to 1 since the last
    /// [`PlayInput::take_run_reset`].
    run_reset: bool,
    /// The held `+gostand` / `+moveup` keys that pressed the jump, which
    /// they do only standing (CoDMP.exe 0x40ad40, 0x40a670).
    jump_by: HashSet<Action>,
    /// Raw view angles, ANGLE2SHORT units, accumulated from mouse counts;
    /// pitch (index 0) down-positive, yaw (index 1) as `FlyCamera`'s own
    /// sign. Index 2 (roll) is unused. Unclamped: retail's server clamps
    /// pitch at 16000 short units, and the drawn view clamps the same way
    /// (`own_view` in main.rs).
    raw_angles: [i32; 3],
    /// The sub-unit rest of the mouse motion per axis [pitch, yaw], carried
    /// so slow motion still turns.
    mouse_rest: [f32; 2],
    pub cl_run: ClRun,
    /// The view kick, degrees per axis, wire convention
    /// (`play::recoil::Recoil::view_kick`).
    pub kick: [f32; 3],
    /// `wbuttons` 0x4 while the shellshock runs, set each frame like the
    /// kick (cgame syscall 0x57, `play::shock`).
    pub shellshock: bool,
    /// United Offensive's `wbuttons` 0x1 while the flame report holds
    /// (uo cgame trap 0x58, `fx::flame::FlameSystem::report`).
    pub in_flame: bool,
    /// `buttons` 0x2 (`BUTTON_TALK`) while a key catcher is up, set each
    /// frame from [`talking`].
    pub talking: bool,
    /// This frame's turn caps per axis [pitch, yaw], degrees, with the
    /// motion summed and the part let through so far: `CL_MouseMove`
    /// (CoDMP.exe 0x40b240) clamps a frame's turn under a shellshock.
    mouse_caps: [Option<f32>; 2],
    mouse_sum: [f32; 2],
    mouse_let: [f32; 2],
    /// The mouse's `forwardmove` and `rightmove` counts ([`MouseMove`])
    /// not yet in a cmd.
    mouse_axes: [i32; 2],
}

impl PlayInput {
    pub fn key(&mut self, action: Action, pressed: bool) {
        let was_down = self.down.contains(&action);
        if pressed {
            self.down.insert(action);
        } else {
            self.down.remove(&action);
            self.jump_by.remove(&action);
        }
        if pressed && !was_down {
            self.on_press(action);
        }
    }

    /// `+prone` or `+movedown` is down: the stance binds and the
    /// forced-stance events leave `cl_stance` alone (`cl_stanceTemp` 1,
    /// docs/research/cod11-console.md 9).
    pub fn stance_held(&self) -> bool {
        held_stance(&self.down).is_some()
    }

    fn on_press(&mut self, action: Action) {
        match stance_press(action, self.wanted_stance, self.stance_held()) {
            StancePress::Set(s) => self.wanted_stance = s,
            StancePress::Jump => {
                self.jump_by.insert(action);
            }
            StancePress::Nothing => {}
        }
        match action {
            Action::Slot(n) => self.weapon_slot(n),
            Action::NextWeapon => self.cycle(true, false),
            Action::PrevWeapon => self.cycle(false, false),
            _ => {}
        }
    }

    /// The stance an `EV_STANCE_FORCE_*` sets, retail cgame's write of
    /// `cl_stance` (0x3001df6d).
    pub fn force_stance(&mut self, stance: Stance) {
        self.wanted_stance = stance;
    }

    /// Lets go of every held key, for a grab release or focus loss. The
    /// stance and the weapon selection stay: neither is a held key.
    pub fn release_all(&mut self) {
        self.down.clear();
        self.jump_by.clear();
    }

    /// A new frame's turn caps, degrees a second per axis [pitch, yaw], 0
    /// for none, over a frame of `frame_ms`: `max * frame_ms * 0.001`.
    pub fn set_mouse_caps(&mut self, frame_ms: f32, max: [f32; 2]) {
        self.mouse_caps = max.map(|m| (m != 0.0).then_some(frame_ms * m * 0.001));
        self.mouse_sum = [0.0; 2];
        self.mouse_let = [0.0; 2];
    }

    /// Turns the view by `[pitch, yaw]` degrees ([`MouseLook::degrees`]),
    /// the frame's sum held inside its cap.
    pub fn mouse(&mut self, delta: [f32; 2]) {
        for (axis, mut d) in delta.into_iter().enumerate() {
            if let Some(cap) = self.mouse_caps[axis] {
                self.mouse_sum[axis] += d;
                let total = self.mouse_sum[axis].clamp(-cap, cap);
                d = total - self.mouse_let[axis];
                self.mouse_let[axis] = total;
            }
            let total = self.mouse_rest[axis] + d * SHORT_PER_DEG;
            let whole = total.trunc();
            self.raw_angles[axis] += whole as i32;
            self.mouse_rest[axis] = total - whole;
        }
    }

    /// Spends a frame's [`MouseMove`]: the turn now, the move counts in
    /// the next cmd built.
    pub fn mouse_move(&mut self, m: MouseMove) {
        self.mouse(m.turn);
        self.mouse_axes[0] += m.forward;
        self.mouse_axes[1] += m.side;
    }

    /// Whether `+mlook` and `+strafe` are held, which `CL_MouseMove` reads.
    pub fn mlook_strafe(&self) -> (bool, bool) {
        (
            self.down.contains(&Action::MLook),
            self.down.contains(&Action::Strafe),
        )
    }

    /// `-mlook` under `cl_freelook` 0 (CoDMP.exe 0x40a460): the view's
    /// pitch levelled, `cl.viewangles[PITCH] = -SHORT2ANGLE(delta_angles)`.
    pub fn center_view(&mut self, delta_pitch: i32) {
        self.raw_angles[0] = delta_pitch.wrapping_neg();
        self.mouse_rest[0] = 0.0;
    }

    /// The selection, `cg_weaponSelect`: the weapon every cmd asks for, and
    /// the one the HUD names and counts the ammo of.
    pub fn selected(&self) -> u8 {
        self.selected
    }

    /// Takes the newest playerstate. A new `(clientNum, stats[5])` is the
    /// cgame's respawn reset (0x30028a70): the selection becomes the
    /// playerstate's weapon, `cl_run` goes back to 1 and `cl_stance` to 0.
    pub fn sync(&mut self, held: &Held) {
        if let Some(spawn) = held.spawn
            && self.spawn.replace(spawn) != Some(spawn)
        {
            self.selected = held.weapon;
            self.run_reset = true;
            self.wanted_stance = Stance::Stand;
        }
        self.held.clone_from(held);
    }

    /// A map load: the first snapshot resets the selection whatever its
    /// spawn count (0x3002fa8f, 0x3003037f).
    pub fn new_gamestate(&mut self) {
        self.spawn = None;
    }

    /// `CG_MapRestart` sets `cg.mapRestart` (0x30207154), which takes the
    /// next snapshot through the same reset whatever its spawn count
    /// (0x30030009).
    pub fn map_restart(&mut self) {
        self.spawn = None;
    }

    /// Whether a select or a respawn reset `cl_run` to 1 since the last call:
    /// the caller writes the cvar, which the cgame sets through the engine.
    pub fn take_run_reset(&mut self) -> bool {
        std::mem::take(&mut self.run_reset)
    }

    /// The cgame's select (0x30037f20): a new weapon resets `cl_run` to 1
    /// unless it is the selection's `altWeapon`, a mode switch on one gun.
    fn select(&mut self, weapon: u8) {
        if weapon == self.selected {
            return;
        }
        let alt_mode = weapon != 0 && self.held.alt_of(self.selected) == weapon;
        self.selected = weapon;
        self.run_reset |= !alt_mode;
    }

    /// The gates a weapon bind runs before it stamps the weapon name and
    /// before `cg_weaponCycleDelay` (`weaponslot` 0x300382f0, the cycle
    /// 0x30038040, `weapalt` 0x30037f90; UO 0x30047750, 0x300474c0,
    /// 0x30047400): our own view, and for `weaponslot` and `weapalt` a UO
    /// seat that fires the rider's weapon, for `weapalt` no mounted gun. A
    /// select these pass stamps even when the slot or the mode is empty.
    pub fn select_allowed(&self, action: Action) -> bool {
        let h = &self.held;
        h.own_view
            && match action {
                Action::Slot(_) => !h.seat_refuses_slot,
                Action::AltWeapon => !h.mounted && !h.seat_refuses_alt,
                _ => true,
            }
    }

    /// `weapalt` (0x30037f90): the selection's `altWeapon`, when it is
    /// carried. Refused by [`PlayInput::select_allowed`].
    pub fn alt_weapon(&mut self) -> AltWeapon {
        if !self.select_allowed(Action::AltWeapon) {
            return AltWeapon::Refused;
        }
        let h = &self.held;
        let alt = h.alt_of(self.selected);
        if alt == 0 {
            return AltWeapon::NoAlternate;
        }
        if !h.owns(alt) {
            return AltWeapon::Refused;
        }
        self.select(alt);
        AltWeapon::Selected
    }

    /// `weapon <name|number>` (0x30038130): the weapon `index` resolves the
    /// name to, else the number, selected unchecked like `a`; a 0 does
    /// nothing. Off our own view it is refused. Returns whether it selected.
    pub fn weapon_command(&mut self, arg: &str, index: impl Fn(&str) -> Option<u8>) -> bool {
        if !self.held.own_view {
            return false;
        }
        // 0x3000f260's miss falls through to `atol`, which reads a leading
        // number and 0 for anything else.
        let w = index(arg)
            .filter(|&w| w != 0)
            .unwrap_or_else(|| atoi_msvc(arg) as u8);
        if w == 0 {
            return false;
        }
        self.select(w);
        true
    }

    /// The server's `a <weapon>` (0x3002e139): selects it unchecked, a value
    /// past the weapon count or below 0 reading 0, as the frame's clamp
    /// leaves it (0x30033e3e..0x30033e94).
    pub fn server_select(&mut self, arg: &str) {
        let w = arg.trim().parse::<i64>().unwrap_or(0);
        let w = u8::try_from(w)
            .ok()
            .filter(|&w| w <= self.held.count())
            .unwrap_or(0);
        self.select(w);
    }

    /// A weapon item picked up by the view's own body (`CG_ItemPickup`,
    /// 0x3001da80): it becomes the selection only while nothing is selected,
    /// and leaves `cl_run` alone.
    pub fn picked_up(&mut self, weapon: u8) {
        if self.selected == 0 {
            self.selected = weapon;
        }
    }

    /// The predicted playerstate's weapons over the newest snapshot's, for
    /// [`PlayInput::out_of_ammo`] (see [`Held::predicted`]). The next
    /// [`PlayInput::sync`] puts the snapshot's back.
    pub fn predict_held(
        &mut self,
        ps: &vcod_common::pmove::PlayerState,
        weapons: &[Option<WeaponDef>],
    ) {
        self.held.predicted(ps, weapons);
    }

    /// `EV_NOAMMO` or `EV_DROPWEAPON` on the view's own body (0x300381b0):
    /// with the held weapon in a slot, the first slot from 1 whose weapon has
    /// a round left, else the cycle forward over loaded weapons.
    pub fn out_of_ammo(&mut self) {
        let h = &self.held;
        if h.slot_of(h.weapon) != 0
            && let Some(w) = h
                .slot_range()
                .map(|n| h.slots[n])
                .find(|&w| w != 0 && h.usable(w, true))
        {
            self.select(w);
            return;
        }
        self.cycle(true, true);
    }

    /// `weaponslot` (0x300382f0): the slot's weapon, when it holds one the
    /// player carries. Slot 0 and one past the protocol's last do nothing.
    fn weapon_slot(&mut self, n: usize) {
        let h = &self.held;
        if !self.select_allowed(Action::Slot(n)) || !h.slot_range().contains(&n) {
            return;
        }
        let w = h.slots[n];
        if w != 0 && h.owns(w) {
            self.select(w);
        }
    }

    /// `weapnext` (`forward`) and `weapprev` (0x30038040, 0x300380b0), and
    /// the out-of-ammo fallback with `need_ammo`: the cycle 0x30038570.
    fn cycle(&mut self, forward: bool, need_ammo: bool) {
        if !self.held.own_view {
            return;
        }
        if let Some(w) = cycle_pick(&self.held, self.selected, forward, need_ammo) {
            self.select(w);
        }
        // 0x300387d3: what is left selected is dropped once no longer carried.
        if self.selected != 0 && !self.held.owns(self.selected) {
            self.selected = 0;
            self.run_reset = true;
        }
    }

    /// The cgame's `trap_SetViewAngles` (uo cgame trap 0xf7): the view put
    /// at `[pitch, yaw]` degrees, wire convention, over `delta_angles`.
    pub fn set_view(&mut self, view: [f32; 2], delta: [i32; 3]) {
        for (i, deg) in view.into_iter().enumerate() {
            self.raw_angles[i] = ((deg * SHORT_PER_DEG) as i32).wrapping_sub(delta[i]);
            self.mouse_rest[i] = 0.0;
        }
    }

    /// The angles a cmd carries: `CL_FinishMove` (CoDMP.exe `0x40b6b7`)
    /// sends `ANGLE2SHORT(cl.viewangles + kick)` truncated, so the server
    /// aims the shot where the kicked view points (combat doc, 15.7). The
    /// kick is truncated on its own here, a unit off retail's at most.
    pub fn cmd_angles(&self) -> [i32; 3] {
        std::array::from_fn(|i| self.raw_angles[i] + (self.kick[i] * SHORT_PER_DEG) as i32)
    }

    pub fn build(&mut self, server_time: i32, held: &Held) -> UserCmd {
        // A stance the held key put the body in is not one to keep.
        let key_stance = held_stance(&self.down);
        if held.stance != self.last_seen_stance && key_stance.is_none() {
            self.wanted_stance = held.stance;
        }
        self.last_seen_stance = held.stance;

        self.sync(held);
        let weapon = self.selected;

        let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as i8 * 127;

        // `CL_KeyMove` (CoDMP.exe 0x40ae90): the held key's stance with
        // `wbuttons` 0x2, else `cl_stance`; the jump's 127 and a stance's
        // -127 add (0x40afc2..0x40b010).
        let mut wbuttons = 0u8;
        let mut up = if self.jump_by.is_empty() { 0 } else { 127 };
        if key_stance.is_some() {
            wbuttons |= msg::WBUTTON_STANCE_HELD;
        }
        match key_stance.unwrap_or(self.wanted_stance) {
            Stance::Crouch => {
                wbuttons |= msg::WBUTTON_CROUCH;
                up -= 127;
            }
            Stance::Prone => {
                wbuttons |= msg::WBUTTON_PRONE;
                up -= 127;
            }
            Stance::Stand => {}
        }
        if self.down.contains(&Action::LeanLeft) {
            wbuttons |= msg::WBUTTON_LEAN_LEFT;
        }
        if self.down.contains(&Action::LeanRight) {
            wbuttons |= msg::WBUTTON_LEAN_RIGHT;
        }
        if self.down.contains(&Action::Reload) {
            wbuttons |= msg::WBUTTON_RELOAD;
        }
        if self.shellshock {
            wbuttons |= msg::WBUTTON_SHELLSHOCK;
        }
        if self.in_flame {
            wbuttons |= msg::WBUTTON_IN_FLAME_V22;
        }

        let mut buttons = 0u8;
        if self.down.contains(&Action::Attack) {
            buttons |= msg::BUTTON_ATTACK;
        }
        if self.cl_run.0 == self.down.contains(&Action::Ads) as i32 {
            buttons |= msg::BUTTON_ADS;
        }
        if self.down.contains(&Action::Melee) {
            buttons |= msg::BUTTON_MELEE;
        }
        if self.down.contains(&Action::Use) {
            buttons |= msg::BUTTON_USE;
        }
        if held.sprint_button && self.down.contains(&Action::Sprint) {
            buttons |= msg::BUTTON_SPRINT_V22;
        }
        if self.talking {
            buttons |= msg::BUTTON_TALK;
        }

        // `CL_MouseMove` adds its counts after the keys, each clamped to a
        // signed byte (0x40b412, 0x40b5d9).
        let [mouse_fwd, mouse_side] = std::mem::take(&mut self.mouse_axes);
        let byte = |v: i32| v.clamp(-128, 127) as i8;
        let forward = axis(
            self.down.contains(&Action::Forward),
            self.down.contains(&Action::Back),
        );
        let right = axis(
            self.down.contains(&Action::Right),
            self.down.contains(&Action::Left),
        );

        UserCmd {
            server_time,
            buttons,
            wbuttons,
            weapon,
            angles: self.cmd_angles(),
            forward: byte(i32::from(forward) + mouse_fwd),
            right: byte(i32::from(right) + mouse_side),
            up,
            ..Default::default()
        }
    }
}

/// `CL_CmdButtons`' talk bit (CoDMP.exe 0x40b600, 1.5 0x40c130, UO
/// 0x40c700): any key catcher up (the console, a menu, the chat line) with
/// `cl_bypassMouseInput` 0, so the quick message menus, which set it to 1,
/// never send it (docs/research/cod11-console.md, "The talk button").
pub fn talking(console: bool, menus: bool, chat: bool, bypass_mouse: i32) -> bool {
    (console || menus || chat) && bypass_mouse == 0
}

/// The cycle's pick (cgame 0x30038570), from the selection `from`. With
/// `from` in a slot: the slots past it toward the last (`forward`) or toward 1,
/// then the carried weapons in no slot, then the first slot from the end the
/// walk started at. With `from` in none: the weapon numbers from it,
/// wrapping, skipping its own alt modes and every slotted weapon, then the
/// first slot, then the weapons in none. `need_ammo` skips the empty ones.
fn cycle_pick(h: &Held, from: u8, forward: bool, need_ammo: bool) -> Option<u8> {
    let dir: i32 = if forward { 1 } else { -1 };
    let first_slot = || {
        let mut order: Vec<usize> = h.slot_range().collect();
        if !forward {
            order.reverse();
        }
        order
            .into_iter()
            .map(|n| h.slots[n])
            .find(|&w| w != 0 && h.usable(w, need_ammo))
    };
    // 0x30038430. Forward stops short of the last weapon number, as retail's.
    let unslotted = || {
        let n = i32::from(h.count());
        let mut w = if forward { 1 } else { n };
        loop {
            if w < 1 || w > n {
                return None;
            }
            let wu = w as u8;
            if h.owns(wu) && h.slot_of(wu) == 0 && h.usable(wu, need_ammo) {
                return Some(wu);
            }
            w += dir;
            if w == 0 || w == n {
                return None;
            }
        }
    };
    let slot = h.slot_of(from);
    if slot == 0 {
        let n = i32::from(h.count());
        if n == 0 {
            return None;
        }
        let stop = if forward { 1 } else { n };
        let mut w = i32::from(from);
        loop {
            w = (n - 1 + dir + w).rem_euclid(n) + 1;
            if w == stop {
                return first_slot().or_else(unslotted);
            }
            let wu = w as u8;
            if h.owns(wu)
                && !h.alt_mode_of(wu, from)
                && h.slot_of(wu) == 0
                && h.usable(wu, need_ammo)
            {
                return Some(wu);
            }
        }
    }
    // UO wraps over its seven slots (uo cgame 0x300479d8).
    let count = h.slot_count() as i32;
    let end = if forward { 1 } else { count };
    let mut s = slot as i32;
    loop {
        s = (s + count - 1 + dir).rem_euclid(count) + 1;
        if s == end {
            break;
        }
        let w = h.slots[s as usize];
        if w != 0 && h.usable(w, need_ammo) {
            return Some(w);
        }
    }
    unslotted().or_else(first_slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot 1 holds 10, slot 3 holds 3, slot 4 holds 6; 11 is 10's alt
    /// mode, 20 a carried weapon in no slot. Everything is loaded, from our
    /// own view, on spawn 1, out of 32 weapons.
    fn held(weapon: u8) -> Held {
        let mut alt = vec![0; 33];
        alt[10] = 11;
        alt[11] = 10;
        let owned = [3, 6, 10, 11].iter().fold(0, |m, w| m | 1u64 << w);
        Held {
            weapon,
            slots: [0, 10, 0, 3, 6, 0, 0, 0],
            stance: Stance::Stand,
            owned,
            loaded: owned,
            alt,
            own_view: true,
            spawn: Some((0, 1)),
            mounted: false,
            sprint_button: false,
            uo_slots: false,
            seat_refuses_slot: false,
            seat_refuses_alt: false,
        }
    }

    #[test]
    fn sprint_reaches_only_a_uo_cmd() {
        let mut i = synced(10);
        i.key(Action::Sprint, true);
        assert_eq!(i.build(0, &held(10)).buttons & msg::BUTTON_SPRINT_V22, 0);
        let uo = Held {
            sprint_button: true,
            ..held(10)
        };
        assert_ne!(i.build(8, &uo).buttons & msg::BUTTON_SPRINT_V22, 0);
    }

    /// In a UO seat the stock keys are the vehicle's controls
    /// (coduo-vehicles.md 2): forward and the strafe keys drive and steer,
    /// `+gostand` holds `upmove` (a tank's hull to the view), the trigger
    /// fires the main gun, `+melee` the alternate gun, `+activate` gets out.
    /// The seated capture's cmds carry the same bytes.
    #[test]
    fn a_seat_drives_with_the_stock_keys() {
        let mut i = synced(10);
        for a in [
            Action::Forward,
            Action::Right,
            Action::Jump,
            Action::Attack,
            Action::Melee,
            Action::Use,
        ] {
            i.key(a, true);
        }
        let c = i.build(0, &held(10));
        assert_eq!((c.forward, c.right, c.up), (127, 127, 127));
        assert_eq!(
            c.buttons & (msg::BUTTON_ATTACK | msg::BUTTON_MELEE | msg::BUTTON_USE),
            msg::BUTTON_ATTACK | msg::BUTTON_MELEE | msg::BUTTON_USE
        );
    }

    /// `trap_SetViewAngles` puts the cmd's view where it is asked to be,
    /// over whatever `delta_angles` hold.
    #[test]
    fn set_view_lands_the_cmd_on_the_view() {
        let mut i = synced(10);
        i.mouse([3.3, -40.7]);
        let delta = [1234, 60000, 0];
        i.set_view([-10.0, 45.0], delta);
        let c = i.build(0, &held(10));
        let view = |k: usize| ((c.angles[k] + delta[k]) as i16) as f32 * 360.0 / 65536.0;
        assert!((view(0) + 10.0).abs() < 0.01, "{}", view(0));
        assert!((view(1) - 45.0).abs() < 0.01, "{}", view(1));
    }

    /// `held(weapon)` already seen, so binds resolve against it.
    fn synced(weapon: u8) -> PlayInput {
        let mut i = PlayInput::default();
        i.sync(&held(weapon));
        i
    }

    fn press(i: &mut PlayInput, a: Action) {
        i.key(a, true);
        i.key(a, false);
    }

    #[test]
    fn the_cmd_carries_the_held_weapon() {
        let mut i = PlayInput::default();
        assert_eq!(i.build(100, &held(10)).weapon, 10);
    }

    /// `cg_weaponSelect` is the client's: a slot key moves it and the
    /// playerstate never does (cgame 0x300382f0, 0x30033ede).
    #[test]
    fn a_slot_key_selects_and_the_playerstate_does_not_move_it() {
        let mut i = synced(10);
        press(&mut i, Action::Slot(3));
        assert_eq!(i.build(100, &held(10)).weapon, 3);
        assert_eq!(i.build(116, &held(3)).weapon, 3);
        // The server put 10 back on its own: the cmd still asks for 3.
        assert_eq!(i.build(124, &held(10)).weapon, 3);
    }

    /// 0x30028a70: a new `(clientNum, stats[5])` or a new gamestate takes
    /// the playerstate's weapon; nothing before the first snapshot does.
    #[test]
    fn a_respawn_or_a_map_load_takes_the_playerstate_weapon() {
        let mut i = synced(10);
        press(&mut i, Action::Slot(3));
        assert_eq!(i.build(100, &Held::default()).weapon, 3, "no snapshot");
        let respawn = Held {
            spawn: Some((0, 2)),
            ..held(6)
        };
        assert_eq!(i.build(108, &respawn).weapon, 6);
        press(&mut i, Action::Slot(1));
        assert_eq!(i.build(116, &respawn).weapon, 10);
        i.new_gamestate();
        assert_eq!(i.build(124, &respawn).weapon, 6, "same spawn, new map");
        // Following another client is a new `clientNum`.
        let followed = Held {
            spawn: Some((4, 2)),
            ..held(3)
        };
        assert_eq!(i.build(132, &followed).weapon, 3);
    }

    /// Following or spectating, the binds do nothing (the `pm_flags`
    /// 0x40000 gate at 0x30038079 and its siblings).
    #[test]
    fn binds_need_our_own_view() {
        let follower = Held {
            own_view: false,
            ..held(10)
        };
        let mut i = PlayInput::default();
        i.sync(&follower);
        for a in [Action::Slot(3), Action::NextWeapon, Action::PrevWeapon] {
            press(&mut i, a);
        }
        assert_eq!(i.build(100, &follower).weapon, 10);
    }

    /// UO's `satchel` and `binocular` slots (6, 7) answer `weaponslot` and
    /// sit in the cycle (uo cgame 0x30047750, 0x300479d8); on 1.1 slot 6 is
    /// past the table and does nothing, and the cycle wraps at 5. In a seat
    /// that does not fire the rider's weapon UO refuses the slot.
    #[test]
    fn uo_has_seven_slots() {
        let five = Held {
            slots: [0, 10, 0, 3, 6, 7, 9, 8],
            owned: [3, 6, 7, 8, 9, 10, 11].iter().fold(0, |m, w| m | 1u64 << w),
            ..held(10)
        };
        let five = Held {
            loaded: five.owned,
            ..five
        };
        let seven = Held {
            uo_slots: true,
            ..five.clone()
        };
        let pick = |h: &Held, a: Action| {
            let mut i = PlayInput::default();
            i.sync(h);
            press(&mut i, a);
            i.selected()
        };
        assert_eq!(pick(&five, Action::Slot(5)), 7);
        assert_eq!(pick(&five, Action::Slot(6)), 10);
        assert_eq!(pick(&five, Action::Slot(0)), 10);
        assert_eq!(pick(&seven, Action::Slot(6)), 9);
        assert_eq!(pick(&seven, Action::Slot(7)), 8);
        assert_eq!(pick(&seven, Action::Slot(8)), 10);
        // On from slot 5: 1.1 takes the lowest unslotted weapon, UO slot 6.
        let on_7 = |h: &Held| Held {
            weapon: 7,
            ..h.clone()
        };
        assert_eq!(pick(&on_7(&five), Action::NextWeapon), 8);
        assert_eq!(pick(&on_7(&seven), Action::NextWeapon), 9);
        let seated = Held {
            seat_refuses_slot: true,
            ..seven.clone()
        };
        assert_eq!(pick(&seated, Action::Slot(3)), 10);
    }

    /// `CG_ItemPickup` (0x3001da80) selects a weapon item only while the
    /// selection is 0, not while the hands are empty: a climber picking up
    /// a gun keeps asking for the rifle it climbed with.
    #[test]
    fn a_pickup_selects_only_out_of_nothing() {
        let mut i = synced(10);
        i.sync(&held(0));
        i.picked_up(6);
        assert_eq!(i.selected(), 10);
        let mut i = synced(0);
        i.picked_up(6);
        assert_eq!(i.selected(), 6);
        assert!(i.take_run_reset(), "the respawn reset's");
        i.picked_up(3);
        assert_eq!(i.selected(), 6);
        assert!(!i.take_run_reset(), "a pickup leaves cl_run alone");
    }

    /// 0x300381b0, off `EV_NOAMMO` or `EV_DROPWEAPON`: the first slot from
    /// 1 whose weapon has a round, so the last frag thrown raises the rifle.
    #[test]
    fn running_dry_selects_the_first_loaded_slot() {
        let mut i = synced(10);
        press(&mut i, Action::Slot(4));
        let dry = Held {
            loaded: held(6).loaded & !(1 << 6 | 1 << 10 | 1 << 11),
            ..held(6)
        };
        i.sync(&dry);
        i.out_of_ammo();
        assert_eq!(i.selected(), 3, "slot 1 is dry too");
        // Nothing loaded anywhere: the selection stays.
        let mut i = synced(10);
        i.sync(&Held {
            loaded: 0,
            ..held(10)
        });
        i.out_of_ammo();
        assert_eq!(i.selected(), 10);
    }

    /// Empty hands are in no slot, so the switch runs the cycle forward
    /// over loaded weapons (0x300381b0's fallback, 0x30038570(1, 1)).
    #[test]
    fn running_dry_with_empty_hands_cycles_from_the_selection() {
        let mut i = synced(10);
        press(&mut i, Action::Slot(3));
        let dry = Held {
            loaded: 1 << 6,
            ..held(0)
        };
        i.sync(&dry);
        i.out_of_ammo();
        assert_eq!(i.selected(), 6);
    }

    /// `a <n>` (0x3002e139) selects unchecked; out of range reads 0
    /// (0x30033e3e).
    #[test]
    fn the_server_a_command_selects() {
        let mut i = synced(10);
        i.server_select("6");
        assert_eq!(i.build(100, &held(10)).weapon, 6);
        i.server_select("33");
        assert_eq!(i.selected(), 0);
        i.server_select("junk");
        assert_eq!(i.selected(), 0);
    }

    /// The select (0x30037f20) resets `cl_run` to 1 unless it is a mode
    /// switch on the same gun.
    #[test]
    fn a_select_resets_cl_run_unless_it_is_the_alt_mode() {
        let mut i = synced(10);
        assert!(i.take_run_reset(), "the respawn reset");
        i.server_select("11");
        assert!(!i.take_run_reset());
        i.server_select("11");
        assert!(!i.take_run_reset(), "no change");
        press(&mut i, Action::Slot(3));
        assert!(i.take_run_reset());
    }

    /// The select keeps `cl_run` when the new weapon is the old selection's
    /// `altWeapon` (0x30037f3e compares the new weapon against the old
    /// def's `+0x2fc`), so a one-way chain resets it going back.
    #[test]
    fn the_cl_run_rule_reads_the_old_selections_alt() {
        let mut h = held(10);
        h.alt[11] = 0;
        let mut i = PlayInput::default();
        i.sync(&h);
        i.take_run_reset();
        i.server_select("11");
        assert!(!i.take_run_reset(), "11 is 10's alt");
        i.server_select("10");
        assert!(i.take_run_reset(), "10 is not 11's alt");
    }

    /// `weapalt` (0x30037f90) selects the selection's `altWeapon` when it is
    /// carried, says so when there is none, and refuses on a mounted gun.
    #[test]
    fn weapalt_selects_the_alt_mode() {
        let mut i = synced(10);
        i.take_run_reset();
        assert_eq!(i.alt_weapon(), AltWeapon::Selected);
        assert_eq!(i.selected(), 11);
        assert!(!i.take_run_reset(), "a mode switch keeps cl_run");
        assert_eq!(i.alt_weapon(), AltWeapon::Selected);
        assert_eq!(i.selected(), 10);

        let mut i = synced(3);
        assert_eq!(i.alt_weapon(), AltWeapon::NoAlternate);
        assert_eq!(i.selected(), 3);

        let mut h = held(10);
        h.mounted = true;
        let mut i = PlayInput::default();
        i.sync(&h);
        assert_eq!(i.alt_weapon(), AltWeapon::Refused);
        let mut h = held(10);
        h.owned &= !(1 << 11);
        i.sync(&h);
        assert_eq!(i.alt_weapon(), AltWeapon::Refused);
        assert_eq!(i.selected(), 10);
    }

    /// UO's `weapalt` (0x30047429) refuses in a seat that does not fire the
    /// rider's weapon, as `weaponslot` does; the jeep passenger switches.
    #[test]
    fn uo_weapalt_refuses_in_a_seat() {
        let mut h = held(10);
        h.seat_refuses_alt = true;
        let mut i = PlayInput::default();
        i.sync(&h);
        assert_eq!(i.alt_weapon(), AltWeapon::Refused);
        assert_eq!(i.selected(), 10);
        h.seat_refuses_alt = false;
        i.sync(&h);
        assert_eq!(i.alt_weapon(), AltWeapon::Selected);
    }

    /// The gates that run before a bind stamps the weapon name: off our own
    /// view nothing passes, a refusing seat stops `weaponslot` and `weapalt`
    /// but not the cycle, and an empty or out-of-range slot still passes.
    #[test]
    fn select_gates_run_before_the_stamp() {
        let binds = [
            Action::Slot(1),
            Action::Slot(0),
            Action::Slot(8),
            Action::NextWeapon,
            Action::PrevWeapon,
            Action::AltWeapon,
        ];
        let i = synced(10);
        assert!(binds.iter().all(|&b| i.select_allowed(b)));
        let mut h = held(10);
        h.own_view = false;
        let mut i = PlayInput::default();
        i.sync(&h);
        assert!(binds.iter().all(|&b| !i.select_allowed(b)));
        let mut h = held(10);
        h.seat_refuses_slot = true;
        h.seat_refuses_alt = true;
        i.sync(&h);
        let passed: Vec<bool> = binds.iter().map(|&b| i.select_allowed(b)).collect();
        assert_eq!(passed, [false, false, false, true, true, false]);
    }

    /// The own-view bit and `weaponslot`'s seat come off `cg.snap`, the
    /// older of the drawn pair (1.1 0x3003830c, UO 0x30047778..0x30047792):
    /// a snapshot after the newest turns the bit on, the binds still refuse.
    #[test]
    fn the_own_view_and_the_slot_seat_read_cg_snap() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let ps = |pm_flags: i32, eflags: i32, veh_type: i32, veh_pos: i32| {
            let mut ps = PlayerState::null(p);
            for (name, v) in [
                ("pm_flags", pm_flags),
                ("eFlags", eflags),
                ("vehType", veh_type),
                ("vehPos", veh_pos),
            ] {
                ps.fields[PlayerState::field_index(p, name).expect(name)] = v;
            }
            ps
        };
        // Newest: the jeep's passenger, own view; cg.snap: still its driver,
        // and following.
        let newest = ps(PMF_OWN_VIEW, EF_IN_VEHICLE_V22, 1, 3);
        let snap = ps(0, EF_IN_VEHICLE_V22, 1, 1);
        let mut h = Held::from_ps(&newest, p, &[]);
        assert!(h.own_view && !h.seat_refuses_slot && !h.seat_refuses_alt);
        h.cg_snap(&newest, &snap, p);
        assert!(!h.own_view);
        assert!(h.seat_refuses_slot, "cg.snap's vehPos 1");
        assert!(!h.seat_refuses_alt, "the predicted vehPos 3");
        // Off the seat in the newest, cg.snap's seat fields no longer count.
        let afoot = ps(PMF_OWN_VIEW, 0, 1, 1);
        h.cg_snap(&afoot, &snap, p);
        assert!(!h.seat_refuses_slot);
    }

    /// `weapon` (0x30038130) takes a name, else a number, unchecked against
    /// what is carried; 0 and junk do nothing.
    #[test]
    fn the_weapon_command_takes_a_name_or_a_number() {
        let index = |n: &str| (n == "colt_mp").then_some(3);
        let mut i = synced(10);
        assert!(i.weapon_command("colt_mp", index));
        assert_eq!(i.selected(), 3);
        assert!(i.weapon_command("20", index));
        assert_eq!(i.selected(), 20, "not carried, still selected");
        assert!(!i.weapon_command("junk", index));
        assert!(!i.weapon_command("", index));
        assert_eq!(i.selected(), 20);
        let mut h = held(10);
        h.own_view = false;
        i.sync(&h);
        assert!(!i.weapon_command("6", index));
    }

    /// The out-of-ammo switch reads the predicted playerstate's ammo when
    /// the caller hands it over (0x300381b0 reads `0x3020715c`).
    #[test]
    fn out_of_ammo_reads_the_predicted_ammo() {
        let mut defs: Vec<Option<WeaponDef>> = vec![None; 33];
        for (w, ammo) in [(3u8, 3usize), (6, 6), (10, 10), (11, 10)] {
            defs[usize::from(w)] = Some(WeaponDef {
                ammo_index: ammo,
                clip_index: ammo,
                ..WeaponDef::default()
            });
        }
        let mut i = synced(10);
        let mut ps = vcod_common::pmove::PlayerState::spawn(glam::Vec3::ZERO, 0.0);
        ps.weapon = 10;
        ps.weapons_held = i.held.owned;
        ps.weapon_slots = i.held.slots;
        // The snapshot still reads slot 1 loaded; the prediction has it dry
        // and the pistol empty, so the frag in slot 4 is the first with a
        // round.
        ps.ammoclip[6] = 1;
        i.predict_held(&ps, &defs);
        i.out_of_ammo();
        assert_eq!(i.selected(), 6);
    }

    /// From an alt mode the slot is its base weapon's, and the walk over
    /// weapons in no slot skips alt modes (0x3000f5c0 follows the chain).
    #[test]
    fn the_cycle_follows_alt_modes_to_their_slot() {
        let mut i = synced(11);
        press(&mut i, Action::NextWeapon);
        assert_eq!(i.selected(), 3);
        let mut i = synced(3);
        press(&mut i, Action::PrevWeapon);
        assert_eq!(i.selected(), 10, "not 11");
    }

    /// Past slot 5 the walk takes the carried weapons in no slot, then wraps
    /// to the first slot (0x30038430, 0x300383b0).
    #[test]
    fn the_cycle_takes_unslotted_weapons_before_wrapping() {
        let with_20 = Held {
            owned: held(6).owned | 1 << 20,
            loaded: held(6).owned | 1 << 20,
            ..held(6)
        };
        let mut i = PlayInput::default();
        i.sync(&with_20);
        press(&mut i, Action::NextWeapon);
        assert_eq!(i.selected(), 20);
        // From a weapon in no slot: the numbers past it, then slot 1.
        press(&mut i, Action::NextWeapon);
        assert_eq!(i.selected(), 10);
    }

    /// The cycle drops a selection no longer carried when it finds nothing
    /// else (0x300387d3).
    #[test]
    fn the_cycle_drops_a_selection_no_longer_carried() {
        let mut i = synced(10);
        let bare = Held {
            owned: 0,
            loaded: 0,
            slots: [0; 8],
            ..held(0)
        };
        i.sync(&bare);
        press(&mut i, Action::NextWeapon);
        assert_eq!(i.selected(), 0);
    }

    #[test]
    fn a_second_notch_steps_from_the_selection() {
        // slot 1 holds 10 (in hand), slot 3 holds 3, slot 4 holds 6.
        let mut i = synced(10);
        i.key(Action::NextWeapon, true);
        i.key(Action::NextWeapon, false);
        assert_eq!(i.build(100, &held(10)).weapon, 3);
        i.key(Action::NextWeapon, true);
        i.key(Action::NextWeapon, false);
        assert_eq!(i.build(108, &held(10)).weapon, 6);
        // Two notches inside one cmd both count; the second wraps back to
        // the weapon in hand, which cancels the switch.
        i.key(Action::PrevWeapon, true);
        i.key(Action::PrevWeapon, false);
        i.key(Action::PrevWeapon, true);
        i.key(Action::PrevWeapon, false);
        assert_eq!(i.build(116, &held(10)).weapon, 10);
    }

    /// The ladder takes `ps.weapon` to 0 and gives nothing back on its own:
    /// retail's pickup raises whatever the cmd names, so the cmd keeps the
    /// rifle the player climbed with.
    #[test]
    fn empty_hands_keep_asking_for_the_weapon_held_before() {
        let mut i = PlayInput::default();
        assert_eq!(i.build(100, &held(10)).weapon, 10);
        assert_eq!(i.build(108, &held(0)).weapon, 10);
        // A wheel notch steps from it, as from the weapon in hand.
        i.key(Action::NextWeapon, true);
        i.key(Action::NextWeapon, false);
        assert_eq!(i.build(116, &held(0)).weapon, 3);
    }

    #[test]
    fn empty_slot_requests_nothing() {
        let mut i = synced(10);
        i.key(Action::Slot(2), true);
        assert_eq!(i.build(100, &held(10)).weapon, 10);
    }

    #[test]
    fn crouch_holds_up_down_until_space_stands() {
        let mut i = synced(10);
        i.key(Action::Crouch, true);
        i.key(Action::Crouch, false);
        let c = i.build(100, &held(10));
        assert_ne!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
        assert_eq!(c.up, -127);
        i.key(Action::Jump, true);
        i.key(Action::Jump, false);
        let c = i.build(108, &held(10));
        assert_eq!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
        assert_eq!(c.up, 0);
    }

    #[test]
    fn c_while_crouched_stays_crouched() {
        let mut i = synced(10);
        for t in [100, 108] {
            i.key(Action::Crouch, true);
            i.key(Action::Crouch, false);
            assert_ne!(i.build(t, &held(10)).wbuttons & msg::WBUTTON_CROUCH, 0);
        }
    }

    #[test]
    fn ctrl_while_prone_stays_prone() {
        let mut i = synced(10);
        for t in [100, 108] {
            i.key(Action::Prone, true);
            i.key(Action::Prone, false);
            assert_ne!(i.build(t, &held(10)).wbuttons & msg::WBUTTON_PRONE, 0);
        }
    }

    #[test]
    fn c_and_ctrl_switch_between_crouch_and_prone() {
        let mut i = synced(10);
        i.key(Action::Prone, true);
        i.key(Action::Prone, false);
        i.key(Action::Crouch, true);
        let c = i.build(100, &held(10));
        assert_eq!(
            c.wbuttons & (msg::WBUTTON_CROUCH | msg::WBUTTON_PRONE),
            msg::WBUTTON_CROUCH
        );
    }

    #[test]
    fn space_stands_from_prone_and_jumps_when_standing() {
        let mut i = PlayInput::default();
        i.key(Action::Prone, true);
        i.key(Action::Prone, false);
        i.key(Action::Jump, true);
        let c = i.build(100, &held(10));
        assert_eq!(c.wbuttons & msg::WBUTTON_PRONE, 0);
        assert_eq!(c.up, 0);
        i.key(Action::Jump, false);
        i.key(Action::Jump, true);
        assert_eq!(i.build(108, &held(10)).up, 127);
    }

    const STANCE_BITS: u8 = msg::WBUTTON_CROUCH | msg::WBUTTON_PRONE | msg::WBUTTON_STANCE_HELD;

    /// A held `+movedown` crouches with 0x2 and -127 for as long as it is
    /// down, and its release goes back to the kept stance.
    #[test]
    fn held_movedown_crouches_until_released() {
        let mut i = synced(10);
        i.key(Action::MoveDown, true);
        let c = i.build(100, &held(10));
        assert_eq!(
            c.wbuttons & STANCE_BITS,
            msg::WBUTTON_CROUCH | msg::WBUTTON_STANCE_HELD
        );
        assert_eq!(c.up, -127);
        // The server's crouch while held is not taken as the kept stance.
        let crouched = Held {
            stance: Stance::Crouch,
            ..held(10)
        };
        assert_eq!(i.build(108, &crouched).up, -127);
        i.key(Action::MoveDown, false);
        let c = i.build(116, &crouched);
        assert_eq!(c.wbuttons & STANCE_BITS, 0);
        assert_eq!(c.up, 0);
    }

    /// `+prone` wins over `+movedown`, and neither moves `cl_stance`: the
    /// stance binds do nothing while one is held.
    #[test]
    fn held_prone_overrides_and_freezes_the_kept_stance() {
        let mut i = synced(10);
        i.key(Action::Crouch, true);
        i.key(Action::Crouch, false);
        i.key(Action::MoveDown, true);
        i.key(Action::HoldProne, true);
        assert!(i.stance_held());
        for a in [Action::Jump, Action::ToggleCrouch, Action::Prone] {
            i.key(a, true);
            i.key(a, false);
        }
        let c = i.build(100, &held(10));
        assert_eq!(
            c.wbuttons & STANCE_BITS,
            msg::WBUTTON_PRONE | msg::WBUTTON_STANCE_HELD
        );
        assert_eq!(c.up, -127);
        i.key(Action::HoldProne, false);
        i.key(Action::MoveDown, false);
        assert!(!i.stance_held());
        let c = i.build(108, &held(10));
        assert_eq!(c.wbuttons & STANCE_BITS, msg::WBUTTON_CROUCH);
    }

    /// `togglecrouch` crouches with no 0x2, as `gocrouch` does, and a
    /// second press stands.
    #[test]
    fn togglecrouch_crouches_then_stands() {
        let mut i = synced(10);
        i.key(Action::ToggleCrouch, true);
        i.key(Action::ToggleCrouch, false);
        let c = i.build(100, &held(10));
        assert_eq!(c.wbuttons & STANCE_BITS, msg::WBUTTON_CROUCH);
        assert_eq!(c.up, -127);
        i.key(Action::ToggleCrouch, true);
        i.key(Action::ToggleCrouch, false);
        let c = i.build(108, &held(10));
        assert_eq!(c.wbuttons & STANCE_BITS, 0);
        assert_eq!(c.up, 0);
    }

    /// CoDMP.exe 0x40a670..0x40ad40.
    #[test]
    fn the_stance_binds_step_as_retail() {
        use Stance::{Crouch, Prone, Stand};
        use StancePress::{Jump, Nothing, Set};
        let cases = [
            (Action::ToggleCrouch, Prone, Set(Crouch)),
            (Action::ToggleProne, Stand, Set(Prone)),
            (Action::ToggleProne, Crouch, Set(Prone)),
            (Action::ToggleProne, Prone, Set(Stand)),
            (Action::LowerStance, Stand, Set(Crouch)),
            (Action::LowerStance, Crouch, Set(Prone)),
            (Action::LowerStance, Prone, Nothing),
            (Action::RaiseStance, Prone, Set(Crouch)),
            (Action::RaiseStance, Crouch, Set(Stand)),
            (Action::RaiseStance, Stand, Nothing),
            (Action::MoveUp, Prone, Set(Crouch)),
            (Action::MoveUp, Crouch, Set(Stand)),
            (Action::MoveUp, Stand, Jump),
            (Action::Jump, Prone, Set(Stand)),
            (Action::Jump, Stand, Jump),
        ];
        for (a, from, to) in cases {
            assert_eq!(stance_press(a, from, false), to, "{a:?} from {from:?}");
        }
        // Held keys stop every change; `+gostand` still jumps, `+moveup`
        // does not.
        assert_eq!(stance_press(Action::LowerStance, Stand, true), Nothing);
        assert_eq!(stance_press(Action::Jump, Crouch, true), Nothing);
        assert_eq!(stance_press(Action::Jump, Stand, true), Jump);
        assert_eq!(stance_press(Action::MoveUp, Stand, true), Nothing);
    }

    /// `+moveup` standing jumps until its own release.
    #[test]
    fn moveup_jumps_standing() {
        let mut i = synced(10);
        i.key(Action::MoveUp, true);
        assert_eq!(i.build(100, &held(10)).up, 127);
        i.key(Action::MoveUp, false);
        assert_eq!(i.build(108, &held(10)).up, 0);
    }

    /// Under a shellshock's caps a frame's turn is its summed motion held
    /// inside the cap, so a swing past it and back nets nothing.
    #[test]
    fn a_key_catcher_talks_unless_the_mouse_is_bypassed() {
        assert!(!talking(false, false, false, 0));
        assert!(talking(true, false, false, 0));
        assert!(talking(false, true, false, 0));
        assert!(talking(false, false, true, 0));
        // A quick message menu sets `cl_bypassMouseInput` 1.
        assert!(!talking(false, true, false, 1));
        assert!(!talking(true, false, true, 1));
    }

    #[test]
    fn talking_adds_the_talk_bit_to_the_held_buttons() {
        let mut i = PlayInput::default();
        i.key(Action::Attack, true);
        let held = held(10);
        assert_eq!(i.build(0, &held).buttons & msg::BUTTON_TALK, 0);
        i.talking = true;
        let c = i.build(8, &held);
        assert_eq!(c.buttons & msg::BUTTON_TALK, msg::BUTTON_TALK);
        // `CL_CmdButtons` ORs it over the keys; the server masks the rest.
        assert_ne!(c.buttons & msg::BUTTON_ATTACK, 0);
        i.talking = false;
        assert_eq!(i.build(16, &held).buttons & msg::BUTTON_TALK, 0);
    }

    #[test]
    fn shellshock_caps_a_frames_turn() {
        let mut i = PlayInput::default();
        i.set_mouse_caps(10.0, [0.0, 90.0]);
        i.mouse([0.0, 2.0]);
        let yaw = i.raw_angles[1];
        assert_eq!(yaw, (0.9 * SHORT_PER_DEG) as i32);
        // The frame's sum is back at 0: so is the turn.
        i.mouse([0.0, -2.0]);
        assert!(i.raw_angles[1].abs() <= 1, "{}", i.raw_angles[1]);
        i.mouse([5.0, 0.0]);
        assert_eq!(i.raw_angles[0], (5.0 * SHORT_PER_DEG) as i32);
        i.shellshock = true;
        let held = Held::default();
        assert_eq!(
            i.build(0, &held).wbuttons & msg::WBUTTON_SHELLSHOCK,
            msg::WBUTTON_SHELLSHOCK
        );
    }

    #[test]
    fn mouse_turns_raw_angles() {
        let mut i = PlayInput::default();
        i.mouse(MouseLook::default().degrees(100.0, 0.0, 1.0, false));
        let yaw = i.build(100, &held(10)).angles[1];
        assert_ne!(yaw, 0);
    }

    /// The kick rides the cmd on top of the mouse's angles and leaves the
    /// mouse's own untouched, so the view returns as the kick centres.
    #[test]
    fn the_view_kick_rides_the_cmd_angles() {
        let mut i = PlayInput::default();
        i.mouse([0.0, 45.0]);
        let base = i.build(100, &held(10)).angles;
        i.kick = [-1.0, 0.5, 0.0];
        let kicked = i.build(150, &held(10)).angles;
        assert_eq!(kicked[0] - base[0], -182, "one degree up, truncated");
        assert_eq!(kicked[1] - base[1], 91);
        i.kick = [0.0; 3];
        assert_eq!(i.build(200, &held(10)).angles, base);
    }

    #[test]
    fn slow_mouse_motion_accumulates() {
        // 0.01 counts is a third of a short unit, which truncated per event
        // to nothing.
        let mut i = PlayInput::default();
        let look = MouseLook::default();
        for _ in 0..1000 {
            i.mouse(look.degrees(0.01, -0.01, 1.0, false));
        }
        let per_1000 = 10.0 * 0.11 * SHORT_PER_DEG;
        let [pitch, yaw, _] = i.cmd_angles();
        assert!((yaw as f32 + per_1000).abs() <= 1.0, "yaw {yaw}");
        assert!((pitch as f32 + per_1000).abs() <= 1.0, "pitch {pitch}");
    }

    #[test]
    fn mouse_look_is_retail_cl_mousemove() {
        let look = MouseLook::default();
        let [pitch, yaw] = look.degrees(100.0, 10.0, 1.0, false);
        assert!(
            (yaw + 11.0).abs() < 1e-4,
            "100 counts right is 11 degrees, {yaw}"
        );
        assert!((pitch - 1.1).abs() < 1e-4, "{pitch}");
        let [_, zoomed] = look.degrees(100.0, 0.0, 0.5, false);
        assert!((zoomed + 5.5).abs() < 1e-4, "{zoomed}");
        let [pitch, yaw] = look.degrees(100.0, 10.0, 0.5, true);
        assert!(
            (yaw + 5.5).abs() < 1e-4,
            "mounted yaw is 2.5 x m_yaw, {yaw}"
        );
        assert!(
            (pitch - 0.44).abs() < 1e-4,
            "mounted pitch is 2 x m_pitch, {pitch}"
        );
    }

    /// `m_filter` 1 spends the mean of this frame's motion and the last
    /// frame's; 0 only this frame's.
    #[test]
    fn m_filter_averages_two_frames() {
        let mut m = MouseAccum::default();
        m.add(10.0, -4.0);
        assert_eq!(m.take(true), (5.0, -2.0));
        m.add(2.0, 0.0);
        assert_eq!(m.take(true), (6.0, -2.0));
        assert_eq!(m.take(true), (1.0, 0.0));
        m.add(3.0, 1.0);
        assert_eq!(m.take(false), (3.0, 1.0));
    }

    /// `cl_freelook` 0 drives `forwardmove` off the vertical motion until
    /// `+mlook`; `+strafe` sends the horizontal one to `rightmove`; both
    /// truncate and clamp to a byte over the keys' 127.
    #[test]
    fn freelook_off_moves_and_strafe_slides() {
        let look = MouseLook {
            freelook: false,
            ..MouseLook::default()
        };
        // 30 counts down at 5 x 0.25: 37.5, truncated, off forwardmove.
        let m = look.moves(0.0, 30.0, 1.0, false, false, false);
        assert_eq!((m.turn[0], m.forward), (0.0, -37));
        let m = look.moves(0.0, 30.0, 1.0, false, true, false);
        assert!((m.turn[0] - 3.3).abs() < 1e-4 && m.forward == 0);
        let m = MouseLook::default().moves(-7.0, 0.0, 1.0, false, false, true);
        assert_eq!((m.turn, m.side), ([0.0, 0.0], -8));

        let mut i = synced(10);
        i.key(Action::Back, true);
        i.mouse_move(look.moves(0.0, 30.0, 1.0, false, false, false));
        let c = i.build(100, &held(10));
        assert_eq!(c.forward, -128, "-127 - 37 clamps to the byte");
        assert_eq!(i.build(150, &held(10)).forward, -127, "spent once");
    }

    /// `-mlook` under `cl_freelook` 0 levels the view over `delta_angles`.
    #[test]
    fn center_view_levels_the_pitch() {
        let mut i = synced(10);
        i.mouse([20.0, 0.0]);
        i.center_view(-300);
        assert_eq!(i.build(100, &held(10)).angles[0], 300);
    }

    #[test]
    fn prone_sets_the_wire_bit_directly() {
        let mut i = synced(10);
        i.key(Action::Prone, true);
        let c = i.build(100, &held(10));
        assert_ne!(c.wbuttons & msg::WBUTTON_PRONE, 0);
        assert_eq!(c.up, -127);
    }

    #[test]
    fn cl_run_zero_aims_while_the_ads_key_is_up() {
        let mut i = PlayInput::default();
        assert_eq!(i.build(100, &held(10)).buttons & msg::BUTTON_ADS, 0);
        i.cl_run = ClRun(0);
        assert_ne!(i.build(108, &held(10)).buttons & msg::BUTTON_ADS, 0);
        i.key(Action::Ads, true);
        assert_eq!(i.build(116, &held(10)).buttons & msg::BUTTON_ADS, 0);
    }

    #[test]
    fn buttons_and_lean_and_reload_are_level() {
        let mut i = PlayInput::default();
        i.key(Action::Attack, true);
        i.key(Action::Ads, true);
        i.key(Action::Melee, true);
        i.key(Action::Use, true);
        i.key(Action::Reload, true);
        i.key(Action::LeanLeft, true);
        let c = i.build(100, &held(10));
        assert_eq!(
            c.buttons,
            msg::BUTTON_ATTACK | msg::BUTTON_ADS | msg::BUTTON_MELEE | msg::BUTTON_USE
        );
        assert_eq!(c.wbuttons, msg::WBUTTON_RELOAD | msg::WBUTTON_LEAN_LEFT);
        i.key(Action::Attack, false);
        i.key(Action::LeanLeft, false);
        i.key(Action::LeanRight, true);
        let c = i.build(108, &held(10));
        assert_eq!(
            c.buttons,
            msg::BUTTON_ADS | msg::BUTTON_MELEE | msg::BUTTON_USE
        );
        assert_eq!(c.wbuttons, msg::WBUTTON_RELOAD | msg::WBUTTON_LEAN_RIGHT);
    }

    #[test]
    fn opposite_axes_cancel() {
        let mut i = PlayInput::default();
        i.key(Action::Forward, true);
        i.key(Action::Back, true);
        i.key(Action::Left, true);
        let c = i.build(100, &held(10));
        assert_eq!(c.forward, 0);
        assert_eq!(c.right, -127);
    }

    #[test]
    fn next_and_prev_weapon_walk_nonzero_slots_wrapping() {
        // slot 1 holds 10 (in hand), slot 3 holds 3, slot 4 holds 6.
        let mut i = synced(10);
        i.key(Action::NextWeapon, true);
        assert_eq!(i.build(100, &held(10)).weapon, 3);
        let mut i = synced(10);
        i.key(Action::PrevWeapon, true);
        assert_eq!(i.build(100, &held(10)).weapon, 6);
    }

    #[test]
    fn a_server_forced_stance_change_is_adopted() {
        let mut i = synced(10);
        i.key(Action::Crouch, true);
        i.key(Action::Crouch, false);
        let crouched = Held {
            stance: Stance::Crouch,
            ..held(10)
        };
        let c = i.build(100, &crouched);
        assert_ne!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
        // The server stands the client up on its own (a respawn); no key
        // press asked for it, and the client must not fight it back.
        let stood = held(10);
        let c = i.build(108, &stood);
        assert_eq!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
    }

    /// 0x30028a70 writes `cl_stance 0`: a crouch asked for while dead does
    /// not carry into the next life, and `cg.mapRestart` takes the same
    /// spawn through the reset.
    #[test]
    fn a_respawn_or_a_restart_stands_and_reselects() {
        let mut i = synced(10);
        let dead = Held {
            spawn: Some((0, 1)),
            ..held(10)
        };
        i.build(100, &dead);
        i.key(Action::Crouch, true);
        i.key(Action::Crouch, false);
        assert_ne!(i.build(108, &dead).wbuttons & msg::WBUTTON_CROUCH, 0);
        let alive = Held {
            spawn: Some((0, 2)),
            ..held(10)
        };
        assert_eq!(i.build(116, &alive).wbuttons & msg::WBUTTON_CROUCH, 0);

        press(&mut i, Action::Slot(3));
        assert_eq!(i.build(124, &alive).weapon, 3);
        i.map_restart();
        assert_eq!(
            i.build(132, &alive).weapon,
            10,
            "same spawn after a restart"
        );
    }

    /// A refused prone: the server's stance never moved, so only the event
    /// takes the client off the key it keeps asking with.
    #[test]
    fn a_forced_stance_replaces_the_wanted_one() {
        let mut i = synced(10);
        i.key(Action::Prone, true);
        assert_ne!(i.build(100, &held(10)).wbuttons & msg::WBUTTON_PRONE, 0);
        i.force_stance(Stance::Stand);
        let c = i.build(108, &held(10));
        assert_eq!(c.wbuttons & (msg::WBUTTON_PRONE | msg::WBUTTON_CROUCH), 0);
    }

    #[test]
    fn held_unpacks_the_playerstate() {
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            ps.fields[PlayerState::field_index(p, name).unwrap()] = v;
        };
        set("weapon", 10);
        let [lo, hi] = vcod_common::pmove::weapon::pack_slots(&[0, 10, 0, 3, 6, 0, 0, 7]);
        set("weaponslots[0]", lo);
        set("weaponslots[4]", hi);
        set("eFlags", EF_PRONE);
        set("weapons[0]", 1 << 3 | 1 << 10);
        set("weapons[1]", 1);
        set("pm_flags", PMF_OWN_VIEW);
        set("clientNum", 2);
        ps.arrays.stats[5] = 7;
        // Weapon 3's clip index is 2, which holds a round; weapon 10 is dry
        // and names 3 as its alt.
        ps.arrays.ammoclip[2] = 1;
        let mut weapons = vec![None; 11];
        weapons[3] = Some(WeaponDef {
            clip_index: 2,
            ammo_index: 1,
            ..Default::default()
        });
        weapons[10] = Some(WeaponDef {
            clip_index: 4,
            ammo_index: 3,
            alt_index: 3,
            ..Default::default()
        });
        let h = Held::from_ps(&ps, p, &weapons);
        assert_eq!(h.weapon, 10);
        assert_eq!(h.slots, [0, 10, 0, 3, 6, 0, 0, 7]);
        assert_eq!(h.stance, Stance::Prone);
        assert_eq!(h.owned, 1 << 3 | 1 << 10 | 1 << 32);
        assert_eq!(h.loaded, 1 << 3);
        assert_eq!((h.alt_of(10), h.count()), (3, 10));
        assert!(h.own_view);
        assert_eq!(h.spawn, Some((2, 7)));
    }

    #[test]
    fn release_all_lets_go_of_keys_but_keeps_the_stance() {
        let mut i = synced(10);
        i.key(Action::Forward, true);
        i.key(Action::Attack, true);
        i.key(Action::Crouch, true);
        i.release_all();
        let c = i.build(100, &held(10));
        assert_eq!((c.forward, c.buttons), (0, 0));
        assert_ne!(c.wbuttons & msg::WBUTTON_CROUCH, 0);
    }

    #[test]
    fn stance_from_eflags_round_trips() {
        assert_eq!(Stance::from_eflags(0), Stance::Stand);
        assert_eq!(Stance::from_eflags(EF_CROUCH), Stance::Crouch);
        assert_eq!(Stance::from_eflags(EF_PRONE), Stance::Prone);
    }
}
