//! CoD4 1.7's weapon numbers as the server keeps them: each registered
//! weapon's def fields the loadout reads, read out of the zone's 0x878-byte
//! `WeaponDef`, and a client's weapons, ammo and clips as the playerstate
//! carries them (docs/research/cod4-gsc.md 7).
//!
//! Every offset below is its field's row in `cod4_lnxded`'s
//! `weaponDefFields` table (0x826a160, 502 rows of name, offset and type;
//! `tools/re/cod4_weapon_fields.py` prints it): `weaponClass` +304,
//! `inventoryType` +316, `offhandClass` +324, `startAmmo` +800, `ammoName`
//! +804, `clipName` +812, `maxAmmo` +820, `clipSize` +824,
//! `sharedAmmoCapName` +832, `sharedAmmoCap` +840, `moveSpeedScale` +1048,
//! `adsOverlayShader` +1072, `clipOnly` +1280, `killIcon` +1304,
//! `altWeapon` +1340, `dropAmmoMin` +1348, `dropAmmoMax` +1352. VERIFIED
//! (the table). The walk capture's ammo and clips read right through them
//! (`cod4::wire_ab`), VERIFIED live.

use vcod_cod4::assets::weapon::Weapon;

/// The `weaponClass` names in enum order, what `weaponClass()` returns:
/// `cod4_lnxded`'s ten-pointer table at 0x826b900. VERIFIED (the table).
pub const WEAP_CLASSES: [&str; 10] = [
    "rifle",
    "mg",
    "smg",
    "spread",
    "pistol",
    "grenade",
    "rocketlauncher",
    "turret",
    "non-player",
    "item",
];
pub const WEAPCLASS_TURRET: i32 = 7;
pub const WEAPCLASS_NON_PLAYER: i32 = 8;
pub const WEAPCLASS_ITEM: i32 = 9;

/// The def fields the server reads.
#[derive(Clone, Debug, Default)]
pub struct WeaponDef4 {
    pub name: String,
    /// `weaponClass` (+304), `inventoryType` (+316), `offhandClass` (+324:
    /// 1 frag, 2 smoke, 3 flash).
    pub weap_class: i32,
    pub inventory_type: i32,
    pub offhand_class: i32,
    /// `startAmmo` (+800), `maxAmmo` (+820), `clipSize` (+824).
    pub start_ammo: i32,
    pub max_ammo: i32,
    pub clip_size: i32,
    /// `sharedAmmoCap` (+840) when `sharedAmmoCapName` names one.
    pub shared_ammo_cap: Option<i32>,
    /// `dropAmmoMin` (+1348) and `dropAmmoMax` (+1352): a placed or
    /// dropped pickup's reserve draw (`G_SpawnItem`, cod4-combat.md 10.9).
    pub drop_ammo_min: i32,
    pub drop_ammo_max: i32,
    /// `moveSpeedScale` (+1048).
    pub move_speed_scale: f32,
    /// `clipOnly` (+1280).
    pub clip_only: bool,
    /// The scales pmove reads (`moveSpeedScale` +1048 and the two after).
    pub mv: vcod_cod4::pmove::WeaponMove,
    pub ammo_name: String,
    pub clip_name: String,
    pub alt_name: Option<String>,
    /// The playerstate's ammo and clip slots, given at registration
    /// ([`Cod4Weapons::register`]).
    pub ammo_slot: usize,
    pub clip_slot: usize,
    /// The weapon number of `alt_name`, once it registered.
    pub alt: usize,
}

impl WeaponDef4 {
    pub fn of(w: &Weapon) -> WeaponDef4 {
        let int = |at: usize| {
            w.raw
                .get(at..at + 4)
                .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
        };
        let s = |k: &str| w.strings.get(k).cloned().unwrap_or_default();
        let shared = s("sharedAmmoCapName");
        WeaponDef4 {
            name: w.name.to_ascii_lowercase(),
            weap_class: int(304),
            inventory_type: int(316),
            offhand_class: int(324),
            start_ammo: int(800),
            max_ammo: int(820),
            clip_size: int(824),
            shared_ammo_cap: (!shared.is_empty()).then(|| int(840)),
            drop_ammo_min: int(1348),
            drop_ammo_max: int(1352),
            move_speed_scale: f32::from_bits(int(1048) as u32),
            clip_only: int(1280) != 0,
            mv: vcod_cod4::pmove::WeaponMove::from_raw(&w.raw).unwrap_or_default(),
            ammo_name: s("ammoName"),
            clip_name: s("clipName"),
            alt_name: Some(s("altWeapon").to_ascii_lowercase()).filter(|a| !a.is_empty()),
            ammo_slot: 0,
            clip_slot: 0,
            alt: 0,
        }
    }
}

