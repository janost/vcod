//! CoD2 1.3's sound aliases: the 24-column csvs, `volumemodgroups.def` and
//! the `.vfcurve` falloff curves, loaded into the [`AliasTable`] CoD 1's
//! audio plays from (docs/research/cod2-sound.md). `.spkrmap` files are
//! read by nothing here: retail's MP loader ignores the `speakermap`
//! column (section 1).

use std::sync::Arc;
use vcod_common::asset_source::AssetSource;

use vcod_common::text::atoi;

use crate::audio::alias::{AliasRow, AliasTable, Channel, MAX_PROBABILITY, MasterSlave, split_csv};

/// `Com_LoadSoundAliasFile`'s blank `dist_min` (Mac 0xb969a).
pub const DEFAULT_DIST_MIN: f32 = 120.0;
/// A blank or 0 `dist_max` is `dist_min` times this (`FLOAT_0032628c`).
pub const DIST_MAX_FACTOR: f32 = 5.0;
/// `probability` is a float weight; the table keeps it as an integer
/// weight this many times larger, so the pick's ratios hold.
pub const PROBABILITY_SCALE: f32 = 1000.0;
/// Most knots a curve holds (`Com_LoadVolumeFalloffCurve`).
const MAX_KNOTS: usize = 8;
/// Most volume mod groups (`Com_LoadSoundAliasFile`).
const MAX_GROUPS: usize = 32;
/// Longest alias, secondary alias or file name, 0x3f less its terminator.
const MAX_NAME: usize = 0x3f;

/// A `.vfcurve`: knots `(x, y)`, x the fraction of the way from
/// `dist_min` to `dist_max`, y the gain. The first is `(0, 1)` and the
/// last `(1, 0)`.
#[derive(Clone, Debug, PartialEq)]
pub struct VfCurve {
    pub name: String,
    pub knots: Vec<[f32; 2]>,
}

impl VfCurve {
    /// `Com_InitDefaultSoundAliasVolumeFalloffCurve` (Mac 0xb7658): the
    /// unnamed straight line every row without a curve takes.
    pub fn linear() -> VfCurve {
        VfCurve {
            name: String::new(),
            knots: vec![[0.0, 1.0], [1.0, 0.0]],
        }
    }

    /// `Com_LoadVolumeFalloffCurve` (Mac 0xb718a): `SNDCURVE`, the knot
    /// count, then that many `x y` pairs, each in 0..1. Endpoints other
    /// than `(0, 1)` and `(1, 0)` are put there, as retail warns and does.
    pub fn parse(name: &str, text: &str) -> Result<VfCurve, String> {
        let body = text
            .strip_prefix("SNDCURVE")
            .ok_or_else(|| format!("{name}: does not appear to be a sndcurve file"))?;
        let mut toks = tokens(body).into_iter();
        let count = toks.next().map_or(0, |t| atoi(&t));
        if count < 2 {
            return Err(format!("{name}: knot count ({count}) is less than 2"));
        }
        if count as usize > MAX_KNOTS {
            return Err(format!(
                "{name}: knot count ({count}) is greater than maxKnots"
            ));
        }
        let mut knots = Vec::new();
        while let Some(x) = toks.next().filter(|t| t != "}") {
            if knots.len() == MAX_KNOTS {
                return Err(format!("{name}: knots parsed is greater than maxKnots"));
            }
            let x = atof(&x);
            if !(0.0..=1.0).contains(&x) {
                return Err(format!(
                    "{name}: knot x-coord '{x}' is not in the range 0-1"
                ));
            }
            let Some(y) = toks.next().filter(|t| t != "}") else {
                break;
            };
            let y = atof(&y);
            if !(0.0..=1.0).contains(&y) {
                return Err(format!(
                    "{name}: knot x-coord '{y}' is not in the range 0-1"
                ));
            }
            knots.push([x, y]);
        }
        if knots.len() != count as usize {
            return Err(format!(
                "{name}: knot count ({count}) does not match knots parsed ({})",
                knots.len()
            ));
        }
        let last = knots.len() - 1;
        if knots[0] != [0.0, 1.0] || knots[last] != [1.0, 0.0] {
            log::warn!("sndcurve {name}: endpoints are not (0, 1) and (1, 0); adjusting them");
            knots[0] = [0.0, 1.0];
            knots[last] = [1.0, 0.0];
        }
        Ok(VfCurve {
            name: name.to_string(),
            knots,
        })
    }

    /// `GraphGetValueFromFraction` (Mac 0x29976): the gain at `frac`,
    /// linear between the knots around it; -1 past the last knot's x.
    pub fn value(&self, frac: f32) -> f32 {
        let k = &self.knots;
        if k.len() < 2 {
            return -1.0;
        }
        let mut i = 1;
        while k[i][0] < frac {
            i += 1;
            if i == k.len() {
                return -1.0;
            }
        }
        let (a, b) = (k[i - 1], k[i]);
        (frac - a[0]) / (b[0] - a[0]) * (b[1] - a[1]) + a[1]
    }

