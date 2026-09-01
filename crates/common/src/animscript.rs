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

/// The movetypes the machine derives, each named exactly as the script's
/// block keys spell it. `turnright` and `turnleft` are in the file and not
/// here: entering them needs a model of the body yaw lagging the view, which
/// this stage does not build.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Movetype {
    #[default]
    Idle,
    IdleCr,
    IdleProne,
    Walk,
    WalkBk,
    WalkCr,
    WalkCrBk,
    WalkProne,
    WalkProneBk,
    Run,
    RunBk,
    RunCr,
    RunCrBk,
    ClimbUp,
    ClimbDown,
}

impl Movetype {
    pub fn name(self) -> &'static str {
        match self {
            Movetype::Idle => "idle",
            Movetype::IdleCr => "idlecr",
            Movetype::IdleProne => "idleprone",
            Movetype::Walk => "walk",
            Movetype::WalkBk => "walkbk",
            Movetype::WalkCr => "walkcr",
            Movetype::WalkCrBk => "walkcrbk",
            Movetype::WalkProne => "walkprone",
            Movetype::WalkProneBk => "walkpronebk",
            Movetype::Run => "run",
            Movetype::RunBk => "runbk",
            Movetype::RunCr => "runcr",
            Movetype::RunCrBk => "runcrbk",
            Movetype::ClimbUp => "climbup",
            Movetype::ClimbDown => "climbdown",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// What the player is doing, in the script's own vocabulary. `weapon` and
/// `weapon_class` are lowercased; the file's names are.
#[derive(Clone, Debug, Default)]
pub struct Conditions {
    pub movetype: Movetype,
    pub weapon: String,
    pub weapon_class: String,
    /// `weapon_position ads`.
    pub ads: bool,
    pub strafing: Option<Side>,
    /// `mounted mg42`. Nothing mounts a turret yet, so this is always `None`.
    pub mounted: Option<String>,
    /// Stage 6 raises this.
    pub firing: bool,
}

impl Conditions {
    fn holds(&self, c: &Condition) -> bool {
        let any = |v: &Vec<String>, x: &str| v.iter().any(|s| s == x);
        match c.kind {
            // `weapons none` is how the file spells an unarmed player, which
            // is a weapon name no weapon has.
            CondKind::Weapons => any(&c.values, &self.weapon),
            CondKind::WeaponClass => any(&c.values, &self.weapon_class),
            CondKind::WeaponPosition => self.ads && any(&c.values, "ads"),
            CondKind::Strafing => self.strafing.is_some_and(|s| any(&c.values, s.name())),
            CondKind::Movetype => any(&c.values, self.movetype.name()),
            CondKind::Mounted => self.mounted.as_deref().is_some_and(|m| any(&c.values, m)),
            CondKind::Firing => self.firing,
        }
    }
}

/// What a clause selected. Either half may be unset: `legs pb_standjump_land`
/// leaves the torso on whatever the continuous state chose.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub legs: Option<AnimRef>,
    pub torso: Option<AnimRef>,
}

impl AnimScript {
    /// The clause a state's movetype block selects. First match wins, which
    /// is the file's own stated rule. Nothing selected is the honest answer
    /// for a state or movetype the file does not cover.
    pub fn select(&self, state: &str, c: &Conditions) -> Selection {
        let Some(blocks) = self.state(state) else {
            return Selection::default();
        };
        let want = c.movetype.name();
        let Some(block) = blocks.iter().find(|b| b.name == want) else {
            return Selection::default();
        };
        pick(block, c)
    }

    /// The same, for one of the `EVENTS` blocks.
    pub fn select_event(&self, event: &str, c: &Conditions) -> Selection {
        let Some(block) = self.event(event) else {
            return Selection::default();
        };
        pick(block, c)
    }
}

/// A clause may list several anim lines per channel (retail's death and
/// melee blocks randomise among them, `commands[rand() % numCommands]` in
/// bg_animation.c); every clause this stage reaches lists exactly one, so
/// taking the first is deterministic without being wrong yet.
fn pick(block: &Block, c: &Conditions) -> Selection {
    for clause in &block.clauses {
        if clause.conditions.iter().all(|cond| c.holds(cond)) {
            return Selection {
                legs: clause.legs.first().cloned(),
                torso: clause.torso.first().cloned(),
            };
        }
    }
    Selection::default()
}

/// The restart toggle, `ANIM_TOGGLEBIT`. Index is the low 9 bits;
/// docs/research/player-model-anim-system.md, "Animation indices".
const ANIM_TOGGLEBIT: i32 = 512;

/// One channel's live anim: the index, the toggle, and when an event anim
/// that owns the channel gives it back.
#[derive(Clone, Copy, Default)]
struct Channel {
    index: i32,
    toggle: bool,
    /// serverTime an event anim releases the channel at; `None` when the
    /// continuous state owns it.
    held_until_ms: Option<i32>,
}

impl Channel {
    fn wire(self) -> i32 {
        self.index | if self.toggle { ANIM_TOGGLEBIT } else { 0 }
    }

