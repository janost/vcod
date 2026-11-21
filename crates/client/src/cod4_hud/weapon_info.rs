//! The owner draws: the weapon info and its clip graphic, held ammo, the
//! dpad slots and the talker list.

use vcod_cod4::hud as h;
use vcod_cod4::net::parse::Snapshot;

use super::assets::Loaded;
use super::env::{HudEnv, action_slot};
use super::text::text;
use super::text_height;
use super::{Cod4Hud, PM_SPECTATOR, image_of, quad};
use crate::console::shell::Shell;
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::{HudQuad, font};
use crate::play::cod4::Cod4Session;

impl Cod4Hud {
    /// The `dpad` menu (cod4-hud.md 15) as owner draws: each visible
    /// action slot's (166..169, `CG_DrawPlayerActionSlot`: the
    /// slot's weapon's `dpadIcon` sized by its ratio and its rounds as
    /// `%3i`, red at none, brighter while it is the selection; the night
    /// vision icon) and the `[key]` under it.
    pub(super) fn dpad(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        env: &HudEnv,
    ) -> Vec<OwnerDraw> {
        use vcod_cod4::menuexp;
        let mut out = Vec::new();
        let Some(layout) = l
            .script_menus
            .get(DPAD_MENU)
            .and_then(|m| m.layout.as_ref())
        else {
            return out;
        };
        let fade = env.game.fade_dpad;
        let Some(gs) = s.gamestate().filter(|_| fade != 0.0) else {
            return out;
        };
        let list = crate::play::cod4::weapon_list(gs);
        let slots = l.ammo_slots(&list);
        let ps = &snap.ps;
        for it in &layout.items {
            if it.dynamic_flags & vcod_cod4::scriptmenu::WINDOW_VISIBLE == 0
                || !(it.exps.visible.is_empty()
                    || menuexp::truth(&it.exps.visible, env).unwrap_or(false))
            {
                continue;
            }
            match it.owner_draw {
                od @ 166..=169 => {
                    let slot = action_slot(s, snap, (od - 166) as usize);
                    let mut rgba = it.fore_color;
                    if slot.active {
                        for c in &mut rgba[..3] {
                            *c *= 1.2;
                        }
                        rgba[3] = 1.0;
                    }
                    rgba[3] *= fade;
                    let r = it.rect;
                    if slot.night_vision {
                        out.push(OwnerDraw::Pic {
                            material: NVG_ICON.into(),
                            item: layout_item(it, r, rgba),
                        });
                        continue;
                    }
                    let w = usize::from(slot.weapon);
                    let Some(d) = w
                        .checked_sub(1)
                        .and_then(|i| l.weapon_hud.get(list.get(i)?))
                    else {
                        continue;
                    };
                    // The icon's ratio (cod4-hud.md 15.2): 4:1 twice as
                    // wide at half height, 2:1 twice as wide.
                    let [x, y, mut iw, mut ih] = r;
                    let mut iy = y;
                    match d.dpad_icon_ratio {
                        2 => {
                            iw *= 2.0;
                            iy += ih * 0.25;
                            ih *= 0.5;
                        }
                        1 => iw *= 2.0,
                        _ => {}
                    }
                    if let Some(m) = &d.dpad_icon {
                        out.push(OwnerDraw::Pic {
                            material: m.clone(),
                            item: layout_item(it, [x, iy, iw, ih], rgba),
                        });
                    }
                    // `BG_WeaponAmmo`: the clip and the reserve.
                    let (ammo, clip) = slots[w - 1];
                    let rounds = ps.ammoclip.get(clip).copied().unwrap_or(0)
                        + ps.ammo.get(ammo).copied().unwrap_or(0);
                    if rounds == 0 {
                        rgba[..3].copy_from_slice(&LOW_AMMO_RGB);
                    }
                    // The count's place (cod4-hud.md 15.2).
                    let mut tx = x + r[2] - 13.0;
                    if matches!(d.dpad_icon_ratio, 1 | 2) {
                        tx += r[2];
                    }
                    out.push(OwnerDraw::Text {
                        text: format!("{rounds:3}"),
                        x: tx,
                        item: layout_item(it, [tx, y + r[3] + 3.0, 0.0, 0.0], rgba),
                        rgba,
                    });
                }
                0 if it.kind == 0 => {
                    let t = menuexp::string(&it.exps.text, env).unwrap_or_default();
                    if t.is_empty() {
                        continue;
                    }
                    let mut rgba = it.fore_color;
                    if !it.exps.forecolor_a.is_empty() {
                        rgba[3] = menuexp::float(&it.exps.forecolor_a, env).unwrap_or(0.0);
                    }
                    // Measured on the 640x480 grid.
                    let grid = ScreenPlacement::new(640.0, 480.0);
                    let f = l.fonts.pick(it.font, it.text_scale, &grid);
                    let tw = font::measure(f, &t, it.text_scale / f.unit_scale());
                    let th = text_height(it.text_scale);
                    let [x, y] = crate::cod4_menu::text_origin(it, it.rect, tw, th);
                    out.push(OwnerDraw::Text {
                        text: t,
                        x,
                        item: layout_item(it, [x, y, 0.0, 0.0], rgba),
                        rgba,
                    });
                }
                _ => {}
            }
        }
        out
    }