    /// `SND_Attenuate` (Mac 0x49c2e): 1 inside `dist_min`, 0 from
    /// `dist_max` on, the curve between.
    pub fn attenuate(&self, dist: f32, dist_min: f32, dist_max: f32) -> f32 {
        // A NaN wire position; keep it out of the gain.
        if !dist.is_finite() {
            return 0.0;
        }
        let d = dist - dist_min;
        if d <= 0.0 {
            return 1.0;
        }
        if dist_min == dist_max {
            return 0.0;
        }
        let frac = d / (dist_max - dist_min);
        if frac >= 1.0 {
            return 0.0;
        }
        self.value(frac)
    }
}

/// `Com_Parse` outside csv mode, as the curve and group files are read:
/// whitespace-split tokens, `//` comments to the line's end skipped.
fn tokens(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .flat_map(str::split_whitespace)
        .map(String::from)
        .collect()
}

/// C's `atof` over the forms the stock files use: the longest leading
/// number, 0 without one.
fn atof(s: &str) -> f32 {
    let s = s.trim_start();
    (1..=s.len())
        .rev()
        .filter(|&n| s.is_char_boundary(n))
        .find_map(|n| s[..n].parse::<f32>().ok())
        .unwrap_or(0.0)
}

/// `soundaliases/volumemodgroups.def`: `VOLUMEMODGROUPS`, then name and
/// value pairs (`Com_LoadSoundAliasFile`'s first call, Mac 0xb969a).
pub fn parse_volume_mod_groups(text: &str) -> Result<Vec<(String, f32)>, String> {
    let body = text
        .strip_prefix("VOLUMEMODGROUPS")
        .ok_or("does not appear to be a volumemodgroups file")?;
    let mut out = Vec::new();
    let mut toks = tokens(body).into_iter();
    while let Some(name) = toks.next().filter(|t| t != "}") {
        if out.len() == MAX_GROUPS {
            return Err(format!(
                "groups parsed is greater than or equal to {MAX_GROUPS}"
            ));
        }
        let Some(v) = toks.next().filter(|t| t != "}") else {
            return Err(format!("groupname '{name}' missing a matching value"));
        };
        out.push((name, atof(&v)));
    }
    Ok(out)
}

/// The columns `Com_LoadSoundAliasField` (Mac 0xb78fa) knows, in
/// `g_pszSndAliasKeyNames` order (Mac `.data` 0x348780); index 0 is a
/// column it ignores (`compression`).
const KEYS: [&str; 24] = [
    "",
    "name",
    "sequence",
    "file",
    "subtitle",
    "vol_min",
    "vol_max",
    "vol_mod",
    "pitch_min",
    "pitch_max",
    "dist_min",
    "dist_max",
    "channel",
    "type",
    "loop",
    "probability",
    "loadspec",
    "masterslave",
    "secondaryaliasname",
    "volumefalloffcurve",
    "startdelay",
    "speakermap",
    "reverb",
    "lfe percentage",
];

/// An alias or secondary alias name the loader accepts: under 63 bytes,
/// letters, digits and `_`.
fn valid_name(s: &str) -> bool {
    s.len() < MAX_NAME && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// What the loader reads besides the csvs.
pub struct Ctx<'a> {
    pub groups: &'a [(String, f32)],
    pub curves: &'a [Arc<VfCurve>],
}

/// One csv, `Com_LoadSoundAliasFile` (Mac 0xb969a), every row kept
/// whatever its `loadspec` ([`for_map`] applies it). A row with a bad
/// field is dropped and its error listed; a row missing its name or file
/// ends the file, as retail's does.
pub fn parse_csv(text: &str, source: &str, ctx: &Ctx) -> AliasTable {
    let mut table = AliasTable {
        cod2: true,
        ..AliasTable::default()
    };
    let mut cols: Option<Vec<usize>> = None;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let cells = split_csv(line);
        let first = cells[0].trim();
        if first.is_empty() || first.starts_with('#') {
            continue;
        }
        let Some(cols) = &cols else {
            let keys: Vec<usize> = cells
                .iter()
                .map(|c| {
                    KEYS.iter()
                        .position(|k| !k.is_empty() && k.eq_ignore_ascii_case(c.trim()))
                        .unwrap_or(0)
                })
                .collect();
            if !keys.contains(&1) || !keys.contains(&3) {
                table
                    .errors
                    .push(format!("{source}: missing 'name' and/or 'file' columns"));
                return table;
            }
            cols = Some(keys);
            continue;
        };
        match parse_row(&cells, cols, source, ctx) {
            Row::Ok(row) => table.rows.entry(row.name.clone()).or_default().push(row),
            Row::Bad(errs) => table.errors.extend(errs),
            Row::EndOfFile => {
                table
                    .errors
                    .push(format!("{source}: alias entry missing name and/or file"));
                break;
            }
        }
    }
    table
}

