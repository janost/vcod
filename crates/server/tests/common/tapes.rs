//! What the taped A/B replays share: where the tapes live and the weapon
//! lists beside them (docs/dev/testing.md, "Trace tapes").

use std::collections::BTreeSet;
use vcod_common::pk3::Pk3Fs;
use vcod_common::weapon::WeaponDef;

pub const TAPE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tapes");

/// The tape of capture `file` (`<capture>.tape`).
pub fn tape_path(file: &str) -> String {
    format!("{TAPE_DIR}/{}.tape", file.trim_end_matches(".txt"))
}

/// `RECORD_TAPES` is set: the taped tests record before they replay.
pub fn recording() -> bool {
    std::env::var_os("RECORD_TAPES").is_some()
}

/// A weapon file's `\key\value` pairs, first key wins in any case, as
/// `xmodel::parse_weapon` reads them.
fn weapon_pairs(text: &str) -> Vec<(String, String)> {
    let mut toks = text.trim_start_matches('\\').split('\\').peekable();
    if toks
        .peek()
        .is_some_and(|t| t.eq_ignore_ascii_case("WEAPONFILE"))
    {
        toks.next();
    }
    let mut out: Vec<(String, String)> = Vec::new();
    while let (Some(k), Some(v)) = (toks.next(), toks.next()) {
        if !out.iter().any(|(o, _)| o.eq_ignore_ascii_case(k)) {
            out.push((k.to_owned(), v.to_owned()));
        }
    }
    out
}

/// Writes the weapon list `names` (1-based, as configstring 7 orders it)
/// to `list` in the tape directory. A weapon in `carried` keeps its
/// numbers and the type names pmove reads; models, sounds, effects, icons
/// and display names stay out. The rest keep only the names
/// `weapon_table::assign_indices` numbers the ammo slots by.
pub fn write_weapons(fs: &Pk3Fs, names: &str, carried: &BTreeSet<usize>, list: &str) {
    const NAMES: [&str; 3] = ["ammoName", "clipName", "sharedAmmoCapName"];
    const TYPES: [&str; 3] = ["weaponType", "weaponClass", "altWeapon"];
    let mut out =
        String::from("# configstring 7's weapons, the numbers pmove reads off each carried one\n");
    for (i, name) in names.split(' ').enumerate() {
        let keep = carried.contains(&(i + 1));
        let keys = match fs.read(&format!("weapons/mp/{name}")) {
            Some(bytes) => {
                let mut pairs: Vec<(String, String)> =
                    weapon_pairs(&String::from_utf8_lossy(&bytes))
                        .into_iter()
                        .filter(|(k, v)| {
                            let named = |l: &[&str]| l.iter().any(|n| n.eq_ignore_ascii_case(k));
                            named(&NAMES)
                                || keep && (named(&TYPES) || v.trim().parse::<f64>().is_ok())
                        })
                        .collect();
                pairs.sort();
                pairs.iter().map(|(k, v)| format!("\\{k}\\{v}")).collect()
            }
            None => "-".to_owned(),
        };
        out.push_str(&format!("w {name} {keys}\n"));
    }
    std::fs::write(format!("{TAPE_DIR}/{list}"), out).unwrap();
}

/// The weapon table off the list [`write_weapons`] wrote, as
/// `weapon_table::from_configstring` builds it from the paks.
pub fn read_weapons(list: &str) -> Vec<Option<WeaponDef>> {
    let text = std::fs::read_to_string(format!("{TAPE_DIR}/{list}")).unwrap();
    let mut names = Vec::new();
    let mut defs: Vec<Option<WeaponDef>> = vec![None];
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let (name, keys) = line
            .strip_prefix("w ")
            .and_then(|l| l.split_once(' '))
            .expect("a `w <name> <keys>` line");
        names.push(name.to_owned());
        defs.push(
            (keys != "-").then(|| WeaponDef::from_map(&weapon_pairs(keys).into_iter().collect())),
        );
    }
    vcod_common::weapon_table::assign_indices(&mut defs);
    vcod_common::weapon_table::link_alts(&mut defs, names.iter().map(String::as_str));
    defs
}

/// The lines of a tape's `# ` header that start with one of `prefixes`:
/// the report a replay must give back.
pub fn recorded<'a>(tape: &'a str, prefixes: &[&str]) -> Vec<&'a str> {
    tape.lines()
        .filter_map(|l| l.strip_prefix("# "))
        .filter(|l| prefixes.iter().any(|p| l.starts_with(p)))
        .collect()
}