    /// The `talkers` menu's four owner draws (`CG_DrawTalkerNum`,
    /// `iw3mp.exe` 0x443de4): talker `n` of [`Cod4Session::talkers`] the
    /// roster names, `voice_on` in the item's rect and the name 2 past it,
    /// centred on the rect's height. `cg_drawTalk` (`NONE`, `ALL`,
    /// `FRIENDLY`, `ENEMY`; `ALL` by default) picks whom. An enemy is a
    /// talker off our team while we are on one (0x443e43). An enemy
    /// heard through Eavesdrop (our `ps.perks` bit 9, not at the
    /// intermission) draws `perk_parabolicIcon` and the name in the enemy
    /// team's colour (0x443eaf).
    pub(super) fn talkers(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        shell: &Shell,
    ) -> Vec<OwnerDraw> {
        let mut out = Vec::new();
        let Some(layout) = l
            .script_menus
            .get(TALKERS_MENU)
            .and_then(|m| m.layout.as_ref())
        else {
            return out;
        };
        let draw_talk = shell.cvar("cg_drawTalk");
        let talkers = s.talkers();
        let own_team = s.own_team();
        let perks = snap.ps.word(vcod_cod4::ps_word!("perks"));
        let intermission = snap.ps.word(vcod_cod4::ps_word!("pm_type")) == h::PM_INTERMISSION;
        for it in &layout.items {
            let Some(n) = it
                .owner_draw
                .checked_sub(OD_TALKER)
                .filter(|n| (0..4).contains(n))
            else {
                continue;
            };
            let Some(&client) = talkers.get(n as usize) else {
                continue;
            };
            let Some(c) = snap.clients.iter().find(|c| c.0[0] == client) else {
                continue;
            };
            let name = crate::play::cod4::client_name(c);
            let team = crate::play::cod4::client_team(c);
            let enemy = own_team != 0 && own_team != team;
            if !draws_talker(draw_talk, enemy) {
                continue;
            }
            let mut text_rgba = it.fore_color;
            let material = if !intermission && enemy && perks >> 9 & 1 != 0 {
                let v = self
                    .server_dvar(s, "g_TeamColor_EnemyTeam")
                    .unwrap_or_default();
                let mut c = v
                    .split_whitespace()
                    .map(|x| x.parse::<f32>().unwrap_or(1.0));
                for t in &mut text_rgba[..3] {
                    *t = c.next().unwrap_or(1.0);
                }
                self.server_dvar(s, "perk_parabolicIcon")
                    .unwrap_or_else(|| PARABOLIC_ICON.into())
            } else {
                VOICE_ON.into()
            };
            let [x, y, w, hh] = it.rect;
            out.push(OwnerDraw::Pic {
                material,
                item: layout_item(it, it.rect, it.fore_color),
            });
            let th = text_height(it.text_scale);
            let (tx, ty) = (x + w + 2.0, th + y + (hh - th) / 2.0);
            out.push(OwnerDraw::Text {
                text: name,
                x: tx,
                item: layout_item(it, [tx, ty, 0.0, 0.0], text_rgba),
                rgba: text_rgba,
            });
        }
        out
    }

    /// A material's HUD texture once the zones load, to find its quads.
    #[cfg(test)]
    pub(crate) fn texture_of(&self, material: &str) -> Option<String> {
        image_of(self.loaded.as_ref()?.as_ref()?, material)
    }

    /// A weapon's HUD fields.
    #[cfg(test)]
    pub(crate) fn weapon_hud(&self, weapon: &str) -> Option<&h::WeaponHud> {
        self.loaded.as_ref()?.as_ref()?.weapon_hud.get(weapon)
    }

    /// A weapon's `dpadIcon` material.
    #[cfg(test)]
    pub(crate) fn dpad_icon(&self, weapon: &str) -> Option<String> {
        self.loaded
            .as_ref()?
            .as_ref()?
            .weapon_hud
            .get(weapon)?
            .dpad_icon
            .clone()
    }

    /// The held clip is low (`CG_CheckPlayerForLowClipSpecific`).
    pub(super) fn low_clip(&self, s: &Cod4Session) -> bool {
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return false;
        };
        held_ammo(l, s, snap).is_some_and(|w| w.low_clip())
    }
}

/// The clip graphic's pictures by `ammoCounterClip` 1..=5
/// (`CG_RegisterGraphics`).
const CLIP_MATERIALS: [&str; 5] = [
    "ammo_counter_bullet",
    "ammo_counter_riflebullet",
    "ammo_counter_shotgunshell",
    "ammo_counter_rocket",
    "ammo_counter_beltbullet",
];