enum Row {
    Ok(AliasRow),
    Bad(Vec<String>),
    EndOfFile,
}

/// One row: every non-blank cell under a known column through
/// `Com_LoadSoundAliasField`, then the row checks and the `vol_mod` scale.
fn parse_row(cells: &[String], cols: &[usize], source: &str, ctx: &Ctx) -> Row {
    let mut errs = Vec::new();
    let mut seen = [false; KEYS.len()];
    let mut name = String::new();
    let mut file = String::new();
    let mut sequence = 0i64;
    let (mut vol, mut vol_mod, mut pitch) = ((1.0f32, 1.0f32), 1.0f32, (1.0f32, 1.0f32));
    let (mut dist_min, mut dist_max) = (DEFAULT_DIST_MIN, 0.0f32);
    let mut channel = Channel::Auto;
    let mut streamed = false;
    let mut looping = false;
    let mut probability = 1.0f32;
    let mut loadspec = String::new();
    let mut master_slave = MasterSlave::None;
    let mut secondary = None;
    let mut curve: Option<Arc<VfCurve>> = None;
    let mut start_delay_ms = 0u32;
    let mut subtitle = None;
    let mut no_wet = false;
    for (i, &key) in cols.iter().enumerate() {
        let cell = cells.get(i).map_or("", |c| c.trim());
        if key == 0 || cell.is_empty() {
            continue;
        }
        if std::mem::replace(&mut seen[key], true) {
            errs.push(format!(
                "{source}: Duplicate entries for the '{}' column",
                KEYS[key]
            ));
            continue;
        }
        match KEYS[key] {
            "name" if cell.len() >= MAX_NAME => errs.push(format!(
                "{source}: Alias name '{cell}' is longer than 63 characters"
            )),
            "name" if !valid_name(cell) => {
                errs.push(format!("{source}: Alias name '{cell}' is invalid"))
            }
            "name" => name = cell.to_ascii_lowercase(),
            "sequence" => sequence = i64::from(atoi(cell)),
            "file" if cell.len() >= MAX_NAME => errs.push(format!(
                "{source}: Sound file '{cell}' is longer than 63 characters"
            )),
            "file" => file = cell.to_string(),
            "subtitle" if !cell.is_ascii() => errs.push(format!(
                "{source}: Subtitle '{cell}' has an invalid character"
            )),
            "subtitle" => subtitle = Some(cell.to_string()),
            "vol_min" | "vol_max" => {
                let v = atof(cell);
                if !(0.0..=1.0).contains(&v) {
                    errs.push(format!("{source}: {} '{v}' is not within 0-1", KEYS[key]));
                } else if KEYS[key] == "vol_min" {
                    vol.0 = v;
                    if !seen[6] {
                        vol.1 = v;
                    }
                } else {
                    vol.1 = v;
                }
            }
            "vol_mod" => match ctx
                .groups
                .iter()
                .find(|(g, _)| g.eq_ignore_ascii_case(cell))
            {
                Some(&(_, v)) => vol_mod = v,
                // Retail's `Com_Error(1)`: the load stops.
                None => errs.push(format!("{source}: Volume Mod Group '{cell}' not found")),
            },
            "pitch_min" => {
                pitch.0 = atof(cell);
                if !seen[9] {
                    pitch.1 = pitch.0;
                }
            }
            "pitch_max" => pitch.1 = atof(cell),
            "dist_min" => dist_min = atof(cell),
            "dist_max" => dist_max = atof(cell),
            "channel" => match Channel::parse(cell) {
                Some(c) => channel = c,
                None => errs.push(format!("{source}: Unknown sound channel '{cell}'")),
            },
            "type" => match cell.to_ascii_lowercase().as_str() {
                "streamed" | "primed" => streamed = true,
                "loaded" => streamed = false,
                _ => errs.push(format!("{source}: Unknown sound type '{cell}'")),
            },
            "loop" => match cell.to_ascii_lowercase().as_str() {
                "looping" => looping = true,
                "nonlooping" => looping = false,
                _ => errs.push(format!("{source}: Unknown sound looping type '{cell}'")),
            },
            "probability" => probability = atof(cell),
            "loadspec" => loadspec = cell.to_ascii_lowercase(),
            "masterslave" if cell.eq_ignore_ascii_case("master") => {
                master_slave = MasterSlave::Master
            }
            "masterslave" => {
                let v = atof(cell);
                if !(0.0..=1.0).contains(&v) {
                    errs.push(format!("{source}: SlavePercentage '{v}' is not within 0-1"));
                }
                master_slave = MasterSlave::Slave(v);
            }
            "secondaryaliasname" if !valid_name(cell) => errs.push(format!(
                "{source}: Secondary Alias name '{cell}' is invalid"
            )),
            "secondaryaliasname" => secondary = Some(cell.to_ascii_lowercase()),
            "volumefalloffcurve" => {
                match ctx
                    .curves
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(cell))
                {
                    Some(c) => curve = Some(c.clone()),
                    // Retail's `Com_Error(1)`: the load stops.
                    None => errs.push(format!("{source}: Volume Falloff Curve '{cell}' not found")),
                }
            }
            "startdelay" => start_delay_ms = atoi(cell).max(0) as u32,
            // Flag 0x10; `fulldrylevel` (0x8) is read only by the blend's
            // validation (cod2-sound.md 7.12).
            "reverb" => no_wet = cell.contains("nowetlevel"),
            // `speakermap` has no arm in the MP loader; `lfe percentage`
            // is stored and read by no mixer path (7.12).
            _ => {}
        }
    }
    if name.is_empty() || file.is_empty() {
        if seen[1] && seen[3] {
            // Both cells were there but failed their checks.
            return Row::Bad(errs);
        }
        return Row::EndOfFile;
    }
    if !errs.is_empty() {
        return Row::Bad(errs);
    }
    if pitch.1 < pitch.0 {
        pitch = (pitch.1, pitch.0);
    }
    if pitch.0 <= 0.0 {
        return Row::Bad(vec![format!(
            "sound alias '{name}' has pitch_min {} <= 0",
            pitch.0
        )]);
    }
    if vol.1 < vol.0 {
        vol = (vol.1, vol.0);
    }
    if dist_max == 0.0 {
        dist_max = dist_min * DIST_MAX_FACTOR;
    }
    if dist_max < dist_min {
        return Row::Bad(vec![format!(
            "sound alias '{name}' has dist_min {dist_min} <= dist_max {dist_max}"
        )]);
    }
    if dist_min <= 0.0 {
        return Row::Bad(vec![format!(
            "sound alias '{name}' has dist_min {dist_min} <= 0"
        )]);
    }
    if vol_mod != 1.0 {
        vol = (
            (vol.0 * vol_mod).clamp(0.0, 1.0),
            (vol.1 * vol_mod).clamp(0.0, 1.0),
        );
    }
    let curve = curve.filter(|c| !c.name.is_empty());
    Row::Ok(AliasRow {
        name,
        file,
        sequence,
        vol,
        pitch,
        dist: (dist_min, dist_max),
        channel,
        streamed,
        looping,
        probability: (probability * PROBABILITY_SCALE)
            .round()
            .clamp(0.0, MAX_PROBABILITY as f32) as u32,
        master_slave,
        lod: (-1.0, -1.0),
        loadspec,
        curve,
        secondary,
        start_delay_ms,
        csv: source.to_string(),
        subtitle,
        no_wet,
    })
}

