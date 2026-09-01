//! `mp/playeranim.script`: which animation a player plays, given its state.
//!
//! RTCW's character animation script format verbatim, ported from
//! `bg_animation.c` in the RTCW-MP sources (GPLv3), the same lineage as the
//! netcode. The file it reads and the index its names resolve to are in
//! docs/research/player-model-anim-system.md.

use crate::animtree::strip_comments;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};

/// Which condition a clause tests. `leaning` and `position` exist in the
/// format and appear nowhere in the multiplayer script, so they are not here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CondKind {
    Weapons,
    WeaponClass,
    WeaponPosition,
    Strafing,
    Movetype,
    Mounted,
    Firing,
}

impl CondKind {
    fn parse(word: &str) -> Option<CondKind> {
        Some(match word {
            "weapons" => CondKind::Weapons,
            "weaponclass" => CondKind::WeaponClass,
            "weapon_position" => CondKind::WeaponPosition,
            "strafing" => CondKind::Strafing,
            "movetype" => CondKind::Movetype,
            "mounted" => CondKind::Mounted,
            "firing" => CondKind::Firing,
            _ => return None,
        })
    }
}

/// One condition: the named axis must hold one of `values`. `Firing` carries
/// none, since the file writes it as a bare word.
#[derive(Clone, Debug)]
pub struct Condition {
    pub kind: CondKind,
    pub values: Vec<String>,
}

/// An anim a clause selects. `duration_ms` and `blend_ms` are the file's own
/// `duration` and `blendtime`, present only on event clauses.
#[derive(Clone, Debug)]
pub struct AnimRef {
    pub name: String,
    pub duration_ms: Option<u32>,
    pub blend_ms: Option<u32>,
}

/// A condition list and what it selects. An empty `conditions` is the file's
/// `default`, which always matches. A channel can list more than one anim
/// (death and melee blocks do); which one plays is a later task's `select`,
/// not this parser's — both vectors keep file order and nothing else.
#[derive(Clone, Debug, Default)]
pub struct Clause {
    pub conditions: Vec<Condition>,
    pub legs: Vec<AnimRef>,
    pub torso: Vec<AnimRef>,
}

/// A movetype block inside a state, or one event block. Clauses are in file
/// order, which is the order they are tested in: the file's own header says
/// first match wins.
#[derive(Clone, Debug)]
pub struct Block {
    pub name: String,
    pub clauses: Vec<Clause>,
}

/// A parsed `mp/playeranim.script`.
pub struct AnimScript {
    states: HashMap<String, Vec<Block>>,
    events: HashMap<String, Block>,
}

/// `set <kind> <alias> = <a> AND <b>`, keyed by the alias, since a value is
/// only ever looked up by name.
type Defines = HashMap<String, Vec<String>>;

impl AnimScript {
    /// Movetype blocks of a state, by lowercased name.
    pub fn state(&self, name: &str) -> Option<&[Block]> {
        self.states.get(name).map(Vec::as_slice)
    }

    /// One event block, by lowercased name.
    pub fn event(&self, name: &str) -> Option<&Block> {
        self.events.get(name)
    }

    /// Every anim name the script names, lowercased. The engine only keeps
    /// animtree nodes whose names are already interned, so this is what
    /// decides the index order (`AnimIndex::build`).
    pub fn referenced_names(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        for block in self.states.values().flatten().chain(self.events.values()) {
            for c in &block.clauses {
                out.extend(c.legs.iter().chain(&c.torso).map(|a| a.name.clone()));
            }
        }
        out
    }

    pub fn parse(text: &str) -> Result<AnimScript> {
        let text = strip_comments(text);
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .peekable();
        let mut defines = Defines::new();
        let mut states = HashMap::new();
        let mut events = HashMap::new();
        let mut section = "";
        while let Some(line) = lines.next() {
            match line {
                "DEFINES" | "ANIMATIONS" | "EVENTS" => {
                    section = match line {
                        "DEFINES" => "defines",
                        "ANIMATIONS" => "animations",
                        _ => "events",
                    };
                }
                _ if section == "defines" => {
                    if let Some(rest) = line.strip_prefix("set ") {
                        let (head, values) = rest
                            .split_once('=')
                            .map(|(h, v)| (h.trim(), v))
                            .unwrap_or((rest, ""));
                        // `set <kind> <alias>`: the kind is implied by where
                        // the alias is used, so only the name is kept.
                        if let Some((_, alias)) = head.split_once(char::is_whitespace) {
                            defines.insert(fold(alias.trim()), split_and(values));
                        }
                    }
                }
                _ if section == "animations" => {
                    if let Some(name) = line.strip_prefix("STATE ") {
                        let blocks = parse_state(&mut lines, &defines)?;
                        states.insert(fold(name.trim()), blocks);
                    }
                }
                _ if section == "events" => {
                    let name = fold(line);
                    let clauses = parse_block(&mut lines, &defines)?;
                    events.insert(name.clone(), Block { name, clauses });
                }
                _ => {}
            }
        }
        Ok(AnimScript { states, events })
    }
}