/// Whose clip the clip graphic draws, with its slots: the held weapon's
/// for kinds 1..=5; for 6 its `altWeapon`'s, nothing when that is
/// missing or 6 too (`iw3mp.exe` 0x422a40, 0x422430; cod4-hud.md 9).
fn clip_source<'a, T>(
    held: (&'a h::WeaponHud, T),
    alt: impl FnOnce(&str) -> Option<(&'a h::WeaponHud, T)>,
) -> Option<(&'a h::WeaponHud, T)> {
    match held.0.ammo_counter_clip {
        1..=5 => Some(held),
        6 => alt(&held.0.alt_weapon).filter(|(a, _)| (1..=5).contains(&a.ammo_counter_clip)),
        _ => None,
    }
}

/// vcod's bound on the clip graphic's rounds, far above any stock clip: a
/// mod's weapon file names the count (cod4-hud.md 9).
const MAX_CLIP_ROUNDS: usize = 1000;

/// The clip graphic's rounds (cod4-hud.md 9), the first nearest the
/// item's corner `base`: each of a `size` clip of `kind` as a real-pixel
/// rect `[x, y, w, h]`. The sizes are physical pixels, not the 640x480
/// grid's. A clip past [`MAX_CLIP_ROUNDS`] draws that many.
fn clip_rounds(kind: i32, size: i32, base: [f32; 2]) -> Vec<[f32; 4]> {
    let [bx, by] = base;
    let count = (size.max(0) as usize).min(MAX_CLIP_ROUNDS);
    // Kinds 1..=4 run leftward, centred on the corner's height:
    // (width, height, pitch from one round's left to the next).
    let row = match kind {
        1 => Some((4.0, 8.0, 4.0)),
        2 => Some((32.0, 8.0, 40.0)),
        3 => Some((16.0, 8.0, 20.0)),
        4 => Some((64.0, 16.0, 72.0)),
        _ => None,
    };
    if let Some((w, h, pitch)) = row {
        let top = by - 0.5 * h;
        return (0..count)
            .map(|n| [bx - w - pitch * n as f32, top, w, h])
            .collect();
    }
    if kind != 5 {
        return Vec::new();
    }
    // The belt (`iw3mp.exe` 0x4228d0): 8 by 4 links in rows of 20, each
    // row 2 above the last, the even rows leftward from 8 left of the
    // corner and the odd ones back rightward from 160 left of it.
    let top = by + (count / 20) as f32 - 2.0;
    (0..count)
        .map(|i| {
            let (row, col) = (i / 20, (i % 20) as f32);
            let left = if row % 2 == 0 { col + 1.0 } else { 20.0 - col };
            [bx - 8.0 * left, top - 2.0 * row as f32, 8.0, 4.0]
        })
        .collect()
}

/// The action slots' menu and the night vision slot's icon
/// (`cgMedia.hudIconNVG`).
pub(super) const DPAD_MENU: &str = "dpad";
/// The low-health overlay's menu and its owner draw
/// (`CG_DrawPlayerLowHealthOverlay`, cod4-hud.md 19).
pub(super) const LOW_HEALTH_MENU: &str = "low health";
pub(super) const OD_LOW_HEALTH: i32 = 112;
pub(super) const NVG_ICON: &str = "hud_icon_nvg";
/// The talker list's menu and its four owner draws (`CG_TALKER`, 193..196;
/// `CG_DrawTalkerNum`, `iw3mp.exe` 0x443de4), and the voice icons.
pub(super) const TALKERS_MENU: &str = "talkers";
const OD_TALKER: i32 = 193;
pub(super) const VOICE_ON: &str = "voice_on";
pub(super) const VOICE_OFF: &str = "voice_off";
/// `perk_parabolicIcon`'s default (`iw3mp.exe` 0x6c2000).
const PARABOLIC_ICON: &str = "specialty_parabolic";

/// `colorLowAmmo` (1, 0.3, 0.3; `iw3mp.exe` rdata 0x6be598).
const LOW_AMMO_RGB: [f32; 3] = [1.0, 0.3, 0.3];

/// The low clip's flash (cod4-hud.md 15.5): the clip graphic's `fore`
/// colour eased toward [`LOW_AMMO_RGB`] by `sin(t / (40 pi)) / 2 + 1/2`,
/// `t` the ms since the clip last read not low.
fn low_clip_flash(fore: [f32; 4], since_ms: i32) -> [f32; 4] {
    let period = 40.0 * std::f32::consts::PI;
    let wave = (since_ms as f32 / period).sin();
    let toward = 0.5 + wave * 0.5;
    let mut rgba = fore;
    for (c, red) in rgba[..3].iter_mut().zip(LOW_AMMO_RGB) {
        *c += toward * (red - *c);
    }
    rgba
}
/// `CG_CheckPlayerForLowAmmoSpecific`: a reserve at or under a fifth of
/// the player's maximum reads low.
const LOW_RESERVE: f32 = 0.2;

/// The held weapon's ammo, as the low-ammo checks read it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeldAmmo {
    pub(super) clip: i32,
    clip_size: i32,
    clip_only: bool,
    threshold: f32,
    reserve: i32,
    /// `BG_GetAmmoPlayerMax`.
    pub(super) max: i32,
}