/// Whether a `loadspec` cell admits `map` on the client's load (`all_mp`,
/// `CL_LoadSoundAliases`, Mac 0x17ae88), `Com_LoadSoundAliasField`'s
/// case 16. A `!` cell starts out excluding and any other including; the
/// map named as a whole word keeps that, otherwise it flips, unless the
/// rest of the cell is `all_mp`.
pub fn loadspec_admits(cell: &str, map: &str) -> bool {
    if cell.is_empty() {
        return map != "menu";
    }
    // An empty name would match at every byte and walk off the cell.
    let map = if map.is_empty() { "menu" } else { map };
    let b = cell.as_bytes();
    let mut admit = !cell.starts_with('!') || map == "menu";
    let mut pos = 0;
    while let Some(off) = cell[pos..].find(map) {
        let i = pos + off;
        let end = i + map.len();
        if (i == 0 || b[i - 1] < b'"') && (end == b.len() || b[end] < b'!') {
            return admit;
        }
        pos = i + 1;
    }
    if map == "menu" || &cell[pos..] != "all_mp" {
        admit = !admit;
    }
    admit
}

/// The rows `map` loads: [`loadspec_admits`], then
/// `Com_SortTempSoundAliases_r` (Mac 0xb6924) by name and sequence. Two
/// rows of one name and sequence from one csv are both dropped
/// (`duplicate alias`); from two csvs, the csv whose name sorts later
/// wins.
pub fn for_map(all: &AliasTable, map: &str) -> AliasTable {
    let mut out = AliasTable {
        cod2: true,
        skipped: all.skipped,
        errors: all.errors.clone(),
        ..AliasTable::default()
    };
    for (name, rows) in &all.rows {
        let mut kept: Vec<AliasRow> = rows
            .iter()
            .filter(|r| loadspec_admits(&r.loadspec, map))
            .cloned()
            .collect();
        kept.sort_by_key(|r| r.sequence);
        let mut group: Vec<AliasRow> = Vec::with_capacity(kept.len());
        for r in kept {
            match group.last() {
                Some(l) if l.sequence == r.sequence => {
                    let (lc, rc) = (l.csv.to_ascii_lowercase(), r.csv.to_ascii_lowercase());
                    if lc == rc {
                        out.errors
                            .push(format!("{}: duplicate alias '{name}'", r.csv));
                        group.pop();
                    } else if rc > lc {
                        group.pop();
                        group.push(r);
                    }
                }
                _ => group.push(r),
            }
        }
        if !group.is_empty() {
            out.rows.insert(name.clone(), group);
        }
    }
    out
}

