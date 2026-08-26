//! `soundaliases/*.csv` parser (docs/research/cod11-sound-system.md, section
//! 1). Columns bind by header name; `dialog_generic.csv` shuffles them.

use std::collections::HashMap;

use vcod_common::pk3::Pk3Fs;

/// Blank `dist_min`; `FUN_00432ad0` @ `CoDMP.exe 0x432ad0` (research doc,
/// section 1c).
pub const DEFAULT_DIST_MIN: f32 = 120.0;
/// Blank or `0` `dist_max` becomes `dist_min * 5` (research doc, section 1c,
/// `_DAT_005690f4`).
pub const DIST_MAX_FACTOR: f32 = 5.0;

/// Channel table order from `CoDMP.exe 0x57acec` (research doc, section 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Auto,
    Menu,
    Weapon,
    Voice,
    Item,
    Body,
    Local,
    Music,
    Announcer,
    Shellshock,
}

impl Channel {
    /// False for the 2D channels `menu, local, music, announcer, shellshock`
    /// (research doc, section 2).
    pub fn is_spatial(self) -> bool {
        !matches!(
            self,
            Channel::Menu
                | Channel::Local
                | Channel::Music
                | Channel::Announcer
                | Channel::Shellshock
        )
    }

    pub fn parse(s: &str) -> Option<Channel> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Channel::Auto,
            "menu" => Channel::Menu,
            "weapon" => Channel::Weapon,
            "voice" => Channel::Voice,
            "item" => Channel::Item,
            "body" => Channel::Body,
            "local" => Channel::Local,
            "music" => Channel::Music,
            "announcer" => Channel::Announcer,
            "shellshock" => Channel::Shellshock,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MasterSlave {
    None,
    Master,
    /// Absolute gain cap while a master plays (research doc, section 4). A
    /// bare `slave` is `Slave(0.0)`, the engine's `atof("")`.
    Slave(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AliasRow {
    pub name: String,
    /// Relative to `sound/`, extension included.
    pub file: String,
    /// Sort key within a name group, not a play order.
    pub sequence: i64,
    pub vol: (f32, f32),
    pub pitch: (f32, f32),
    /// `(dist_min, dist_max)`, never `0` (research doc, section 1c fix-up).
    pub dist: (f32, f32),
    pub channel: Channel,
    pub streamed: bool,
    pub looping: bool,
    pub probability: u32,
    pub master_slave: MasterSlave,
    /// Trimmed, lowercased raw cell; `for_map` matches against it (research
    /// doc, section 1d).
    pub loadspec: String,
}

#[derive(Default, Debug)]
pub struct AliasTable {
    rows: HashMap<String, Vec<AliasRow>>,
    /// Rows dropped for a missing name or file.
    pub skipped: usize,
}

/// Upper bound for a row's `probability` weight.
pub const MAX_PROBABILITY: u32 = 1_000_000;

#[derive(Debug)]
pub enum Pick<'a> {
    Unknown,
    /// A `null.wav` row (research doc, section 1f). It still counts for the
    /// no-repeat rule, so its index comes back like any pick.
    Silent,
    Row(&'a AliasRow),
}

fn is_null_file(file: &str) -> bool {
    file.eq_ignore_ascii_case("null.wav") || file.eq_ignore_ascii_case("null2.wav")
}

/// Whole-word match with the engine's boundary test, bytes `< b'!'`
/// (research doc, section 1d).
fn contains_whole_word(cell: &str, word: &str) -> bool {
    if word.is_empty() || word.len() > cell.len() {
        return false;
    }
    let cell = cell.as_bytes();
    let word = word.as_bytes();
    (0..=cell.len() - word.len()).any(|start| {
        let end = start + word.len();
        cell[start..end] == *word
            && (start == 0 || cell[start - 1] < b'!')
            && (end == cell.len() || cell[end] < b'!')
    })
}

/// Whether a `loadspec` cell admits `map` (research doc, section 1d,
/// `FUN_00432d20`). Both arguments are already lowercased.
fn loadspec_admits(cell: &str, map: &str) -> bool {
    if cell.is_empty() {
        // Blank defaults to included on the map path (research doc, section
        // 1c).
        return true;
    }
    if let Some(rest) = cell.strip_prefix('!') {
        let remainder = rest.trim_start_matches(|c: char| c <= ' ');
        if remainder == map {
            return false;
        }
        // `!all_mp` excludes everywhere; `!all_sp` falls through to admitted.
        return remainder != "all_mp";
    }
    if contains_whole_word(cell, map) {
        return true;
    }
    // Not found as a whole word; only the literal cell `all_mp` still admits.
    cell == "all_mp"
}

/// Splits on commas outside double quotes; quotes are stripped, so `""` is an
/// empty cell. An unterminated quote swallows the rest of the line.
fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// (`sequence`, `file` case-insensitive), the engine's merge-sort order
/// (research doc, section 1e); `pick`'s no-repeat index refers to it.
fn sort_group(rows: &mut [AliasRow]) {
    rows.sort_by(|a, b| {
        a.sequence.cmp(&b.sequence).then_with(|| {
            a.file
                .to_ascii_lowercase()
                .cmp(&b.file.to_ascii_lowercase())
        })
    });
}

impl AliasTable {
    /// `source` labels warnings.
    pub fn parse_csv(text: &str, source: &str) -> AliasTable {
        let mut table = AliasTable::default();
        let mut cols: Option<HashMap<String, usize>> = None;
        let mut warned_channel = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("\"#") {
                continue;
            }
            let cells = split_csv(line);
            if cells[0].trim().is_empty() {
                // The engine skips any row with a blank first cell, like a
                // `#` row (research doc, section 1b); not counted in
                // `skipped`.
                continue;
            }
            let Some(cols) = &cols else {
                // First data-looking line is the header.
                let map = cells
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| !c.trim().is_empty())
                    .map(|(i, c)| (c.trim().to_ascii_lowercase(), i))
                    .collect();
                cols = Some(map);
                continue;
            };
            let cell = |key: &str| -> &str {
                cols.get(key)
                    .and_then(|&i| cells.get(i))
                    .map(|s| s.trim())
                    .unwrap_or("")
            };
            let f = |key: &str| -> Option<f32> { cell(key).parse::<f32>().ok() };
            let name = cell("name").to_ascii_lowercase();
            let file = cell("file").to_string();
            if name.is_empty() || file.is_empty() {
                table.skipped += 1;
                continue;
            }

            let mut vol_min = f("vol_min").unwrap_or(1.0);
            let mut vol_max = f("vol_max").unwrap_or(vol_min);
            let mut pitch_min = f("pitch_min").unwrap_or(1.0);
            let mut pitch_max = f("pitch_max").unwrap_or(pitch_min);
            if pitch_max < pitch_min {
                std::mem::swap(&mut pitch_max, &mut pitch_min);
            }
            if vol_max < vol_min {
                std::mem::swap(&mut vol_max, &mut vol_min);
            }

            let dist_min = f("dist_min").unwrap_or(DEFAULT_DIST_MIN);
            let mut dist_max = f("dist_max").unwrap_or(0.0);
            if dist_max == 0.0 {
                dist_max = dist_min * DIST_MAX_FACTOR;
            }

            let channel = match Channel::parse(cell("channel")) {
                Some(c) => c,
                None => {
                    if !warned_channel {
                        log::warn!(
                            "{source}: unknown sound channel '{}': treating as auto (further \
                             unknown channels in this file are not logged)",
                            cell("channel")
                        );
                        warned_channel = true;
                    }
                    Channel::Auto
                }
            };

            let ms = cell("masterslave").to_ascii_lowercase();
            let master_slave = match ms.as_str() {
                "" => MasterSlave::None,
                "master" => MasterSlave::Master,
                other => MasterSlave::Slave(other.parse::<f32>().unwrap_or(0.0)),
            };

            // Clamped so a group's weights sum in u32; NaN falls to 1 via max.
            let probability = cell("probability")
                .parse::<f32>()
                .map(|p| p.round().max(1.0).min(MAX_PROBABILITY as f32) as u32)
                .unwrap_or(1);

            let row = AliasRow {
                name: name.clone(),
                file,
                sequence: cell("sequence").parse::<i64>().unwrap_or(0),
                vol: (vol_min, vol_max),
                pitch: (pitch_min, pitch_max),
                dist: (dist_min, dist_max),
                channel,
                streamed: cell("type").eq_ignore_ascii_case("streamed"),
                looping: cell("loop").eq_ignore_ascii_case("looping"),
                probability,
                master_slave,
                loadspec: cell("loadspec").to_ascii_lowercase(),
            };
            table.rows.entry(name).or_default().push(row);
        }
        for rows in table.rows.values_mut() {
            sort_group(rows);
        }
        table
    }

    pub fn merge(&mut self, other: AliasTable) {
        for (name, rows) in other.rows {
            let entry = self.rows.entry(name).or_default();
            entry.extend(rows);
            sort_group(entry);
        }
        self.skipped += other.skipped;
    }

    /// Every `soundaliases/*.csv`. `names_with_suffix` dedups paths with the
    /// later pak winning; a duplicate name across files adds rows to the
    /// group (research doc, section 1a).
    pub fn load(fs: &Pk3Fs) -> AliasTable {
        let mut table = AliasTable::default();
        for path in fs.names_with_suffix(".csv") {
            if !path.starts_with("soundaliases/") {
                continue;
            }
            match fs.read(&path) {
                Some(bytes) => {
                    let t = AliasTable::parse_csv(&String::from_utf8_lossy(&bytes), &path);
                    if t.skipped > 0 {
                        log::warn!("{path}: skipped {} rows without name/file", t.skipped);
                    }
                    table.merge(t);
                }
                None => log::warn!("{path}: listed but unreadable"),
            }
        }
        log::info!("sound aliases: {} names", table.len());
        table
    }

    pub fn get(&self, name: &str) -> Option<&[AliasRow]> {
        self.rows.get(&name.to_ascii_lowercase()).map(Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Rows that apply on `map` (research doc, section 1d); group order is
    /// preserved.
    pub fn for_map(&self, map: &str) -> AliasTable {
        let map = map.to_ascii_lowercase();
        let mut out = AliasTable {
            skipped: self.skipped,
            ..AliasTable::default()
        };
        for (name, rows) in &self.rows {
            let kept: Vec<AliasRow> = rows
                .iter()
                .filter(|r| loadspec_admits(&r.loadspec, &map))
                .cloned()
                .collect();
            if !kept.is_empty() {
                out.rows.insert(name.clone(), kept);
            }
        }
        out
    }

    /// Weighted variant for `name`; `roll` is uniform in `[0, 1)`. With 3+
    /// rows `last` is excluded and the weights renormalise (research doc,
    /// section 1e). The returned index is into the full group, `Silent`
    /// included, so it feeds back as `last`.
    pub fn pick(&self, name: &str, roll: f32, last: Option<usize>) -> (Pick<'_>, Option<usize>) {
        let Some(rows) = self.get(name) else {
            return (Pick::Unknown, None);
        };
        let exclude = last.filter(|&i| rows.len() >= 3 && i < rows.len());
        let total: u32 = rows
            .iter()
            .enumerate()
            .filter(|&(i, _)| Some(i) != exclude)
            .map(|(_, r)| r.probability)
            .sum();
        if total == 0 {
            // Unreachable while `probability` parses to at least 1.
            return (Pick::Unknown, None);
        }
        let mut target = (roll.clamp(0.0, 0.999_999) * total as f32) as u32;
        for (i, r) in rows.iter().enumerate() {
            if Some(i) == exclude {
                continue;
            }
            if target < r.probability {
                let pick = if is_null_file(&r.file) {
                    Pick::Silent
                } else {
                    Pick::Row(r)
                };
                return (pick, Some(i));
            }
            target -= r.probability;
        }
        (Pick::Unknown, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "name,sequence,file,vol_min,vol_max,pitch_min,pitch_max,dist_min,dist_max,channel,type,probability,loop,masterslave,loadspec,subtitle";

    #[test]
    fn parses_a_weapon_row() {
        let text = format!(
            "# legend line, ignored\n\"# quoted, legend, line\"\n{HEADER}\nweap_thompson_fire,,weapons/thompson/thompson_01.wav,1.25,1.35,0.9,1.05,7,7800,weapon,,,,,,\n"
        );
        let t = AliasTable::parse_csv(&text, "test");
        assert!(!t.is_empty());
        let rows = t.get("WEAP_THOMPSON_FIRE").unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.file, "weapons/thompson/thompson_01.wav");
        assert_eq!(r.vol, (1.25, 1.35));
        assert_eq!(r.pitch, (0.9, 1.05));
        assert_eq!(r.dist, (7.0, 7800.0));
        assert_eq!(r.channel, Channel::Weapon);
        assert!(!r.streamed && !r.looping);
        assert_eq!(r.probability, 1);
        assert_eq!(r.master_slave, MasterSlave::None);
        assert!(r.loadspec.is_empty());
        assert_eq!(t.skipped, 0);
    }

    #[test]
    fn defaults_and_flags() {
        let text = format!(
            "{HEADER}\nambient_mp_carentan,,ambient/amb_brecourt_ext.mp3,0.7,,,,,,local,streamed,,looping,,all_mp,\nrocket_fuel_pump_loop,,misc/pump.wav,1,,,,100,1000,auto,,,looping,slave,rocket,\nwhizby,16,null.wav,0.85,1,0.8,1.1,100,700,auto,,5,,,!airfield,\nx,,y.wav,1,,,,,,,,,,25,! Airfield,"
        );
        let t = AliasTable::parse_csv(&text, "test");
        let amb = &t.get("ambient_mp_carentan").unwrap()[0];
        assert_eq!(amb.vol, (0.7, 0.7)); // vol_max blank = vol_min
        assert_eq!(amb.pitch, (1.0, 1.0));
        assert_eq!(amb.dist, (120.0, 600.0)); // dist_min blank -> 120, dist_max blank -> 5x
        assert_eq!(amb.channel, Channel::Local);
        assert!(amb.streamed && amb.looping);
        assert_eq!(amb.loadspec, "all_mp");
        let pump = &t.get("rocket_fuel_pump_loop").unwrap()[0];
        assert_eq!(pump.master_slave, MasterSlave::Slave(0.0)); // bare "slave" = atof("") = 0
        let wb = &t.get("whizby").unwrap()[0];
        assert_eq!(wb.probability, 5);
        assert_eq!(wb.loadspec, "!airfield");
        let x = &t.get("x").unwrap()[0];
        assert_eq!(x.master_slave, MasterSlave::Slave(25.0));
        assert_eq!(x.loadspec, "! airfield");
    }

    #[test]
    fn binds_columns_by_header_name() {
        // dialog_generic.csv order: probability before file, blank vol_max header.
        let text = "name,sequence,probability,file,vol_min,,pitch_min,pitch_max,dist_min,dist_max,channel,type,loop,masterslave,loadspec,subtitle\nsay_hi,1,3,voice/hi.wav,0.5,,,,,,voice,,,,,\n";
        let t = AliasTable::parse_csv(text, "test");
        let r = &t.get("say_hi").unwrap()[0];
        assert_eq!(r.probability, 3);
        assert_eq!(r.file, "voice/hi.wav");
        assert_eq!(r.vol, (0.5, 0.5));
        assert_eq!(r.channel, Channel::Voice);
    }

    #[test]
    fn skips_rows_without_name_or_file_and_unknown_channels() {
        let text = format!("{HEADER}\n,,a.wav,,,,,,,,,,,,,\nnofile,,,,,,,,,,,,,,,\nbadchan,,a.wav,,,,,,,bogus,,,,,,\nok,,a.wav,,,,,,,,,,,,,\n");
        let t = AliasTable::parse_csv(&text, "test");
        assert_eq!(t.len(), 2); // badchan keeps the row with Channel::Auto, ok too
        assert_eq!(t.get("badchan").unwrap()[0].channel, Channel::Auto);
        // A blank first cell is ignored like a comment; only "nofile" counts.
        assert_eq!(t.skipped, 1);
    }

    #[test]
    fn swaps_inverted_vol_and_pitch_and_rounds_probability() {
        let text = format!(
            "{HEADER}\ninverted,,a.wav,1.2,0.8,1.1,0.9,,,,,,,,\nround_up,,a.wav,,,,,,,,,2.6,,,,\nround_to_min,,a.wav,,,,,,,,,0.4,,,,\n"
        );
        let t = AliasTable::parse_csv(&text, "test");
        let inv = &t.get("inverted").unwrap()[0];
        assert_eq!(inv.vol, (0.8, 1.2));
        assert_eq!(inv.pitch, (0.9, 1.1));
        assert_eq!(t.get("round_up").unwrap()[0].probability, 3);
        assert_eq!(t.get("round_to_min").unwrap()[0].probability, 1);
    }

    #[test]
    fn huge_probabilities_are_clamped_and_still_pick() {
        let text =
            format!("{HEADER}\nbig,1,a.wav,,,,,,,,,1e30,,,,\nbig,2,b.wav,,,,,,,,,1e30,,,,\n");
        let t = AliasTable::parse_csv(&text, "test");
        let rows = t.get("big").unwrap();
        assert_eq!(rows[0].probability, MAX_PROBABILITY);
        assert_eq!(rows[1].probability, MAX_PROBABILITY);
        assert!(matches!(t.pick("big", 0.0, None).0, Pick::Row(r) if r.file == "a.wav"));
        assert!(matches!(t.pick("big", 0.75, None).0, Pick::Row(r) if r.file == "b.wav"));
    }

    #[test]
    fn merge_appends_rows_for_the_same_name() {
        let mut a = AliasTable::parse_csv(&format!("{HEADER}\nn,1,a.wav,,,,,,,,,,,,,\n"), "test");
        let b = AliasTable::parse_csv(&format!("{HEADER}\nn,2,b.wav,,,,,,,,,,,,,\n"), "test");
        a.merge(b);
        assert_eq!(a.get("n").unwrap().len(), 2);
    }

    #[test]
    fn sorts_rows_by_sequence_within_a_name_group() {
        let text = format!("{HEADER}\nn,2,b.wav,,,,,,,,,,,,,\nn,1,a.wav,,,,,,,,,,,,,\n");
        let t = AliasTable::parse_csv(&text, "test");
        let rows = t.get("n").unwrap();
        assert_eq!(rows[0].sequence, 1);
        assert_eq!(rows[1].sequence, 2);
    }

    #[test]
    fn merge_keeps_rows_sorted_by_sequence() {
        let mut a = AliasTable::parse_csv(&format!("{HEADER}\nn,2,b.wav,,,,,,,,,,,,,\n"), "test");
        let b = AliasTable::parse_csv(&format!("{HEADER}\nn,1,a.wav,,,,,,,,,,,,,\n"), "test");
        a.merge(b);
        let rows = a.get("n").unwrap();
        assert_eq!(rows[0].sequence, 1);
        assert_eq!(rows[1].sequence, 2);
    }

    #[test]
    fn for_map_applies_loadspec() {
        // Raw cells, not a token list.
        let text = format!(
            "{HEADER}\nfire,1,a.wav,,,,,,,,,,,,! Airfield,\nfire,2,b.wav,,,,,,,,,,,,airfield,\nany,,c.wav,,,,,,,,,,,,,\nmp,,d.wav,,,,,,,,,,,,all_mp,\nsp,,e.wav,,,,,,,,,,,,all_sp,\nmenu,,f.wav,,,,,,,,,,,,menu,\nnotsp,,g.wav,,,,,,,,,,,,!all_sp,\nmulti,,h.wav,,,,,,,,,,,,airfield mp_carentan,\nsubstr,,i.wav,,,,,,,,,,,,mp_carentanx,\nnomp,,j.wav,,,,,,,,,,,,!all_mp,\nprefixthenword,,k.wav,,,,,,,,,,,,mp_carentanx mp_carentan,\n"
        );
        let t = AliasTable::parse_csv(&text, "test");

        let carentan = t.for_map("mp_carentan");
        assert_eq!(carentan.get("fire").unwrap()[0].file, "a.wav");
        assert!(carentan.get("any").is_some());
        assert!(carentan.get("mp").is_some());
        assert!(carentan.get("sp").is_none());
        assert!(carentan.get("menu").is_none());
        assert!(carentan.get("notsp").is_some());
        assert!(carentan.get("multi").is_some());
        assert!(carentan.get("substr").is_none());
        // `!all_mp` excludes everywhere, unlike `!all_sp`.
        assert!(carentan.get("nomp").is_none());
        // A first-hit-only `strstr` port would miss the later whole-word
        // occurrence.
        assert!(carentan.get("prefixthenword").is_some());

        let airfield = t.for_map("airfield");
        assert_eq!(airfield.get("fire").unwrap()[0].file, "b.wav");
        // Only the literal cell `all_mp` gets the not-found fallback; `all_sp`
        // is excluded on any map.
        assert!(airfield.get("mp").is_some());
        assert!(airfield.get("sp").is_none());
        assert!(airfield.get("notsp").is_some());
        assert!(airfield.get("multi").is_some());
        assert!(airfield.get("nomp").is_none());
    }

    #[test]
    fn pick_weights_variants_and_reports_silence() {
        let text = format!("{HEADER}\nw,1,a.wav,,,,,,,,,1,,,,\nw,2,b.wav,,,,,,,,,1,,,,\nw,3,null.wav,,,,,,,,,2,,,,\n");
        let t = AliasTable::parse_csv(&text, "test");

        let (p, idx) = t.pick("w", 0.0, None);
        assert!(matches!(p, Pick::Row(r) if r.file == "a.wav"));
        assert_eq!(idx, Some(0));

        let (p, idx) = t.pick("w", 0.26, None);
        assert!(matches!(p, Pick::Row(r) if r.file == "b.wav"));
        assert_eq!(idx, Some(1));

        let (p, idx) = t.pick("w", 0.5, None);
        assert!(matches!(p, Pick::Silent));
        assert_eq!(idx, Some(2));

        let (p, idx) = t.pick("w", 0.999, None);
        assert!(matches!(p, Pick::Silent));
        assert_eq!(idx, Some(2));

        // With 3+ rows the last-played row is excluded and the weights
        // renormalise.
        let (p, idx) = t.pick("w", 0.999, Some(2));
        assert!(matches!(p, Pick::Row(r) if r.file == "b.wav"));
        assert_eq!(idx, Some(1));

        let (p, idx) = t.pick("w", 0.0, Some(2));
        assert!(matches!(p, Pick::Row(r) if r.file == "a.wav"));
        assert_eq!(idx, Some(0));

        let (p, idx) = t.pick("nope", 0.3, None);
        assert!(matches!(p, Pick::Unknown));
        assert_eq!(idx, None);
    }

    #[test]
    fn pick_ignores_last_with_fewer_than_three_rows() {
        let text = format!("{HEADER}\nxy,1,x.wav,,,,,,,,,1,,,,\nxy,2,y.wav,,,,,,,,,1,,,,\n");
        let t = AliasTable::parse_csv(&text, "test");
        let (p, idx) = t.pick("xy", 0.0, Some(0));
        assert!(matches!(p, Pick::Row(r) if r.file == "x.wav"));
        assert_eq!(idx, Some(0));
    }

    #[test]
    fn retail_tables_parse() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let t = AliasTable::load(&fs);
        assert!(t.len() > 1500, "{} names", t.len());
        let th = &t.get("weap_thompson_fire").unwrap()[0];
        assert_eq!((th.dist, th.channel), ((7.0, 7800.0), Channel::Weapon));
        let wb = t.get("whizby").unwrap();
        assert!(wb
            .iter()
            .any(|r| r.file == "null.wav" && r.probability == 5));
        let amb = &t.get("ambient_mp_carentan").unwrap()[0];
        assert!(amb.streamed && amb.looping && amb.channel == Channel::Local);
        // pak1 has a `local` row on every install; the Deluxe paks add an
        // `auto` one (iw_sound2.csv) the 1.1 install lacks. See
        // `AudioSystem::set_ambient`.
        let ooa = t.get("player_out_of_ammo").unwrap();
        assert!(ooa.iter().any(|r| r.channel == Channel::Local));
        if ooa.len() > 1 {
            assert!(ooa.iter().any(|r| r.channel == Channel::Auto));
        }
        // Retail csvs carry blank-first-cell rows after the header; those are
        // comments, not `skipped`.
        assert_eq!(t.skipped, 0, "retail rows should all parse");
    }

    #[test]
    fn retail_mp40_per_map_override() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let t = AliasTable::load(&fs);
        let mp = t.for_map("mp_carentan");
        let rows = mp.get("weap_mp40_fire").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].dist, (7.0, 7800.0));
        let af = t.for_map("airfield");
        assert_eq!(af.get("weap_mp40_fire").unwrap()[0].dist, (7.0, 3000.0));
    }
}