impl HeldAmmo {
    /// `CG_CheckPlayerForLowClipSpecific`: not a clip-only weapon, and a
    /// clip at or under `lowAmmoWarningThreshold` of its size.
    pub(super) fn low_clip(&self) -> bool {
        let clip = self.clip.min(999);
        let size = self.clip_size.min(999);
        !self.clip_only && clip >= 0 && size > 0 && clip as f32 <= self.threshold * size as f32
    }

    /// `CG_CheckPlayerForLowAmmoSpecific`: the reserve at or under a fifth
    /// of the player's maximum.
    fn low_reserve(&self) -> bool {
        self.max >= 0 && self.max.min(999) as f32 * LOW_RESERVE >= self.reserve.min(999) as f32
    }
}

/// The drawn playerstate's held weapon's ammo (`ps.weapon`), `None` for
/// no weapon, a spectator or a dead player.
fn held_ammo(l: &Loaded, s: &Cod4Session, snap: &Snapshot) -> Option<HeldAmmo> {
    let ps = &snap.ps;
    let u = |n: &str| ps.get(n).unwrap_or(0);
    if u("pm_type") >= PM_SPECTATOR {
        return None;
    }
    let list = crate::play::cod4::weapon_list(s.gamestate()?);
    let slots = l.ammo_slots(&list);
    let w = (u("weapon") as usize).checked_sub(1)?;
    let d = l.weapon_hud.get(list.get(w)?)?;
    let (ammo, clip) = slots[w];
    let clip_n = ps.ammoclip.get(clip).copied().unwrap_or(0);
    // `BG_GetAmmoPlayerMax`: a shared cap, a clip-only weapon's clip, else
    // the sum over every held weapon on this ammo slot.
    let max = if d.shared_ammo_cap > 0 {
        d.shared_ammo_cap
    } else if d.def.clip_only {
        d.def.clip_size
    } else {
        (0..list.len())
            .filter(|&k| u(&format!("weapons[{}]", (k + 1) / 32)) >> ((k + 1) % 32) & 1 != 0)
            .filter(|&k| slots[k].0 == ammo)
            .filter_map(|k| l.weapon_hud.get(&list[k]))
            .map(|d| d.def.max_ammo)
            .sum()
    };
    Some(HeldAmmo {
        clip: clip_n,
        clip_size: d.def.clip_size,
        clip_only: d.def.clip_only,
        threshold: d.low_ammo_threshold,
        reserve: if d.def.clip_only {
            clip_n
        } else {
            ps.ammo.get(ammo).copied().unwrap_or(0)
        },
        max,
    })
}

/// One `weaponinfo` owner draw, placed by its menu item.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum OwnerDraw {
    /// Text at `x` on the item's baseline.
    Text {
        text: String,
        x: f32,
        item: h::MenuItem,
        rgba: [f32; 4],
    },
    /// The clip graphic (117): the clip's rounds as a row of `kind`'s
    /// pictures leftward of the item's corner, those fired grey.
    Clip {
        kind: i32,
        size: i32,
        rounds: i32,
        item: h::MenuItem,
        /// The live rounds' colour, flashing on a low clip (cod4-hud.md
        /// 15.5).
        rgba: [f32; 4],
    },
    /// `CG_DrawPlayerWeaponName`: ending 28 left of the item's right edge.
    Name {
        text: String,
        item: h::MenuItem,
    },
    Pic {
        material: String,
        item: h::MenuItem,
    },
}

