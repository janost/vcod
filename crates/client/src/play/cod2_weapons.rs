//! A CoD2 session's weapon files: `CS_WEAPONS`' list parsed once
//! (`vcod_cod2::weapon`), with what each consumer derives from a file. The
//! prediction, the effects, the sounds, the view and the recoil all read
//! this one table.

use vcod_cod2::net::ConfigStrings;
use vcod_cod2::pmove::weapon::WeaponParams;
use vcod_cod2::weapon::WeaponDef;
use vcod_common::asset_source::AssetSource;

use crate::play::cod2_fx::WeaponFx;
use crate::play::cod2_gun::GunKeys;
use crate::play::cod2_recoil::kick_def;
use crate::play::cod2_view::SightDef;
use crate::play::recoil::KickDef;

/// One CS 7 weapon's file and the consumers' views of it.
pub struct LevelWeapon {
    pub def: WeaponDef,
    pub(crate) fx: WeaponFx,
    pub kick: KickDef,
    pub sight: (SightDef, GunKeys),
}

impl LevelWeapon {
    fn of(def: WeaponDef) -> LevelWeapon {
        LevelWeapon {
            fx: WeaponFx::of(&def),
            kick: kick_def(&def),
            sight: (SightDef::of(&def), GunKeys::of(&def)),
            def,
        }
    }
}

/// The level's weapons, weapon `n` being `CS_WEAPONS`' `n`th name, read
/// off one CS 7 text over one search path. [`Self::ensure`] rebuilds it
/// when CS 7 changes; the session [`Self::clear`]s it at a gamestate and
/// when the search path opens again.
#[derive(Default)]
pub struct LevelWeapons {
    /// The CS 7 text the table was built off; `None` before a build.
    cs7: Option<String>,
    /// By weapon index; 0 and a file that did not load are `None`.
    weapons: Vec<Option<LevelWeapon>>,
    /// The prediction's `PM_Weapon` table (`pmove::weapon::table_of`).
    params: Vec<Option<WeaponParams>>,
}

impl LevelWeapons {
    /// Forget every file: the next [`Self::ensure`] reads them again.
    pub fn clear(&mut self) {
        *self = LevelWeapons::default();
    }

    /// Reads `cs`'s weapon files unless the table holds them already.
    pub fn ensure(&mut self, fs: &dyn AssetSource, cs: &ConfigStrings) {
        let cs7 = cs.get(vcod_cod2::net::protocol::CS_WEAPONS);
        if self.cs7.as_deref() == Some(cs7) {
            return;
        }
        let names: Vec<&str> = cs7.split(' ').collect();
        let defs: Vec<Option<WeaponDef>> = vcod_cod2::weapon::load_all(fs, &names)
            .into_iter()
            .zip(&names)
            .map(|(p, name)| match p {
                Ok(p) => Some(p.def),
                Err(e) => {
                    if !name.is_empty() {
                        log::warn!("cod2 weapon: {e:#}");
                    }
                    None
                }
            })
            .collect();
        let refs: Vec<Option<&WeaponDef>> = defs.iter().map(Option::as_ref).collect();
        self.params = vcod_cod2::pmove::weapon::table_of(&names, &refs);
        self.weapons = std::iter::once(None)
            .chain(defs.into_iter().map(|d| d.map(LevelWeapon::of)))
            .collect();
        self.cs7 = Some(cs7.to_string());
    }

    /// Weapon `index`'s file; `None` for 0, an index past the list or a
    /// file that did not load.
    pub fn get(&self, index: i32) -> Option<&LevelWeapon> {
        self.weapons.get(usize::try_from(index).ok()?)?.as_ref()
    }