fn fold(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

/// `a AND b AND c`, lowercased. An empty right-hand side is an empty set.
fn split_and(values: &str) -> Vec<String> {
    values
        .split_whitespace()
        .filter(|w| *w != "AND")
        .map(fold)
        .collect()
}

/// Consumes the `{ ... }` after `STATE <name>`, one nested block per movetype.
fn parse_state<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    defines: &Defines,
) -> Result<Vec<Block>> {
    expect_open(lines)?;
    let mut out = Vec::new();
    loop {
        let Some(line) = lines.next() else {
            bail!("a STATE block that never closes");
        };
        if line == "}" {
            return Ok(out);
        }
        let name = fold(line);
        let clauses = parse_block(lines, defines)?;
        out.push(Block { name, clauses });
    }
}

/// Consumes the `{ ... }` after a block name, one clause per condition list.
fn parse_block<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    defines: &Defines,
) -> Result<Vec<Clause>> {
    expect_open(lines)?;
    let mut out = Vec::new();
    loop {
        let Some(line) = lines.next() else {
            bail!("a block that never closes");
        };
        if line == "}" {
            return Ok(out);
        }
        let mut clause = Clause {
            conditions: parse_conditions(line, defines),
            ..Clause::default()
        };
        expect_open(lines)?;
        loop {
            let Some(line) = lines.next() else {
                bail!("a clause that never closes");
            };
            if line == "}" {
                break;
            }
            apply_anim_line(line, &mut clause);
        }
        out.push(clause);
    }
}

fn expect_open<'a>(lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>) -> Result<()> {
    match lines.next() {
        Some("{") => Ok(()),
        Some(other) => bail!("expected `{{`, found {other:?}"),
        None => bail!("expected `{{`, found end of file"),
    }
}

/// `default` is the empty conjunction, which matches everything.
fn parse_conditions(line: &str, defines: &Defines) -> Vec<Condition> {
    if fold(line) == "default" {
        return Vec::new();
    }
    line.split(',')
        .filter_map(|part| {
            let part = part.trim();
            let (head, rest) = part.split_once(char::is_whitespace).unwrap_or((part, ""));
            let kind = CondKind::parse(&fold(head))?;
            let mut values = Vec::new();
            for v in split_and(rest) {
                match defines.get(&v) {
                    Some(expanded) => values.extend(expanded.iter().cloned()),
                    None => values.push(v),
                }
            }
            Some(Condition { kind, values })
        })
        .collect()
}