impl Cod4Hud {
    /// The `weaponinfo` menu's owner draws off the drawn
    /// playerstate: the stock (119), the clip (117, its rounds), the
    /// weapon's name (81), and the frag and second offhand's icons (103,
    /// 104) and counts (105, 106). Nothing for a spectator, a dead
    /// player, one on a turret or with the HUD's weapon off
    /// (`weapFlags` 0x80). cod4-hud.md 9.
    pub(super) fn weapon_info(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        now: i32,
    ) -> Vec<OwnerDraw> {
        let mut out = Vec::new();
        let ps = &snap.ps;
        let u = |n: &str| ps.get(n).unwrap_or(0);
        if u("pm_type") >= PM_SPECTATOR || u("weapFlags") & 0x80 != 0 || u("eFlags") & 0x20000 != 0
        {
            return out;
        }
        let Some(gs) = s.gamestate() else {
            return out;
        };
        let list = crate::play::cod4::weapon_list(gs);
        let slots = l.ammo_slots(&list);
        let def = |w: usize| {
            l.weapon_hud
                .get(list.get(w.checked_sub(1)?)?)
                .map(|d| (d, slots[w - 1]))
        };
        let held: Vec<usize> = (1..list.len() + 1)
            .filter(|&b| u(&format!("weapons[{}]", b / 32)) >> (b % 32) & 1 != 0)
            .collect();
        let clip = |slot: usize| ps.ammoclip.get(slot).copied().unwrap_or(0);
        let stock = |slot: usize| ps.ammo.get(slot).copied().unwrap_or(0);
        let item = |name: &str| l.weapon_info.get(name);
        let text = |out: &mut Vec<OwnerDraw>, it: &h::MenuItem, t: String, x: f32, rgba| {
            out.push(OwnerDraw::Text {
                text: t,
                x,
                item: it.clone(),
                rgba,
            })
        };
        let weapon = u("weapon") as usize;
        let low = held_ammo(l, s, snap);
        if let Some((d, (ammo, cl))) = def(weapon) {
            if let Some(it) = item("ammoStock") {
                // `CG_CheckPlayerForLowAmmoSpecific`: the reserve in red.
                let mut rgba = it.fore_color;
                if low.is_some_and(|w| w.low_reserve()) {
                    rgba[..3].copy_from_slice(&LOW_AMMO_RGB);
                }
                text(
                    &mut out,
                    it,
                    format!("{:3}", stock(ammo)),
                    it.rect.rect[0],
                    rgba,
                );
            }
            let by_name = |n: &str| {
                let i = list
                    .iter()
                    .position(|w| !n.is_empty() && w.eq_ignore_ascii_case(n))?;
                def(i + 1)
            };
            if let Some(it) = item("clipGraphic")
                && let Some((c, (_, ccl))) = clip_source((d, (ammo, cl)), by_name)
            {
                let rgba = if low.is_some_and(|w| w.low_clip()) {
                    low_clip_flash(it.fore_color, now - self.clip_flash_from)
                } else {
                    it.fore_color
                };
                out.push(OwnerDraw::Clip {
                    kind: c.ammo_counter_clip,
                    size: c.def.clip_size,
                    rounds: clip(ccl),
                    item: it.clone(),
                    rgba,
                });
            }
            if let Some(it) = item("weapname") {
                let mut name = self.message(&d.display_name);
                if !d.mode_name.is_empty() {
                    name = format!("{name} / {}", self.message(&d.mode_name));
                }
                out.push(OwnerDraw::Name {
                    text: name,
                    item: it.clone(),
                });
            }
        }
        let second = if u("offhandSecondary") == 1 { 3 } else { 2 };
        for (icon, count, class) in [
            ("offhandFragIcon", "offHandFragAmmo", 1),
            ("offhandSmokeIcon", "offHandSmokeAmmo", second),
        ] {
            let of_class: Vec<(usize, i32, i32)> = held
                .iter()
                .filter_map(|&w| {
                    let (d, (ammo, cl)) = def(w)?;
                    (d.def.offhand_class == class).then_some((w, clip(cl), stock(ammo)))
                })
                .collect();
            let Some(&(first, ..)) = of_class.first() else {
                continue;
            };
            // The equipped offhand when it is of the class, else the
            // class's first with rounds, else its first (cod4-hud.md 9).
            let equipped = u("offHandIndex") as usize;
            let throwback = (u("throwBackGrenadeTimeLeft") as i32) > 0;
            let best = if equipped != 0
                && def(equipped).is_some_and(|(d, _)| d.def.offhand_class == class)
            {
                equipped
            } else {
                of_class
                    .iter()
                    .find(|&&(_, c, a)| throwback || c + a > 0)
                    .map_or(first, |&(w, ..)| w)
            };
            if let (Some(it), Some(m)) =
                (item(icon), def(best).and_then(|(d, _)| d.hud_icon.clone()))
            {
                out.push(OwnerDraw::Pic {
                    material: m,
                    item: it.clone(),
                });
            }
            if let Some(it) = item(count) {
                let n: i32 = of_class.iter().map(|(_, n, _)| n).sum();
                let rgba = if n != 0 {
                    it.fore_color
                } else {
                    [0.89, 0.18, 0.01, it.fore_color[3]]
                };
                text(&mut out, it, n.to_string(), it.rect.rect[0], rgba);
            }
        }
        out
    }