    /// Returns whether the channel took the anim.
    fn start(&mut self, index: i32, held_until_ms: Option<i32>) -> bool {
        if self.index == index && self.held_until_ms.is_none() && held_until_ms.is_none() {
            return false;
        }
        self.index = index;
        self.toggle = !self.toggle;
        self.held_until_ms = held_until_ms;
        true
    }

    fn released(&mut self, now_ms: i32) -> bool {
        match self.held_until_ms {
            Some(t) if now_ms.wrapping_sub(t) >= 0 => {
                self.held_until_ms = None;
                true
            }
            Some(_) => false,
            None => true,
        }
    }
}

/// One client's animation channels. The server owns one per client and reads
/// `legs()` / `torso()` into the playerstate every frame.
#[derive(Clone, Copy, Default)]
pub struct AnimState {
    legs: Channel,
    torso: Channel,
}

impl AnimState {
    /// The continuous state's choice. A channel an event anim still holds
    /// keeps it until the duration runs out.
    pub fn set(&mut self, sel: &Selection, now_ms: i32, resolve: impl Fn(&str) -> Option<i32>) {
        apply(&mut self.legs, sel.legs.as_ref(), now_ms, false, &resolve);
        apply(&mut self.torso, sel.torso.as_ref(), now_ms, false, &resolve);
    }

    /// An event's choice, which takes the channel for the clause's own
    /// `duration`. A clause with no duration behaves like the continuous
    /// state.
    pub fn event(&mut self, sel: &Selection, now_ms: i32, resolve: impl Fn(&str) -> Option<i32>) {
        apply(&mut self.legs, sel.legs.as_ref(), now_ms, true, &resolve);
        apply(&mut self.torso, sel.torso.as_ref(), now_ms, true, &resolve);
    }

    pub fn legs(&self) -> i32 {
        self.legs.wire()
    }

    pub fn torso(&self) -> i32 {
        self.torso.wire()
    }
}

fn apply(
    ch: &mut Channel,
    anim: Option<&AnimRef>,
    now_ms: i32,
    is_event: bool,
    resolve: &impl Fn(&str) -> Option<i32>,
) {
    if !ch.released(now_ms) && !is_event {
        return;
    }
    let Some(anim) = anim else { return };
    let Some(index) = resolve(&anim.name) else {
        return;
    };
    let held = if is_event {
        anim.duration_ms.map(|d| now_ms.wrapping_add(d as i32))
    } else {
        None
    };
    ch.start(index, held);
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

    fn rifleman(movetype: Movetype) -> Conditions {
        Conditions {
            movetype,
            weapon: "m1carbine_mp".into(),
            weapon_class: "rifle".into(),
            ..Conditions::default()
        }
    }

    #[test]
    fn selection_takes_the_first_matching_clause() {
        let s = AnimScript::parse(SAMPLE).unwrap();
        let sel = s.select("combat", &rifleman(Movetype::Idle));
        assert_eq!(
            sel.legs.map(|a| a.name),
            Some("pb_stand_alert".to_string()),
            "a rifleman falls through to the default clause"
        );

        let mut ads_pistol = rifleman(Movetype::Idle);
        ads_pistol.weapon_class = "pistol".into();
        ads_pistol.ads = true;
        assert_eq!(
            s.select("combat", &ads_pistol).legs.map(|a| a.name),
            Some("pb_stand_ads_pistol".to_string())
        );

        // The same pistol, hip-fired: the ads clause no longer holds, so the
        // default does.
        let mut hip_pistol = ads_pistol.clone();
        hip_pistol.ads = false;
        assert_eq!(
            s.select("combat", &hip_pistol).legs.map(|a| a.name),
            Some("pb_stand_alert".to_string())
        );
    }

    #[test]
    fn strafing_picks_the_side_clause() {
        let s = AnimScript::parse(SAMPLE).unwrap();
        let mut c = rifleman(Movetype::Run);
        assert_eq!(
            s.select("combat", &c).legs.map(|a| a.name),
            Some("pb_combatrun_forward_loop".to_string())
        );
        c.strafing = Some(Side::Left);
        assert_eq!(
            s.select("combat", &c).legs.map(|a| a.name),
            Some("pb_combatrun_left_loop".to_string())
        );
    }

    #[test]
    fn an_event_selects_from_its_own_block() {
        let s = AnimScript::parse(SAMPLE).unwrap();
        let sel = s.select_event("land", &rifleman(Movetype::Run));
        let legs = sel.legs.expect("land sets legs");
        assert_eq!(legs.name, "pb_runjump_land");
        assert_eq!(legs.duration_ms, Some(100));
        assert!(sel.torso.is_none());
    }

    #[test]
    fn a_block_or_state_that_does_not_exist_selects_nothing() {
        let s = AnimScript::parse(SAMPLE).unwrap();
        assert!(s
            .select("relaxed", &rifleman(Movetype::Idle))
            .legs
            .is_none());
        assert!(s
            .select("combat", &rifleman(Movetype::ClimbUp))
            .legs
            .is_none());
        assert!(s
            .select_event("fireweapon", &rifleman(Movetype::Idle))
            .legs
            .is_none());
    }

    /// The two values the retail capture pins, read out of the shipped file.
    /// `mp_carentan-dm-motion.txt` has `legsAnim` 634 standing (index 122,
    /// `pb_stand_alert`, toggle set) and 94 running
    /// (`pb_combatrun_forward_loop`). Needs the paks.
    #[test]
    fn the_real_script_picks_what_retail_played() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = crate::animtree::PlayerAnims::load(&fs).unwrap();
        let idle = anims
            .script
            .select("combat", &rifleman(Movetype::Idle))
            .legs
            .expect("an idle anim");
        assert_eq!(idle.name, "pb_stand_alert");
        assert_eq!(anims.wire_of(&idle.name), Some(122));

        let run = anims
            .script
            .select("combat", &rifleman(Movetype::Run))
            .legs
            .expect("a run anim");
        assert_eq!(run.name, "pb_combatrun_forward_loop");
        assert_eq!(anims.wire_of(&run.name), Some(94));

        let crouch = anims
            .script
            .select("combat", &rifleman(Movetype::IdleCr))
            .legs
            .expect("a crouched idle anim");
        assert_eq!(crouch.name, "pb_crouch_alert");
        assert_eq!(anims.wire_of(&crouch.name), Some(111));
    }