/// `both|legs|torso <name> [duration <n>] [blendtime <n>] [turretanim]`.
/// Anything else on the line is ignored, which is what makes the trailing
/// `turretanim` modifier harmless. A clause may list more than one line per
/// channel (death and melee do), so this pushes rather than overwrites.
fn apply_anim_line(line: &str, clause: &mut Clause) {
    let mut w = line.split_whitespace();
    let Some(part) = w.next() else { return };
    if !matches!(part, "both" | "legs" | "torso") {
        return;
    }
    let Some(name) = w.next() else { return };
    let mut anim = AnimRef {
        name: fold(name),
        duration_ms: None,
        blend_ms: None,
    };
    while let Some(key) = w.next() {
        let value = w.next().and_then(|v| v.parse::<u32>().ok());
        match key {
            "duration" => anim.duration_ms = value,
            "blendtime" => anim.blend_ms = value,
            _ => {}
        }
    }
    if part != "torso" {
        clause.legs.push(anim.clone());
    }
    if part != "legs" {
        clause.torso.push(anim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
DEFINES

set weaponclass autofire = mg AND smg
set movetype moving = walk AND run

ANIMATIONS

STATE ALERT
{
}

STATE COMBAT
{
	idle
	{
		weaponclass pistol, weapon_position ads
		{
			both pb_stand_ads_pistol
		}
		default // two handed rifle type weapon
		{
			both pb_stand_alert
		}
	}
	run
	{
		strafing left
		{
			both pb_combatrun_left_loop
		}
		default
		{
			both pb_combatrun_forward_loop
		}
	}
}

EVENTS

land
{
	movetype run
	{
		legs pb_runjump_land duration 100 blendtime 50
	}
	default
	{
		legs pb_standjump_land duration 100 blendtime 50
	}
}
"#;

    #[test]
    fn parses_states_blocks_and_clauses() {
        let s = AnimScript::parse(SAMPLE).unwrap();
        let combat = s.state("combat").expect("STATE COMBAT");
        assert_eq!(
            combat.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            ["idle", "run"]
        );
        // An empty state parses to an empty block list rather than to nothing.
        assert_eq!(s.state("alert").map(<[Block]>::len), Some(0));
        let idle = &combat[0];
        assert_eq!(idle.clauses.len(), 2);
        assert_eq!(
            idle.clauses[1].conditions.len(),
            0,
            "default has no conditions"
        );
        assert_eq!(
            idle.clauses[1]
                .legs
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["pb_stand_alert"]
        );
        assert_eq!(
            idle.clauses[1]
                .torso
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["pb_stand_alert"],
            "`both` sets legs and torso"
        );
    }

    #[test]
    fn a_condition_list_is_a_conjunction_of_any_of_sets() {
        let s = AnimScript::parse(SAMPLE).unwrap();
        let c = &s.state("combat").unwrap()[0].clauses[0];
        assert_eq!(c.conditions.len(), 2);
        assert_eq!(c.conditions[0].kind, CondKind::WeaponClass);
        assert_eq!(c.conditions[0].values, ["pistol"]);
        assert_eq!(c.conditions[1].kind, CondKind::WeaponPosition);
        assert_eq!(c.conditions[1].values, ["ads"]);
    }

    /// A DEFINES alias is expanded where it is used, so nothing downstream
    /// has to know the alias table exists.
    #[test]
    fn defines_expand_into_condition_values() {
        let text = SAMPLE.replace("weaponclass pistol,", "weaponclass autofire,");
        let s = AnimScript::parse(&text).unwrap();
        let c = &s.state("combat").unwrap()[0].clauses[0];
        assert_eq!(c.conditions[0].values, ["mg", "smg"]);
    }

    #[test]
    fn an_event_clause_keeps_its_duration_and_blendtime() {
        let s = AnimScript::parse(SAMPLE).unwrap();
        let land = s.event("land").expect("land");
        assert_eq!(
            land.clauses[0]
                .legs
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["pb_runjump_land"]
        );
        let legs = &land.clauses[0].legs[0];
        assert_eq!(legs.duration_ms, Some(100));
        assert_eq!(legs.blend_ms, Some(50));
        assert!(
            land.clauses[0].torso.is_empty(),
            "`legs` leaves torso alone"
        );
    }

    /// A clause may list more than one anim line per channel (death and melee
    /// blocks do); the parser keeps every one rather than only the last.
    #[test]
    fn a_clause_keeps_every_anim_line_in_a_channel() {
        let text = SAMPLE.replace(
            "\t\t\tboth pb_stand_alert\n",
            "\t\t\tboth pb_stand_alert\n\t\t\tboth pb_stand_alert2\n",
        );
        let s = AnimScript::parse(&text).unwrap();
        let idle = &s.state("combat").unwrap()[0];
        let names: Vec<_> = idle.clauses[1]
            .legs
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["pb_stand_alert", "pb_stand_alert2"]);
        let torso_names: Vec<_> = idle.clauses[1]
            .torso
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(torso_names, ["pb_stand_alert", "pb_stand_alert2"]);
        let referenced = s.referenced_names();
        assert!(referenced.contains("pb_stand_alert"));
        assert!(referenced.contains("pb_stand_alert2"));
    }

    /// Every anim name the script names, which is what decides the animtree
    /// index order. This replaces `animtree::anim_script_names`.
    #[test]
    fn referenced_names_are_lowercased_and_deduplicated() {
        let s = AnimScript::parse(SAMPLE).unwrap();
        let names = s.referenced_names();
        assert!(names.contains("pb_stand_alert"));
        assert!(names.contains("pb_runjump_land"));
        assert!(!names.contains("duration"));
    }

    #[test]
    fn a_block_that_never_closes_is_an_error() {
        assert!(AnimScript::parse("ANIMATIONS\nSTATE COMBAT\n{\n idle\n {\n").is_err());
    }
}