    /// One owner draw at its item's rect and alignment.
    pub(super) fn draw_owner(
        &self,
        l: &Loaded,
        sp: &ScreenPlacement,
        d: &OwnerDraw,
        out: &mut Vec<HudQuad>,
    ) {
        let put_text =
            |out: &mut Vec<HudQuad>, it: &h::MenuItem, t: &str, x: f32, rgba: [f32; 4]| {
                let scale = it.text_scale;
                let f = l.fonts.pick(it.font, scale, sp);
                let px = scale / f.unit_scale();
                let h = text_height(scale);
                let (rx, _) = sp.axis(0, x, 0.0, it.rect.horz_align);
                let (ry, _) = sp.axis(1, it.rect.rect[1] - h, h, it.rect.vert_align);
                text(f, t, rx, ry, px * sp.virtual_to_real[1], rgba, out);
            };
        let width = |it: &h::MenuItem, t: &str| {
            let f = l.fonts.pick(it.font, it.text_scale, sp);
            font::measure(f, t, it.text_scale / f.unit_scale())
        };
        match d {
            OwnerDraw::Text {
                text: t,
                x,
                item,
                rgba,
            } => put_text(out, item, t, *x, *rgba),
            OwnerDraw::Clip {
                kind,
                size,
                rounds,
                item,
                rgba: live,
            } => {
                let r = &item.rect;
                let (bx, _) = sp.axis(0, r.rect[0], 0.0, r.horz_align);
                let (by, _) = sp.axis(1, r.rect[1], 0.0, r.vert_align);
                for (i, [x, y, w, h]) in clip_rounds(*kind, *size, [bx, by]).into_iter().enumerate()
                {
                    let mut rgba = *live;
                    if i as i32 >= *rounds {
                        rgba[..3].copy_from_slice(&[0.3; 3]);
                    }
                    if let Some(tex) = image_of(l, CLIP_MATERIALS[*kind as usize - 1]) {
                        out.push(quad(x, y, w, h, rgba, tex, false));
                    }
                }
            }
            OwnerDraw::Name { text: t, item } => {
                let x = item.rect.rect[2] + item.rect.rect[0] - width(item, t) - 28.0;
                put_text(out, item, t, x, item.fore_color);
            }
            OwnerDraw::Pic { material, item } => {
                if let Some(tex) = image_of(l, material) {
                    let [x, y, w, h] = item.rect.rect;
                    let (rx, rw) = sp.axis(0, x, w, item.rect.horz_align);
                    let (ry, rh) = sp.axis(1, y, h, item.rect.vert_align);
                    out.push(quad(rx, ry, rw, rh, item.fore_color, tex, false));
                }
            }
        }
    }
}

/// Whether `cg_drawTalk` (`NONE`, `ALL`, `FRIENDLY`, `ENEMY` or 0..3;
/// unset or unknown is `ALL`) draws a talker who is an `enemy` or not.
fn draws_talker(cg_draw_talk: Option<&str>, enemy: bool) -> bool {
    match cg_draw_talk.map(str::to_ascii_uppercase).as_deref() {
        Some("NONE") | Some("0") => false,
        Some("FRIENDLY") | Some("2") => !enemy,
        Some("ENEMY") | Some("3") => enemy,
        _ => true,
    }
}