/// The level's weapons by number, 0 the empty hand's, and the ammo and clip
/// names that took each slot (`BG_SetupAmmoIndexes`: a weapon
/// takes the slot of the first earlier weapon with its name, slot 0 weapon
/// 0's; the walk capture's `beretta_mp`, weapon 11, reads clip 11 and ammo
/// 9, VERIFIED live, cod4-hud.md 9).
#[derive(Default)]
pub struct Cod4Weapons {
    pub defs: Vec<WeaponDef4>,
    ammo_names: Vec<String>,
    clip_names: Vec<String>,
}

impl Cod4Weapons {
    pub fn new() -> Cod4Weapons {
        // Weapon 0's empty names hold slot 0.
        Cod4Weapons {
            defs: vec![WeaponDef4::default()],
            ammo_names: vec![String::new()],
            clip_names: vec![String::new()],
        }
    }

    /// The next weapon number for `def`, its slots assigned.
    pub fn register(&mut self, mut def: WeaponDef4) -> usize {
        let slot = |names: &mut Vec<String>, n: &str| match names.iter().position(|x| x == n) {
            Some(i) => i,
            None => {
                names.push(n.to_string());
                names.len() - 1
            }
        };
        def.ammo_slot = slot(&mut self.ammo_names, &def.ammo_name);
        def.clip_slot = slot(&mut self.clip_names, &def.clip_name);
        self.defs.push(def);
        let n = self.defs.len() - 1;
        // An alt registered after its parent: link both ways once both are in.
        let name = self.defs[n].name.clone();
        for i in 1..n {
            if self.defs[i].alt_name.as_deref() == Some(&name) {
                self.defs[i].alt = n;
            }
        }
        if let Some(alt) = self.defs[n].alt_name.clone()
            && let Some(i) = self.index(&alt)
        {
            self.defs[n].alt = i;
        }
        n
    }

    /// A registered weapon's number by name, case-blind.
    pub fn index(&self, name: &str) -> Option<usize> {
        (1..self.defs.len()).find(|&i| self.defs[i].name.eq_ignore_ascii_case(name))
    }

    pub fn get(&self, i: usize) -> Option<&WeaponDef4> {
        self.defs.get(i).filter(|_| i > 0)
    }
}

/// `ACTIONSLOTTYPE_*`: the walk capture's spawn reads 3 on slot 1
/// (`nightvision`) and 2 on slot 3 (`altMode`). VERIFIED live.
pub const ACTIONSLOT_NOTHING: u8 = 0;
pub const ACTIONSLOT_WEAPON: u8 = 1;
pub const ACTIONSLOT_ALTWEAPON: u8 = 2;
pub const ACTIONSLOT_NIGHTVISION: u8 = 3;

/// One client's weapons as its playerstate carries them.
#[derive(Clone, Debug)]
pub struct Loadout {
    /// `ps.weapons` and `ps.weaponold`, one bit a weapon number.
    pub weapons: [u32; 4],
    pub weaponold: [u32; 4],
    pub ammo: [i32; 128],
    pub ammoclip: [i32; 128],
    /// `ps.weapon`, `ps.offHandIndex`, `ps.offhandSecondary` (0 smoke, 1
    /// flash).
    pub weapon: usize,
    pub offhand: usize,
    pub offhand_secondary: u32,
    pub action_slot_type: [u8; 4],
    pub action_slot_param: [u32; 4],
    /// `setMoveSpeedScale`: `ps.moveSpeedScaleMultiplier`.
    pub move_speed_scale: f32,
}

impl Default for Loadout {
    fn default() -> Self {
        Loadout {
            weapons: [0; 4],
            weaponold: [0; 4],
            ammo: [0; 128],
            ammoclip: [0; 128],
            weapon: 0,
            offhand: 0,
            offhand_secondary: 0,
            action_slot_type: [0; 4],
            action_slot_param: [0; 4],
            move_speed_scale: 1.0,
        }
    }
}