/// Every `soundaliases/*.vfcurve`, named by file stem, after the unnamed
/// default (`Com_LoadSoundAliases`, Mac 0xacc20).
pub fn load_curves(fs: &dyn AssetSource, errors: &mut Vec<String>) -> Vec<Arc<VfCurve>> {
    let mut curves = vec![Arc::new(VfCurve::linear())];
    for path in vcod_common::asset_source::AssetSource::list(fs, "soundaliases", ".vfcurve") {
        let stem = path
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .trim_end_matches(".vfcurve");
        let Some(bytes) = fs.read(&path) else {
            errors.push(format!("Could not load sndcurve file '{path}'"));
            continue;
        };
        match VfCurve::parse(stem, &String::from_utf8_lossy(&bytes)) {
            Ok(c) => curves.push(Arc::new(c)),
            Err(e) => errors.push(format!("Failed to load sndcurve file: {e}")),
        }
    }
    curves
}

/// Every CoD2 alias csv, curve and volume group under `fs`.
pub fn load(fs: &dyn AssetSource) -> AliasTable {
    let mut errors = Vec::new();
    let groups = match fs.read("soundaliases/volumemodgroups.def") {
        Some(b) => parse_volume_mod_groups(&String::from_utf8_lossy(&b)).unwrap_or_else(|e| {
            errors.push(format!("volumemodgroups.def: {e}"));
            Vec::new()
        }),
        None => {
            errors.push("Could not find 'soundaliases/volumemodgroups.def'".into());
            Vec::new()
        }
    };
    let curves = load_curves(fs, &mut errors);
    let ctx = Ctx {
        groups: &groups,
        curves: &curves,
    };
    let mut table = AliasTable {
        cod2: true,
        errors,
        ..AliasTable::default()
    };
    for path in vcod_common::asset_source::AssetSource::list(fs, "soundaliases", ".csv") {
        let Some(bytes) = fs.read(&path) else {
            table.errors.push(format!("{path}: listed but unreadable"));
            continue;
        };
        let file = path.rsplit('/').next().unwrap_or(&path);
        table.merge(parse_csv(&String::from_utf8_lossy(&bytes), file, &ctx));
    }
    for e in &table.errors {
        log::warn!("sound aliases: {e}");
    }
    log::info!(
        "sound aliases: {} CoD2 names, {} curves, {} volume groups",
        table.len(),
        curves.len(),
        groups.len()
    );
    table
}

/// One priority's environment (`SND_SetEnvironmentEffects`, Mac 0x4a782):
/// the room name, and the dry and wet levels each as (current, target,
/// change per ms).
#[derive(Clone, Debug, PartialEq)]
struct Env {
    room: String,
    dry: [f32; 3],
    wet: [f32; 3],
    active: bool,
}

/// CoD2's three environment priorities (default, `snd_enveffectsprio_level`
/// 1, `snd_enveffectsprio_shellshock` 2) and the one in force: the highest
/// active (`g_snd` +0x2d8 points at it). Slot 0 is `SND_Init`'s room 0,
/// dry 1, wet 0 (Mac 0x4b496), and nothing deactivates it. The wet level
/// is the reverb send every voice but a `nowetlevel` row gets
/// (`SND_Update3DChannelReverb`, Mac 0x568fa); the dry level fades and is
/// read by no mixer path (cod2-sound.md 7.12).
#[derive(Clone, Debug, PartialEq)]
pub struct EnvEffects {
    slots: [Env; 3],
    active: usize,
}

impl Default for EnvEffects {
    fn default() -> Self {
        let off = Env {
            room: String::new(),
            dry: [1.0, 1.0, 0.0],
            wet: [0.0; 3],
            active: false,
        };
        let mut slots = [off.clone(), off.clone(), off];
        slots[0].active = true;
        EnvEffects { slots, active: 0 }
    }
}

impl EnvEffects {
    /// `SND_SetEnvironmentEffects` (Mac 0x4a782): `prio`'s levels fade from
    /// the ones in force to `dry` and `wet` over `fade_ms` (at once under
    /// 1), and it takes over unless a higher priority is active.
    pub fn set(&mut self, prio: usize, room: &str, dry: f32, wet: f32, fade_ms: i32) {
        if prio > 2 {
            return;
        }
        let f = if fade_ms > 0 { fade_ms as f32 } else { 1.0 };
        let (cur_dry, cur_wet) = (
            self.slots[self.active].dry[0],
            self.slots[self.active].wet[0],
        );
        let s = &mut self.slots[prio];
        s.active = true;
        s.room = room.to_string();
        s.dry = [cur_dry, dry, (dry - cur_dry) / f];
        s.wet = [cur_wet, wet, (wet - cur_wet) / f];
        if prio == self.active || !self.slots[prio + 1..].iter().any(|s| s.active) {
            self.active = prio;
        }
    }