/// A layout item as a HUD menu item at `rect`, coloured `rgba`.
fn layout_item(
    it: &vcod_cod4::scriptmenu::LayoutItem,
    rect: [f32; 4],
    rgba: [f32; 4],
) -> h::MenuItem {
    h::MenuItem {
        rect: h::MenuRect {
            rect,
            horz_align: it.align[0],
            vert_align: it.align[1],
            background: None,
        },
        owner_draw: it.owner_draw,
        font: it.font,
        text_scale: it.text_scale,
        text_style: it.text_style,
        fore_color: rgba,
        text_align: it.text_align,
        text: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::replay;

    /// `cg_drawTalk`'s four modes, by name or number, any case; unset or
    /// unknown draws everyone.
    #[test]
    fn draw_talk_picks_whom_the_talker_list_shows() {
        for (v, mate, enemy) in [
            (None, true, true),
            (Some("ALL"), true, true),
            (Some("1"), true, true),
            (Some("none"), false, false),
            (Some("0"), false, false),
            (Some("Friendly"), true, false),
            (Some("2"), true, false),
            (Some("ENEMY"), false, true),
            (Some("3"), false, true),
            (Some("7"), true, true),
        ] {
            assert_eq!(
                (draws_talker(v, false), draws_talker(v, true)),
                (mate, enemy),
                "{v:?}"
            );
        }
    }

    /// The kill capture's shooter empties his M16 to 9 of 30: from that
    /// snapshot on the clip graphic's rounds pulse toward red
    /// (`low_clip_flash`) until the reload fills it, and never before; the
    /// reserve turns red once the reload leaves 30 of 180 (a fifth or
    /// under), and only then. cod4-hud.md 15.
    #[test]
    fn kill_capture_pulses_the_shooters_low_clip() {
        if vcod_cod4::testing::cod4_install().is_none() {
            return;
        }
        let (mut low, mut pulsed, mut white_low, mut red_high) = (0, 0, 0, 0);
        let (mut stock_red, mut stock_wrong) = (0, 0);
        replay("play-1.7-kill-shooter.txt", |hud, s, now| {
            let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) else {
                return;
            };
            let held = held_ammo(l, s, snap);
            let is_low = held.is_some_and(|w| w.low_clip());
            let reserve_low = held.is_some_and(|w| w.low_reserve());
            for d in hud.weapon_info(l, s, snap, now) {
                match d {
                    OwnerDraw::Clip { rgba, item, .. } => {
                        let red = rgba[1] < item.fore_color[1] - 0.01;
                        match (is_low, red) {
                            (true, true) => pulsed += 1,
                            (true, false) => white_low += 1,
                            (false, true) => red_high += 1,
                            _ => {}
                        }
                        low += usize::from(is_low);
                    }
                    OwnerDraw::Text { item, rgba, .. } if item.owner_draw == 119 => {
                        let red = rgba != item.fore_color;
                        stock_red += usize::from(red);
                        stock_wrong += usize::from(red != reserve_low);
                    }
                    _ => {}
                }
            }
        });
        println!("low {low} pulsed {pulsed} white {white_low} red_high {red_high}");
        assert!(low > 150, "{low}");
        assert_eq!(red_high, 0);
        // The pulse is a sine: it reads red most of a low span, white at
        // its troughs.
        assert!(pulsed > low / 2 && white_low > 0, "{pulsed} {white_low}");
        assert!(stock_red > 0);
        assert_eq!(stock_wrong, 0);
    }

    /// The target's own weapon HUD while it holds the M16: 60 in stock,
    /// the clip's count, the rifle's name and both offhands, one each
    /// (cod4-hud.md 9).
    #[test]
    fn kill_capture_draws_the_targets_weapon_info() {
        let mut seen: Vec<OwnerDraw> = Vec::new();
        let (hud, _) = replay("play-1.7-kill-target.txt", |hud, s, _| {
            let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) else {
                return;
            };
            if snap.ps.word(vcod_cod4::ps_word!("weapon")) == 27
                && snap.ps.word(vcod_cod4::ps_word!("pm_type")) == 0
            {
                seen = hud.weapon_info(l, s, snap, 0);
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        println!("{seen:#?}");
        let texts: Vec<&str> = seen
            .iter()
            .filter_map(|d| match d {
                OwnerDraw::Text { text, .. } | OwnerDraw::Name { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts[0], " 60", "{texts:?}");
        assert!(texts[1].starts_with("M16"), "{texts:?}");
        assert_eq!(&texts[2..], ["1", "1"], "{texts:?}");
        // The clip as the magazine's 30 bullets, not a count.
        assert!(
            seen.iter().any(|d| matches!(d,
                OwnerDraw::Clip { kind: 1, size: 30, rounds, .. } if (1..=30).contains(rounds))),
            "{seen:?}"
        );
        let pics = seen
            .iter()
            .filter(|d| matches!(d, OwnerDraw::Pic { .. }))
            .count();
        assert_eq!(pics, 2, "{seen:?}");
        // Drawn at 1280x960 the bullets touch no other piece: the
        // number the clip once drew sat under the frag icon.
        let Some(Some(l)) = &hud.loaded else { return };
        let sp = ScreenPlacement::new(1280.0, 960.0);
        let boxes = |d: &OwnerDraw| {
            let mut q = Vec::new();
            hud.draw_owner(l, &sp, d, &mut q);
            q.iter()
                .map(|q| [q.verts[0], q.verts[2]])
                .collect::<Vec<_>>()
        };
        let (clip, rest): (Vec<_>, Vec<_>) = seen
            .iter()
            .partition(|d| matches!(d, OwnerDraw::Clip { .. }));
        let bullets = boxes(clip[0]);
        assert_eq!(bullets.len(), 30);
        for d in rest {
            for [a0, a1] in boxes(d) {
                for [b0, b1] in &bullets {
                    let apart =
                        a1[0] <= b0[0] || b1[0] <= a0[0] || a1[1] <= b0[1] || b1[1] <= a0[1];
                    assert!(apart, "{d:?} {a0:?}..{a1:?} meets a bullet {b0:?}..{b1:?}");
                }
            }
        }
    }

    /// The low clip's flash, fixed to the bit against the colours the
    /// owner draw computed before its rewrite: halfway at the start, the
    /// full red near 197 ms (20 pi squared) on.
    #[test]
    fn the_low_clip_flash_eases_toward_red() {
        let cases: [([f32; 4], i32, [f32; 4]); 8] = [
            ([1.0, 1.0, 1.0, 1.0], 0, [1.0, 0.65, 0.65, 1.0]),
            ([1.0, 1.0, 1.0, 1.0], 1, [1.0, 0.6472148, 0.6472148, 1.0]),
            ([1.0, 1.0, 1.0, 1.0], 188, [1.0, 0.3009771, 0.3009771, 1.0]),
            ([1.0, 1.0, 1.0, 1.0], 377, [1.0, 0.6006324, 0.6006324, 1.0]),
            ([1.0, 1.0, 1.0, 1.0], 4321, [1.0, 0.5900641, 0.5900641, 1.0]),
            (
                [0.8, 0.9, 0.7, 0.65],
                31,
                [0.9244196, 0.52674127, 0.45116085, 0.65],
            ),
            (
                [0.8, 0.9, 0.7, 0.65],
                250,
                [0.99136424, 0.3259073, 0.31727153, 0.65],
            ),
            (
                [0.8, 0.9, 0.7, 0.65],
                1000,
                [0.9994621, 0.30161363, 0.30107576, 0.65],
            ),
        ];
        for (fore, ms, want) in cases {
            assert_eq!(low_clip_flash(fore, ms), want, "{fore:?} at {ms} ms");
        }
    }

    /// Kind 6 draws its `altWeapon`'s clip with that weapon's slot; a
    /// missing alt or one set to 6 itself draws nothing, as 1.7 refuses
    /// both (cod4-hud.md 9).
    #[test]
    fn the_alt_weapons_clip_graphic_is_its_alts() {
        let hud = |kind, alt: &str| h::WeaponHud {
            ammo_counter_clip: kind,
            alt_weapon: alt.into(),
            ..Default::default()
        };
        let (gl, rifle, other_gl) = (
            hud(6, "m16_gl_mp"),
            hud(1, "gl_m16_mp"),
            hud(6, "gl_m16_mp"),
        );
        let lookup = |n: &str| match n {
            "m16_gl_mp" => Some((&rifle, 27)),
            "gl_m16_mp" => Some((&other_gl, 28)),
            _ => None,
        };
        let kind = |r: Option<(&h::WeaponHud, i32)>| r.map(|(d, slot)| (d.ammo_counter_clip, slot));
        assert_eq!(kind(clip_source((&gl, 28), lookup)), Some((1, 27)));
        assert_eq!(kind(clip_source((&rifle, 27), lookup)), Some((1, 27)));
        assert_eq!(kind(clip_source((&other_gl, 28), lookup)), None);
        let (no_alt, no_kind) = (hud(6, ""), hud(0, "m16_gl_mp"));
        assert_eq!(kind(clip_source((&no_alt, 3), lookup)), None);
        assert_eq!(kind(clip_source((&no_kind, 3), lookup)), None);
    }

    /// The magazine's bullets run left of the corner, 4 by 8 pixels each
    /// and touching; the belt stacks upward.

    #[test]
    fn the_clip_graphic_lays_out_its_rounds() {
        let m = clip_rounds(1, 3, [100.0, 50.0]);
        assert_eq!(
            m,
            [
                [96.0, 46.0, 4.0, 8.0],
                [92.0, 46.0, 4.0, 8.0],
                [88.0, 46.0, 4.0, 8.0]
            ]
        );
        let b = clip_rounds(5, 2, [100.0, 50.0]);
        assert_eq!(b, [[92.0, 48.0, 8.0, 4.0], [84.0, 48.0, 8.0, 4.0]]);
        // A row of 20 leftward, the next back rightward 2 higher, the
        // third leftward again; two rows put the first a unit lower.
        let b = clip_rounds(5, 42, [100.0, 50.0]);
        assert_eq!(b.len(), 42);
        for (i, at) in [
            (0, [92.0, 50.0]),
            (19, [-60.0, 50.0]),
            (20, [-60.0, 48.0]),
            (39, [92.0, 48.0]),
            (40, [92.0, 46.0]),
            (41, [84.0, 46.0]),
        ] {
            assert_eq!(b[i], [at[0], at[1], 8.0, 4.0], "round {i}");
        }
        assert!(clip_rounds(6, 1, [0.0, 0.0]).is_empty());
        assert!(clip_rounds(0, 4, [0.0, 0.0]).is_empty());
        assert!(clip_rounds(1, 0, [10.0, 10.0]).is_empty());
        assert!(clip_rounds(3, -2, [10.0, 10.0]).is_empty());
        for kind in [1, 5] {
            assert_eq!(clip_rounds(kind, i32::MAX, [0.0; 2]).len(), MAX_CLIP_ROUNDS);
        }
        let base = [612.5, 431.25];
        assert_eq!(
            clip_rounds(2, 2, base),
            [[580.5, 427.25, 32.0, 8.0], [540.5, 427.25, 32.0, 8.0]]
        );
        assert_eq!(
            clip_rounds(3, 3, base),
            [
                [596.5, 427.25, 16.0, 8.0],
                [576.5, 427.25, 16.0, 8.0],
                [556.5, 427.25, 16.0, 8.0]
            ]
        );
        assert_eq!(
            clip_rounds(4, 2, base),
            [[548.5, 423.25, 64.0, 16.0], [476.5, 423.25, 64.0, 16.0]]
        );
        assert_eq!(
            clip_rounds(5, 5, base),
            [
                [604.5, 429.25, 8.0, 4.0],
                [596.5, 429.25, 8.0, 4.0],
                [588.5, 429.25, 8.0, 4.0],
                [580.5, 429.25, 8.0, 4.0],
                [572.5, 429.25, 8.0, 4.0]
            ]
        );
    }
}