    fn anim(name: &str, duration_ms: Option<u32>) -> AnimRef {
        AnimRef {
            name: name.into(),
            duration_ms,
            blend_ms: None,
        }
    }

    /// Two names, so a test can resolve without an animtree.
    fn resolve(name: &str) -> Option<i32> {
        match name {
            "pb_stand_alert" => Some(122),
            "pb_combatrun_forward_loop" => Some(94),
            "pb_standjump_land" => Some(200),
            _ => None,
        }
    }

    #[test]
    fn a_changed_anim_flips_the_toggle_and_the_same_one_does_not() {
        let mut st = AnimState::default();
        let stand = Selection {
            legs: Some(anim("pb_stand_alert", None)),
            torso: Some(anim("pb_stand_alert", None)),
        };
        st.set(&stand, 0, resolve);
        let first = st.legs();
        assert_eq!(first & 511, 122);

        // Held: the same index, the same toggle, so the client keeps phase.
        st.set(&stand, 50, resolve);
        assert_eq!(st.legs(), first);

        let run = Selection {
            legs: Some(anim("pb_combatrun_forward_loop", None)),
            torso: Some(anim("pb_stand_alert", None)),
        };
        st.set(&run, 100, resolve);
        assert_eq!(st.legs() & 511, 94);
        assert_ne!(st.legs() & 512, first & 512, "a change flips the toggle");
        assert_eq!(st.torso(), first, "the torso did not change");
    }

    /// An event anim holds for its own duration and then lets the continuous
    /// state take the channel back.
    #[test]
    fn an_event_anim_expires() {
        let mut st = AnimState::default();
        let stand = Selection {
            legs: Some(anim("pb_stand_alert", None)),
            torso: None,
        };
        st.set(&stand, 0, resolve);
        st.event(
            &Selection {
                legs: Some(anim("pb_standjump_land", Some(100))),
                torso: None,
            },
            0,
            resolve,
        );
        assert_eq!(st.legs() & 511, 200);

        // Still inside the duration: the continuous state cannot take it.
        st.set(&stand, 50, resolve);
        assert_eq!(st.legs() & 511, 200);

        st.set(&stand, 100, resolve);
        assert_eq!(st.legs() & 511, 122, "the event expired");
    }

    /// A name the tree does not index leaves the channel where it was, rather
    /// than sending an index that plays the wrong clip.
    #[test]
    fn an_unresolvable_name_changes_nothing() {
        let mut st = AnimState::default();
        st.set(
            &Selection {
                legs: Some(anim("pb_stand_alert", None)),
                torso: None,
            },
            0,
            resolve,
        );
        let before = st.legs();
        st.set(
            &Selection {
                legs: Some(anim("pb_not_in_the_tree", None)),
                torso: None,
            },
            50,
            resolve,
        );
        assert_eq!(st.legs(), before);
    }
}