    /// `SND_DeactivateEnvironmentEffects` (Mac 0x4a682): `prio` goes off;
    /// if it was in force, the next lower active one takes over, its
    /// levels fading from the ones just in force over `fade_ms`.
    pub fn deactivate(&mut self, prio: usize, fade_ms: i32) {
        if prio > 2 {
            return;
        }
        self.slots[prio].active = false;
        if prio != self.active {
            return;
        }
        let next = (0..prio).rev().find(|&i| self.slots[i].active).unwrap_or(0);
        let f = if fade_ms > 0 { fade_ms as f32 } else { 1.0 };
        let (cur_dry, cur_wet) = (self.slots[prio].dry[0], self.slots[prio].wet[0]);
        let s = &mut self.slots[next];
        s.dry = [cur_dry, s.dry[1], (s.dry[1] - cur_dry) / f];
        s.wet = [cur_wet, s.wet[1], (s.wet[1] - cur_wet) / f];
        self.active = next;
    }

    /// `SND_Update`'s step of the levels in force over `ms` (Mac 0x4df64):
    /// each moves by its rate and stops on its target.
    pub fn advance(&mut self, ms: f32) {
        let s = &mut self.slots[self.active];
        for l in [&mut s.dry, &mut s.wet] {
            if l[2] == 0.0 {
                continue;
            }
            l[0] += l[2] * ms;
            if (l[2] < 0.0 && l[0] <= l[1]) || (l[2] > 0.0 && l[1] <= l[0]) {
                l[0] = l[1];
                l[2] = 0.0;
            }
        }
    }

    /// The wet level in force.
    pub fn wet(&self) -> f32 {
        self.slots[self.active].wet[0]
    }