fn bit(bits: &[u32; 4], i: usize) -> bool {
    i < 128 && bits[i / 32] >> (i % 32) & 1 != 0
}

fn set_bit(bits: &mut [u32; 4], i: usize, on: bool) {
    if i < 128 {
        if on {
            bits[i / 32] |= 1 << (i % 32);
        } else {
            bits[i / 32] &= !(1 << (i % 32));
        }
    }
}

/// The alts `w` chains to, in order, ending at none, back at `w` or at a
/// weapon already listed.
fn alts(t: &Cod4Weapons, w: usize) -> impl Iterator<Item = usize> + '_ {
    let mut seen = vec![w];
    std::iter::successors(t.defs.get(w).map(|d| d.alt), move |&a| {
        t.defs.get(a).map(|d| d.alt)
    })
    .take_while(move |&a| {
        let fresh = a != 0 && !seen.contains(&a);
        seen.push(a);
        fresh
    })
}

impl Loadout {
    pub fn has(&self, w: usize) -> bool {
        bit(&self.weapons, w)
    }

    /// The weapons held whose reserve is ammo slot `slot`.
    fn held_on<'a>(
        &'a self,
        t: &'a Cod4Weapons,
        slot: usize,
    ) -> impl Iterator<Item = &'a WeaponDef4> + 'a {
        (1..t.defs.len())
            .filter(move |&i| self.has(i) && t.defs[i].ammo_slot == slot)
            .map(move |i| &t.defs[i])
    }

    /// `BG_GetAmmoPlayerMax`, the most reserve `w` may carry: its shared
    /// cap where its file names one, its clip for a clip-only weapon, and
    /// otherwise what every held weapon on its ammo slot allows (each one's
    /// shared cap or `maxAmmo`) added up. 0 for no weapon.
    pub fn ammo_max(&self, t: &Cod4Weapons, w: usize) -> i32 {
        let Some(d) = t.get(w) else { return 0 };
        let own = d.shared_ammo_cap.or(d.clip_only.then_some(d.clip_size));
        own.unwrap_or_else(|| {
            self.held_on(t, d.ammo_slot)
                .map(|o| o.shared_ammo_cap.unwrap_or(o.max_ammo))
                .sum()
        })
    }

    /// `Fill_Clip`: the clip topped up out of the ammo slot.
    pub fn fill_clip(&mut self, t: &Cod4Weapons, w: usize) {
        let Some(d) = t.get(w) else { return };
        let moved = (d.clip_size - self.ammoclip[d.clip_slot]).min(self.ammo[d.ammo_slot]);
        self.ammo[d.ammo_slot] -= moved;
        self.ammoclip[d.clip_slot] += moved;
    }

    /// `G_GivePlayerWeapon`. `None` when refused: no such weapon, one
    /// already held, a turret's or a non-player one. Otherwise the weapon
    /// is held and not "old"; an item stops there, an offhand becomes the
    /// equipped one when none is (then `Some(Some(w))`: the `C <index>` the
    /// client is told), and anything else brings the alts it chains to
    /// that are not held yet.
    pub fn give(&mut self, t: &Cod4Weapons, w: usize) -> Option<Option<usize>> {
        let d = t.get(w)?;
        let refused = matches!(d.weap_class, WEAPCLASS_TURRET | WEAPCLASS_NON_PLAYER);
        if refused || self.has(w) {
            return None;
        }
        set_bit(&mut self.weapons, w, true);
        set_bit(&mut self.weaponold, w, false);
        if d.weap_class == WEAPCLASS_ITEM {
            return Some(None);
        }
        if d.offhand_class != 0 {
            let first = self.offhand == 0;
            if first {
                self.offhand = w;
            }
            return Some(first.then_some(w));
        }
        let new: Vec<usize> = alts(t, w).take_while(|&a| !self.has(a)).collect();
        for a in new {
            set_bit(&mut self.weapons, a, true);
        }
        Some(None)
    }

    /// `G_InitializeAmmo` for `w` and each held alt it chains to: a weapon
    /// is topped up to its `startAmmo`, counting as already there what its
    /// ammo slot holds less what the other held weapons on that slot are
    /// still owed of their own start (start less clip). The top-up fills
    /// the clip unless the client `had` the weapon; with nothing owed a
    /// weapon new to him still gets its clip filled.
    pub fn initialize_ammo(&mut self, t: &Cod4Weapons, w: usize, had: bool) {
        let chain: Vec<usize> = std::iter::once(w)
            .chain(alts(t, w).take_while(|&a| self.has(a)))
            .collect();
        for cur in chain {
            let Some(d) = t.get(cur) else { break };
            let others_owed: i32 = (0..t.defs.len())
                .filter(|&i| i != cur && self.has(i) && t.defs[i].ammo_slot == d.ammo_slot)
                .map(|i| t.defs[i].start_ammo - self.ammoclip[t.defs[i].clip_slot])
                .sum();
            let there = (self.ammo[d.ammo_slot] - others_owed).max(0);
            match d.start_ammo - there {
                owed if owed > 0 => self.add_ammo(t, cur, owed, !had),
                _ if !had => self.fill_clip(t, cur),
                _ => {}
            }
        }
    }

    /// `Add_Ammo`'s count into the reserve: `count` added to `w`'s ammo
    /// slot, the clip filled from it when asked (always for a clip-only
    /// weapon, whose reserve is then emptied), the reserve held to
    /// [`Self::ammo_max`] and the clip to its size.
    pub fn add_ammo(&mut self, t: &Cod4Weapons, w: usize, count: i32, fill: bool) {
        let Some(d) = t.get(w) else { return };
        self.ammo[d.ammo_slot] += count;
        if d.clip_only {
            // Everything goes in the clip; nothing stays in reserve.
            self.fill_clip(t, w);
            self.ammo[d.ammo_slot] = 0;
        } else {
            if fill {
                self.fill_clip(t, w);
            }
            let cap = self.ammo_max(t, w);
            self.ammo[d.ammo_slot] = self.ammo[d.ammo_slot].min(cap);
        }
        let clip = &mut self.ammoclip[d.clip_slot];
        *clip = (*clip).min(d.clip_size);
    }

    /// `takeWeapon`: the bit, and the current or offhand weapon
    /// when it was this one.
    pub fn take(&mut self, w: usize) {
        set_bit(&mut self.weapons, w, false);
        if self.weapon == w {
            self.weapon = 0;
        }
        if self.offhand == w {
            self.offhand = 0;
        }
    }

    pub fn set_old(&mut self, w: usize) {
        set_bit(&mut self.weaponold, w, true);
    }

    /// Every held weapon's number, lowest first.
    pub fn held(&self) -> impl Iterator<Item = usize> + '_ {
        (1..128).filter(|&i| self.has(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str, ammo: &str, start: i32, max: i32, clip: i32) -> WeaponDef4 {
        WeaponDef4 {
            name: name.into(),
            ammo_name: ammo.into(),
            clip_name: name.into(),
            start_ammo: start,
            max_ammo: max,
            clip_size: clip,
            ..Default::default()
        }
    }

    /// m16 with a launcher alt, an m4 on the m16's ammo, a frag (clip
    /// only, offhand 1), a smoke (offhand 2), a shared-cap pistol, a
    /// turret, a non-player weapon and an item.
    fn table() -> Cod4Weapons {
        let mut t = Cod4Weapons::new();
        t.register(WeaponDef4 {
            alt_name: Some("gl_m16".into()),
            ..def("m16", "556", 90, 120, 30)
        });
        t.register(def("gl_m16", "gl", 1, 1, 1));
        t.register(def("m4", "556", 60, 90, 30));
        t.register(WeaponDef4 {
            clip_only: true,
            offhand_class: 1,
            ..def("frag", "frag", 1, 1, 1)
        });
        t.register(WeaponDef4 {
            offhand_class: 2,
            ..def("smoke", "smoke", 1, 1, 1)
        });
        t.register(WeaponDef4 {
            shared_ammo_cap: Some(45),
            ..def("colt", "45", 30, 60, 7)
        });
        t.register(WeaponDef4 {
            weap_class: WEAPCLASS_TURRET,
            ..def("saw_mp", "saw", 0, 0, 0)
        });
        t.register(WeaponDef4 {
            weap_class: WEAPCLASS_NON_PLAYER,
            ..def("heli", "heli", 0, 0, 0)
        });
        t.register(WeaponDef4 {
            weap_class: WEAPCLASS_ITEM,
            ..def("radar", "radar", 0, 0, 0)
        });
        t
    }

    fn state(l: &Loadout, t: &Cod4Weapons) -> Vec<(usize, i32, i32)> {
        l.held()
            .map(|w| {
                let d = &t.defs[w];
                (w, l.ammo[d.ammo_slot], l.ammoclip[d.clip_slot])
            })
            .collect()
    }

    #[test]
    fn give_brings_the_alt_and_equips_the_first_offhand() {
        let t = table();
        let mut l = Loadout::default();
        l.set_old(1);
        assert_eq!(l.give(&t, 1), Some(None));
        assert_eq!(l.held().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(l.weaponold, [0; 4]);
        assert_eq!(l.give(&t, 1), None);
        assert_eq!(l.give(&t, 7), None);
        assert_eq!(l.give(&t, 8), None);
        assert_eq!(l.give(&t, 0), None);
        assert_eq!(l.give(&t, 9), Some(None));
        assert_eq!(l.give(&t, 4), Some(Some(4)));
        assert_eq!(l.give(&t, 5), Some(None));
        assert_eq!(l.offhand, 4);
        assert_eq!(l.held().collect::<Vec<_>>(), [1, 2, 4, 5, 9]);
    }

    #[test]
    fn ammo_max_reads_the_cap_the_clip_or_the_held_sum() {
        let t = table();
        let mut l = Loadout::default();
        assert_eq!(l.ammo_max(&t, 1), 0);
        assert_eq!(l.ammo_max(&t, 4), 1);
        assert_eq!(l.ammo_max(&t, 6), 45);
        assert_eq!(l.ammo_max(&t, 0), 0);
        l.give(&t, 1);
        assert_eq!(l.ammo_max(&t, 1), 120);
        l.give(&t, 3);
        assert_eq!(l.ammo_max(&t, 1), 210);
        assert_eq!(l.ammo_max(&t, 3), 210);
    }

    #[test]
    fn initialize_ammo_fills_the_chain_less_what_the_slot_holds() {
        let t = table();
        let mut l = Loadout::default();
        l.give(&t, 1);
        l.initialize_ammo(&t, 1, false);
        let one = state(&l, &t);
        l.give(&t, 3);
        l.initialize_ammo(&t, 3, false);
        let two = state(&l, &t);
        l.ammo[t.defs[1].ammo_slot] = 5;
        l.ammoclip[t.defs[3].clip_slot] = 2;
        l.initialize_ammo(&t, 3, true);
        let again = state(&l, &t);
        l.give(&t, 4);
        l.initialize_ammo(&t, 4, false);
        l.give(&t, 6);
        l.initialize_ammo(&t, 6, false);
        let rest = state(&l, &t);
        assert_eq!(one, [(1, 60, 30), (2, 0, 1)]);
        assert_eq!(two, [(1, 90, 30), (2, 0, 1), (3, 90, 30)]);
        assert_eq!(again, [(1, 65, 30), (2, 0, 1), (3, 65, 2)]);
        assert_eq!(
            rest,
            [(1, 65, 30), (2, 0, 1), (3, 65, 2), (4, 0, 1), (6, 23, 7)]
        );
    }

    #[test]
    fn add_ammo_caps_and_fills_on_request() {
        let t = table();
        let mut l = Loadout::default();
        l.give(&t, 1);
        l.add_ammo(&t, 1, 500, false);
        let capped = state(&l, &t);
        l.add_ammo(&t, 1, 10, true);
        let filled = state(&l, &t);
        l.ammoclip[t.defs[1].clip_slot] = 99;
        l.add_ammo(&t, 1, 0, false);
        let clip = state(&l, &t);
        l.give(&t, 4);
        l.add_ammo(&t, 4, 3, false);
        let frag = state(&l, &t);
        l.add_ammo(&t, 0, 3, true);
        assert_eq!(capped, [(1, 120, 0), (2, 0, 0)]);
        assert_eq!(filled, [(1, 100, 30), (2, 0, 0)]);
        assert_eq!(clip, [(1, 100, 30), (2, 0, 0)]);
        assert_eq!(frag, [(1, 100, 30), (2, 0, 0), (4, 0, 1)]);
    }
}