    /// The prediction's weapon table, index 0 first.
    pub fn params(&self) -> &Vec<Option<WeaponParams>> {
        &self.params
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use vcod_common::pk3::Pk3Fs;

    /// Configstrings holding `CS_WEAPONS` as `names`.
    pub(crate) fn cs_weapons(names: &str) -> ConfigStrings {
        let gs = vcod_cod2::net::Gamestate2 {
            server_command_sequence: 0,
            configstrings: Default::default(),
            baselines: Default::default(),
            client_num: 0,
            checksum_feed: 0,
        };
        let mut cs = ConfigStrings::from_gamestate(&gs);
        let slot = vcod_cod2::net::protocol::CS_WEAPONS;
        cs.apply_command(format!("d {slot} {names}").as_bytes());
        cs
    }

    /// A level whose weapon `n` is `files[n - 1]`: a name and its file's
    /// `\key\value` text past the `WEAPONFILE` tag.
    pub(crate) fn level(files: &[(&str, &str)]) -> LevelWeapons {
        let mut fs = Pk3Fs::empty();
        for (name, keys) in files {
            let text = format!("WEAPONFILE{keys}").into_bytes();
            fs.overlay(&format!("weapons/mp/{name}"), text);
        }
        let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
        let mut w = LevelWeapons::default();
        w.ensure(&fs, &cs_weapons(&names.join(" ")));
        w
    }

    /// Weapon `n` is CS 7's `n`th name; 0, a gap and a missing file read
    /// as none, and a new CS 7 text rebuilds the table.
    #[test]
    fn the_table_follows_cs_7() {
        let mut fs = Pk3Fs::empty();
        fs.overlay(
            "weapons/mp/a_mp",
            b"WEAPONFILE\\moveSpeedScale\\0.5".to_vec(),
        );
        fs.overlay(
            "weapons/mp/b_mp",
            b"WEAPONFILE\\moveSpeedScale\\0.25".to_vec(),
        );
        let mut w = LevelWeapons::default();
        w.ensure(&fs, &cs_weapons("a_mp  gone_mp b_mp"));
        assert!(w.get(0).is_none());
        assert_eq!(w.get(1).unwrap().def.float("moveSpeedScale"), 0.5);
        assert!(w.get(2).is_none(), "the gap");
        assert!(w.get(3).is_none(), "the missing file");
        assert_eq!(w.get(4).unwrap().def.float("moveSpeedScale"), 0.25);
        assert!(w.get(5).is_none());
        assert_eq!(w.params().len(), 5);
        w.ensure(&fs, &cs_weapons("b_mp"));
        assert_eq!(w.get(1).unwrap().def.float("moveSpeedScale"), 0.25);
        assert!(w.get(4).is_none());
    }

    /// Every key the sounds read, off every stock weapon file, is what
    /// CoD 1's `\key\value` reader the sounds used before gave.
    #[test]
    fn stock_sound_keys_read_as_cod1s_reader_read_them() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        const KEYS: [&str; 28] = [
            "pickupSound",
            "ammoPickupSound",
            "projectileSound",
            "pullbackSound",
            "fireSound",
            "fireSoundPlayer",
            "lastShotSound",
            "lastShotSoundPlayer",
            "meleeSwipeSound",
            "rechamberSound",
            "rechamberSoundPlayer",
            "reloadSound",
            "reloadSoundPlayer",
            "reloadEmptySound",
            "reloadEmptySoundPlayer",
            "reloadStartSound",
            "reloadStartSoundPlayer",
            "reloadEndSound",
            "reloadEndSoundPlayer",
            "raiseSound",
            "putawaySound",
            "altSwitchSound",
            "projExplosionSound",
            "noteTrackSoundA",
            "noteTrackSoundB",
            "noteTrackSoundC",
            "noteTrackSoundD",
            "clipOnly",
        ];
        let names: Vec<String> = fs
            .list_prefix("weapons/mp/")
            .into_iter()
            .filter_map(|p| {
                p.rsplit('/')
                    .next()
                    .filter(|n| !n.is_empty())
                    .map(str::to_string)
            })
            .collect();
        assert!(names.len() >= 40, "{names:?}");
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut w = LevelWeapons::default();
        w.ensure(&fs, &cs_weapons(&refs.join(" ")));
        let mut named = 0;
        for (i, name) in names.iter().enumerate() {
            let text = fs.read(&format!("weapons/mp/{name}")).unwrap();
            let old = vcod_common::xmodel::parse_weapon(&String::from_utf8_lossy(&text));
            let def = &w.get(i as i32 + 1).unwrap_or_else(|| panic!("{name}")).def;
            for key in KEYS {
                let was = old.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
                if key == "clipOnly" {
                    assert_eq!(def.flag(key), was.is_some_and(|v| v != "0"), "{name}");
                    continue;
                }
                let now = Some(def.str(key).trim()).filter(|v| !v.is_empty());
                assert_eq!(now, was, "{name}: {key}");
                named += usize::from(now.is_some());
            }
        }
        assert!(named > 200, "{named}");
    }
}