    /// The room in force, `""` for slot 0's. vcod's one reverb stands in
    /// for every Miles room type, so only the tests read it.
    #[cfg(test)]
    pub fn room(&self) -> &str {
        &self.slots[self.active].room
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::alias::Pick;

    /// `weapon1.vfcurve` as iw_07.iwd ships it, CRLF and all.
    const WEAPON1: &str = "SNDCURVE\r\n\r\n5\r\n0.0000 1.0000\r\n0.0415 0.6839\r\n0.2796 0.4024\r\n0.6314 0.1975\r\n1.0000 0.0000\r\n";

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn a_curve_samples_between_its_knots() {
        let c = VfCurve::parse("weapon1", WEAPON1).unwrap();
        assert_eq!(c.knots.len(), 5);
        assert_eq!(c.value(0.0), 1.0);
        assert!(close(c.value(0.0415), 0.6839));
        // Half way from the second knot to the third.
        let mid = (0.0415 + 0.2796) / 2.0;
        assert!(close(c.value(mid), (0.6839 + 0.4024) / 2.0));
        assert!(close(c.value(1.0), 0.0));
        // Past the last knot: `GraphGetValueFromFraction`'s -1.
        assert_eq!(c.value(1.5), -1.0);
        // weap_kar98k_fire, 35 to 8400: full inside, nothing at the edge.
        assert_eq!(c.attenuate(10.0, 35.0, 8400.0), 1.0);
        assert_eq!(c.attenuate(8400.0, 35.0, 8400.0), 0.0);
        let d = 35.0 + 0.2796 * (8400.0 - 35.0);
        assert!((c.attenuate(d, 35.0, 8400.0) - 0.4024).abs() < 1e-4);
        assert_eq!(c.attenuate(f32::NAN, 35.0, 8400.0), 0.0);
        // The default curve is CoD 1's straight line.
        let l = VfCurve::linear();
        assert!(close(
            l.attenuate(360.0, 120.0, 600.0),
            crate::audio::spatial::falloff(360.0, 120.0, 600.0)
        ));
    }

    #[test]
    fn bad_curves_are_refused_and_endpoints_fixed() {
        assert!(VfCurve::parse("x", "SNDCURVE 1 0 1").is_err());
        assert!(VfCurve::parse("x", "SNDCURVE 9 0 1").is_err());
        assert!(VfCurve::parse("x", "SNDCURVE 3 0 1 1 0").is_err());
        assert!(VfCurve::parse("x", "SNDCURVE 2 0 1 1.5 0").is_err());
        assert!(VfCurve::parse("x", "CURVE 2 0 1 1 0").is_err());
        let c = VfCurve::parse("x", "SNDCURVE 2 0 0.5 1 0.25").unwrap();
        assert_eq!(c.knots, vec![[0.0, 1.0], [1.0, 0.0]]);
    }

    #[test]
    fn loadspec_follows_the_client_rule() {
        let m = "mp_toujane";
        assert!(loadspec_admits("", m));
        assert!(loadspec_admits("mp_toujane mp_burgundy", m));
        assert!(!loadspec_admits("mp_burgundy", m));
        assert!(!loadspec_admits("mp_toujanex", m));
        assert!(!loadspec_admits("!mp_toujane !mp_burgundy", m));
        assert!(loadspec_admits("!mp_burgundy", m));
        assert!(loadspec_admits("all_mp", m));
        assert!(!loadspec_admits("all_sp", m));
        // `step_run_dirt`'s: the SP town, not the MP map.
        assert!(loadspec_admits("!decoytrenches !toujane !matmata", m));
        assert!(!loadspec_admits("!decoytrenches !toujane", "toujane"));
    }

    const HEADER: &str = "name,sequence,file,vol_min,vol_max,vol_mod,pitch_min,pitch_max,dist_min,dist_max,channel,type,probability,loop,masterslave,loadspec,subtitle,compression,secondaryaliasname,volumefalloffcurve,startdelay,speakermap,reverb,lfe percentage";

    fn ctx_parse(text: &str) -> AliasTable {
        let groups = vec![("wpnai".to_string(), 1.02), ("footstep".to_string(), 0.9)];
        let curves = vec![
            Arc::new(VfCurve::linear()),
            Arc::new(VfCurve::parse("weapon2", WEAPON1).unwrap()),
        ];
        parse_csv(
            text,
            "test.csv",
            &Ctx {
                groups: &groups,
                curves: &curves,
            },
        )
    }

    #[test]
    fn a_cod2_row_reads_every_column() {
        let t = ctx_parse(&format!(
            "{HEADER}\nweap_kar98k_fire,,weapons/kar98k/wpnfire_k98_1b.wav,0.6,0.65,wpnai,0.96,1.32,35,8400,weapon,,0.3,,0.7,,,,whizby,weapon2,5,radio,,0.2\n"
        ));
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        let r = &t.get("weap_kar98k_fire").unwrap()[0];
        assert!(close(r.vol.0, 0.6 * 1.02) && close(r.vol.1, 0.65 * 1.02));
        assert_eq!(r.pitch, (0.96, 1.32));
        assert_eq!(r.dist, (35.0, 8400.0));
        assert_eq!(r.channel, Channel::Weapon);
        assert_eq!(r.probability, 300);
        assert_eq!(r.master_slave, MasterSlave::Slave(0.7));
        assert_eq!(r.secondary.as_deref(), Some("whizby"));
        assert_eq!(r.curve.as_ref().unwrap().name, "weapon2");
        assert_eq!(r.start_delay_ms, 5);
        assert_eq!(r.csv, "test.csv");
    }

    #[test]
    fn row_checks_drop_the_row_and_list_the_error() {
        let t = ctx_parse(&format!(
            "{HEADER}\nloud,,a.wav,1.5,,,,,,,,,,,,,,,,,,,,\nbad_chan,,a.wav,,,,,,,,nowhere,,,,,,,,,,,,,\nbad-name,,a.wav\nnear,,a.wav,,,,,,500,100\nflat,,a.wav,,,,,,,,auto2d\n"
        ));
        assert_eq!(t.errors.len(), 4, "{:?}", t.errors);
        assert_eq!(t.len(), 1);
        let r = &t.get("flat").unwrap()[0];
        assert_eq!(r.channel, Channel::Auto2d);
        assert!(!r.channel.is_spatial());
        // Blank dist_max: five times dist_min.
        assert_eq!(r.dist, (120.0, 600.0));
        // A row with no name ends the file.
        let t = ctx_parse(&format!("{HEADER}\na,,a.wav\n,x\nnofile,1\nb,,b.wav\n"));
        assert_eq!(t.len(), 1);
        assert_eq!(t.errors.len(), 1);
    }

    #[test]
    fn duplicate_sequences_settle_by_csv() {
        let mut a = ctx_parse(&format!(
            "{HEADER}\nn,1,a.wav\nn,2,b.wav\nm,1,x.wav\nm,1,y.wav\n"
        ));
        let mut b = ctx_parse(&format!("{HEADER}\nn,1,c.wav\n"));
        for r in b.rows.values_mut().flatten() {
            r.csv = "z.csv".into();
        }
        a.merge(b);
        let t = a.for_map("mp_toujane");
        let n: Vec<_> = t
            .get("n")
            .unwrap()
            .iter()
            .map(|r| r.file.as_str())
            .collect();
        assert_eq!(n, ["c.wav", "b.wav"]);
        assert!(t.get("m").is_none());
        assert_eq!(t.errors.len(), 1);
        assert!(matches!(t.pick("n", 0.9, None).0, Pick::Row(r) if r.file == "b.wav"));
    }

    /// Every stock csv, curve and group under `COD2_DIR` loads without
    /// an error on every stock MP map, and every file a row names is in
    /// the iwds.
    #[test]
    fn stock_cod2_aliases_load_and_resolve() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let all = load(&fs);
        assert!(all.errors.is_empty(), "{:#?}", all.errors);
        assert!(all.len() > 5000, "{} names", all.len());
        let mut maps: Vec<String> = fs
            .list_prefix("maps/mp/")
            .into_iter()
            .filter_map(|p| {
                p.strip_suffix(".d3dbsp")
                    .map(|m| m["maps/mp/".len()..].to_string())
            })
            .collect();
        maps.sort();
        assert!(maps.len() >= 13, "{maps:?}");
        for map in &maps {
            let t = all.for_map(map);
            assert!(t.errors.is_empty(), "{map}: {:#?}", t.errors);
        }
        let t = all.for_map("mp_toujane");
        let fire = &t.get("weap_kar98k_fire").unwrap()[0];
        assert_eq!(fire.curve.as_ref().unwrap().name, "weapon2");
        assert_eq!(fire.channel, Channel::Weapon);
        assert!(t.get("step_run_dirt").is_some() && t.get("grenade_explode_default").is_some());
        // Every file a row names, and the ones a row some MP map loads names.
        let missing = |t: &AliasTable| {
            let mut files: Vec<String> = t
                .rows
                .values()
                .flatten()
                .map(|r| r.file.to_ascii_lowercase())
                .collect();
            files.sort_unstable();
            files.dedup();
            let gone: Vec<String> = files
                .iter()
                .filter(|f| {
                    fs.source_archive(&format!("sound/{}", f.trim_start_matches('/')))
                        .is_none()
                })
                .cloned()
                .collect();
            (files.len(), gone)
        };
        let (total, gone) = missing(&all);
        println!(
            "{} names, {total} sound files, {} missing: {gone:?}",
            all.len(),
            gone.len()
        );
        // Retail's own gaps: five single-player files and the MP `beep`
        // row's `misc/beep.wav` (iw_multiplayer2.csv), which no iwd holds.
        assert_eq!(gone.len(), 6, "{gone:?}");
        for map in &maps {
            let (n, gone) = missing(&all.for_map(map));
            println!("{map}: {n} sound files, missing {gone:?}");
            assert_eq!(gone, ["misc/beep.wav"], "{map}");
        }
    }

    /// Leaving a game asks for the menu's rows with no map name; that
    /// once panicked walking past the end of the cell.
    #[test]
    fn an_empty_map_name_reads_as_the_menu() {
        assert_eq!(
            super::loadspec_admits("mp_toujane", ""),
            super::loadspec_admits("mp_toujane", "menu")
        );
        assert_eq!(
            super::loadspec_admits("!all_mp", ""),
            super::loadspec_admits("!all_mp", "menu")
        );
    }

    /// `setReverb` at the level priority fades the wet level in; the
    /// shellshock priority overrides it while active, and deactivating it
    /// fades back to the level's from where it was.
    #[test]
    fn environment_priorities_fade_and_hand_back() {
        let mut e = EnvEffects::default();
        assert_eq!((e.wet(), e.room()), (0.0, ""));
        e.set(1, "stonecorridor", 1.0, 0.5, 1000);
        e.advance(500.0);
        assert!((e.wet() - 0.25).abs() < 1e-6);
        e.advance(600.0);
        assert_eq!((e.wet(), e.room()), (0.5, "stonecorridor"));
        // The shellshock priority starts from the level in force.
        e.set(2, "underwater", 1.0, 0.9, 0);
        e.advance(16.0);
        assert_eq!((e.wet(), e.room()), (0.9, "underwater"));
        // A level change under it waits its turn.
        e.set(1, "hangar", 1.0, 0.1, 0);
        assert_eq!(e.room(), "underwater");
        e.deactivate(2, 400);
        assert_eq!((e.wet(), e.room()), (0.9, "hangar"));
        e.advance(200.0);
        assert!((e.wet() - 0.5).abs() < 1e-6);
        e.advance(300.0);
        assert_eq!(e.wet(), 0.1);
        // Deactivating a priority not in force changes nothing.
        e.deactivate(2, 0);
        assert_eq!(e.wet(), 0.1);
        // Slot 0 takes over from the level in force, at once for fade 0.
        e.deactivate(1, 0);
        e.advance(1.0);
        assert_eq!((e.wet(), e.room()), (0.0, ""));
    }

    /// The `subtitle` and `reverb` cells reach the row.
    #[test]
    fn subtitle_and_nowetlevel_reach_the_row() {
        let ctx = Ctx {
            groups: &[],
            curves: &[],
        };
        let csv = "name,file,subtitle,reverb\nvo_a,a.wav,SUBTITLE_VO_A,nowetlevel fulldrylevel\nvo_b,b.wav,,\n";
        let t = parse_csv(csv, "t.csv", &ctx);
        let a = &t.rows["vo_a"][0];
        assert_eq!(a.subtitle.as_deref(), Some("SUBTITLE_VO_A"));
        assert!(a.no_wet);
        let b = &t.rows["vo_b"][0];
        assert_eq!((b.subtitle.as_deref(), b.no_wet), (None, false));
    }
}
