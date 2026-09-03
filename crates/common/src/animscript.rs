//! `mp/playeranim.script`: which animation a player plays, given its state.
//!
//! RTCW's character animation script format verbatim, ported from
//! `bg_animation.c` in the RTCW-MP sources (GPLv3), the same lineage as the
//! netcode. The file it reads and the index its names resolve to are in
//! docs/research/player-model-anim-system.md.

use crate::animtree::strip_comments;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};

/// Which condition a clause tests. `leaning`, `position`, `underwater` and
/// `underhand` are in the format's legend and in no clause the shipped
/// multiplayer script carries, so they have no variant; anything else the file
/// names becomes `Unknown`, which never holds, so a clause the machine cannot
/// evaluate never matches instead of matching unconditionally. The shipped
/// script reaches that path nowhere -- its `enemy_weapon` and `impact_point`
/// clauses are all commented out -- so `Unknown` is what a downloaded pak or a
/// later revision lands in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CondKind {
    Weapons,
    WeaponClass,
    WeaponPosition,
    Strafing,
    Movetype,
    Mounted,
    Firing,
    Unknown(String),
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

    /// The kind, or `Unknown` carrying the spelling the file used.
    fn of(word: &str) -> CondKind {
        CondKind::parse(word).unwrap_or_else(|| CondKind::Unknown(word.to_string()))
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
/// (death, pain and melee blocks do); which one plays is the selection's
/// business ([`AnimScript::select_event_random`]), not this parser's — both
/// vectors keep file order and nothing else.
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

    /// Every anim name an `EVENTS` block names. These are the ones whose own
    /// length matters: an event clause without a `duration` holds its channel
    /// for the clip's length ([`AnimState::event`]).
    pub fn event_referenced_names(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        for block in self.events.values() {
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
        .map(|part| {
            let part = part.trim();
            let (head, rest) = part.split_once(char::is_whitespace).unwrap_or((part, ""));
            let kind = CondKind::of(&fold(head));
            let mut values = Vec::new();
            for v in split_and(rest) {
                match defines.get(&v) {
                    Some(expanded) => values.extend(expanded.iter().cloned()),
                    None => values.push(v),
                }
            }
            Condition { kind, values }
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
    /// `ps.weaponstate` reads firing. The shipped file's only `firing` clauses
    /// are `mounted mg42, firing`, so nothing decides on it until a turret
    /// does.
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
            // A condition nothing evaluates cannot be satisfied. Dropping it
            // instead would widen the clause to everything it does not gate.
            CondKind::Unknown(_) => false,
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

    /// The same, for one of the `EVENTS` blocks. The events that reach it --
    /// `fireweapon`, `reload`, `dropweapon`, `raiseweapon`, `jump`, `jumpbk`,
    /// `land` -- list one anim per channel per clause in the shipped file, so
    /// the first line is the whole answer.
    pub fn select_event(&self, event: &str, c: &Conditions) -> Selection {
        let Some(block) = self.event(event) else {
            return Selection::default();
        };
        pick(block, c)
    }

    /// The same, drawing among the clause's lines instead of taking the
    /// first: `death`, `pain` and `meleeattack` list several. One draw per
    /// clause, not one per channel, so a clause of `both` lines keeps its two
    /// channels on the same line.
    pub fn select_event_random(&self, event: &str, c: &Conditions, rng: &mut u64) -> Selection {
        let Some(clause) = self.event(event).and_then(|b| matching_clause(b, c)) else {
            return Selection::default();
        };
        let draw = crate::rng::xorshift(rng) as usize;
        let nth = |v: &[AnimRef]| v.get(draw % v.len().max(1)).cloned();
        Selection {
            legs: nth(&clause.legs),
            torso: nth(&clause.torso),
        }
    }

    /// The legs anims of the clause an event selects, in file order: what
    /// [`AnimScript::select_event_random`] draws from. For tests and tools.
    pub fn event_clause_anims(&self, event: &str, c: &Conditions) -> Vec<String> {
        self.event(event)
            .and_then(|b| matching_clause(b, c))
            .map(|clause| clause.legs.iter().map(|a| a.name.clone()).collect())
            .unwrap_or_default()
    }
}

/// First match wins, which is the file's own stated rule.
fn matching_clause<'a>(block: &'a Block, c: &Conditions) -> Option<&'a Clause> {
    block
        .clauses
        .iter()
        .find(|clause| clause.conditions.iter().all(|cond| c.holds(cond)))
}

/// The first line of the matching clause. A clause listing several
/// (retail's `DEATH`, `pain` and `meleeattack`) is drawn from by
/// [`AnimScript::select_event_random`] instead.
fn pick(block: &Block, c: &Conditions) -> Selection {
    matching_clause(block, c).map_or_else(Selection::default, |clause| Selection {
        legs: clause.legs.first().cloned(),
        torso: clause.torso.first().cloned(),
    })
}

/// The restart toggle, `ANIM_TOGGLEBIT`. Index is the low 9 bits;
/// docs/research/player-model-anim-system.md, "Animation indices".
const ANIM_TOGGLEBIT: i32 = 512;

/// What an event anim's hold gets on top of the clip's own length, RTCW's
/// `duration + 50 // account for lerping between anims` (`bg_animation.c`,
/// `BG_PlayAnim`). The four measured clears in the combat captures land
/// within one snapshot of it (player-model-anim-system.md, "The weapon
/// channel").
const LERP_MS: u32 = 50;

/// The floor under a derived hold. A clause with no `duration` over a clip
/// shorter than this holds for this instead, which is what the only putaway
/// measurement there is says: `pt_stand_pullout_pose` is a single frame, 0 ms
/// long, and the torso held it for about 500 ms
/// (player-model-anim-system.md, "The weapon channel"). It is a floor and not
/// `max(clip, state length)`: the carbine's reload clip is 1000 ms inside a
/// 2650 ms state and the capture clears at the clip, so the state's length
/// cannot be what holds a channel. A clause's own `duration` never takes it.
const MIN_EVENT_HOLD_MS: u32 = 500;

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
        apply(&mut self.legs, sel.legs.as_ref(), now_ms, &resolve);
        apply(&mut self.torso, sel.torso.as_ref(), now_ms, &resolve);
    }

    /// An event's choice, which takes the channel for the clause's own
    /// `duration`, or for the clip's own length plus [`LERP_MS`] when the
    /// clause carries none. `length` is that clip length in ms, which only the
    /// animtree knows.
    pub fn event(
        &mut self,
        sel: &Selection,
        now_ms: i32,
        resolve: impl Fn(&str) -> Option<i32>,
        length: impl Fn(&str) -> Option<u32>,
    ) {
        let hold = |a: &AnimRef| {
            a.duration_ms
                .or_else(|| length(&a.name).map(|d| (d + LERP_MS).max(MIN_EVENT_HOLD_MS)))
        };
        apply_event(&mut self.legs, sel.legs.as_ref(), now_ms, &resolve, &hold);
        apply_event(&mut self.torso, sel.torso.as_ref(), now_ms, &resolve, &hold);
    }

    /// The continuous torso choice, which every settled retail pose reads as
    /// index 0 (docs/research/player-model-anim-system.md, "The weapon
    /// channel"): an event anim that ran out gives the channel back to no
    /// anim at all, flipping the toggle doing it.
    pub fn clear_torso(&mut self, now_ms: i32) {
        if self.torso.released(now_ms) && self.torso.index != 0 {
            self.torso.start(0, None);
        }
    }

    /// The torso restarted on no anim at all, toggle flipped whatever it
    /// held: what a death leaves the channel reading (512 beside the death
    /// `legsAnim` in both retail deaths, cod11-combat.md section 8).
    pub fn restart_torso_empty(&mut self) {
        self.torso = Channel {
            index: 0,
            toggle: !self.torso.toggle,
            held_until_ms: None,
        };
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
    resolve: &impl Fn(&str) -> Option<i32>,
) {
    if !ch.released(now_ms) {
        return;
    }
    let Some(anim) = anim else { return };
    let Some(index) = resolve(&anim.name) else {
        return;
    };
    ch.start(index, None);
}

/// An event anim takes the channel whatever is on it, and holds it for `hold`.
/// A hold is what makes a repeated shot flip the restart toggle: the index is
/// unchanged, so nothing else would tell the client to play it again.
fn apply_event(
    ch: &mut Channel,
    anim: Option<&AnimRef>,
    now_ms: i32,
    resolve: &impl Fn(&str) -> Option<i32>,
    hold: &impl Fn(&AnimRef) -> Option<u32>,
) {
    let Some(anim) = anim else { return };
    let Some(index) = resolve(&anim.name) else {
        return;
    };
    let held = hold(anim).map(|d| now_ms.wrapping_add(d as i32));
    ch.start(index, held);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

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

    /// The `EVENTS` clauses gate on movetype *categories* -- `crouching`,
    /// `prone`, `moving` -- which the file itself defines as `set movetype`
    /// aliases over the movetype names the machine produces. So a category
    /// needs no separate condition kind: the alias expands where it is used.
    #[test]
    fn a_movetype_category_holds_for_every_movetype_it_names() {
        let text = SAMPLE.replace("movetype run\n", "movetype moving\n");
        let s = AnimScript::parse(&text).unwrap();
        for (movetype, want) in [
            (Movetype::Run, "pb_runjump_land"),
            (Movetype::Walk, "pb_runjump_land"),
            (Movetype::Idle, "pb_standjump_land"),
        ] {
            assert_eq!(
                s.select_event("land", &rifleman(movetype))
                    .legs
                    .map(|a| a.name),
                Some(want.to_string()),
                "{movetype:?}"
            );
        }
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

    /// Every torso index the two retail combat captures read, back out of the
    /// shipped script's own `EVENTS` clauses. This is what says the weapon
    /// channel needs no table of its own
    /// (docs/research/player-model-anim-system.md, "The weapon channel").
    /// Needs the paks.
    #[test]
    fn the_event_blocks_pick_the_indices_the_combat_captures_read() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = crate::animtree::PlayerAnims::load(&fs).unwrap();
        let mosin = |movetype| Conditions {
            weapon: "mosin_nagant_mp".into(),
            ..rifleman(movetype)
        };
        let cases = [
            (
                "fireweapon",
                rifleman(Movetype::Idle),
                "pt_stand_shoot",
                253,
            ),
            (
                "fireweapon",
                rifleman(Movetype::IdleCr),
                "pt_crouch_shoot",
                249,
            ),
            (
                "fireweapon",
                rifleman(Movetype::IdleProne),
                "pt_prone_shoot",
                224,
            ),
            ("fireweapon", mosin(Movetype::Idle), "pt_stand_shoot", 253),
            (
                "reload",
                rifleman(Movetype::Idle),
                "pt_reload_stand_auto",
                229,
            ),
            (
                "reload",
                mosin(Movetype::Idle),
                "pt_reload_stand_rifle",
                231,
            ),
        ];
        for (event, c, name, wire) in cases {
            let torso = anims
                .script
                .select_event(event, &c)
                .torso
                .unwrap_or_else(|| panic!("{event} selects a torso anim for {:?}", c.movetype));
            assert_eq!(torso.name, name, "{event} {:?} {}", c.movetype, c.weapon);
            assert_eq!(anims.wire_of(&torso.name), Some(wire));
            assert!(
                torso.duration_ms.is_none(),
                "{name} carries no clause duration, so its hold is the clip's"
            );
            assert!(
                anims.length_ms(&torso.name).is_some(),
                "{name} has an xanim to take that length from"
            );
        }
        // The file answers a moving shot with an empty clause: nothing plays.
        assert!(anims
            .script
            .select_event("fireweapon", &rifleman(Movetype::Run))
            .torso
            .is_none());
    }

    /// The shipped `death` clauses list several anims each; retail draws one
    /// of them (`commands[rand() % numCommands]`, RTCW's `bg_animation.c`).
    /// Over many draws every listed anim comes up and nothing else does, and
    /// a `both` line keeps the two channels on the same anim. Needs the paks.
    #[test]
    fn a_multi_line_death_clause_is_drawn_at_random() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = crate::animtree::PlayerAnims::load(&fs).unwrap();
        let c = rifleman(Movetype::Idle);
        let listed = anims.script.event_clause_anims("death", &c);
        assert!(
            listed.len() > 1,
            "the shipped death clause lists {} anims",
            listed.len()
        );
        let mut rng = 7u64;
        let mut seen = BTreeSet::new();
        for _ in 0..200 {
            let sel = anims.script.select_event_random("death", &c, &mut rng);
            let legs = sel.legs.expect("death selects a legs anim").name;
            assert_eq!(
                sel.torso.map(|a| a.name).as_deref(),
                Some(legs.as_str()),
                "a `both` line takes both channels"
            );
            assert!(listed.contains(&legs), "{legs} is not in the clause");
            seen.insert(legs);
        }
        assert_eq!(seen.len(), listed.len(), "every listed anim was drawn");
    }

    /// `pain`'s prone clause is the only other multi-line one; the clauses a
    /// standing or crouched player reaches list one anim, so those draws are
    /// the same anim every time. Needs the paks.
    #[test]
    fn a_prone_pain_draws_from_two_anims_and_a_standing_one_from_one() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = crate::animtree::PlayerAnims::load(&fs).unwrap();
        let prone = rifleman(Movetype::IdleProne);
        assert_eq!(
            anims.script.event_clause_anims("pain", &prone),
            ["pb_prone_paina_holdchest", "pb_prone_painb_holdhead"]
        );
        let mut rng = 11u64;
        let mut seen = BTreeSet::new();
        for _ in 0..100 {
            let sel = anims.script.select_event_random("pain", &prone, &mut rng);
            seen.insert(sel.legs.expect("pain selects a legs anim").name);
        }
        assert_eq!(seen.len(), 2, "both prone pain anims were drawn");

        let stand = rifleman(Movetype::Idle);
        assert_eq!(
            anims.script.event_clause_anims("pain", &stand),
            ["pb_crouch_pain_holdstomach"]
        );
        let drawn = anims
            .script
            .select_event_random("pain", &stand, &mut rng)
            .legs
            .expect("pain selects a legs anim")
            .name;
        assert_eq!(drawn, "pb_crouch_pain_holdstomach");
    }

    /// What lets `select_event` take the first line and still be the whole
    /// answer: every clause of the events it serves lists one anim per
    /// channel. `death`, `pain` and `meleeattack` are the three blocks that
    /// list more, and they go through `select_event_random`. Needs the paks.
    #[test]
    fn only_the_random_events_list_more_than_one_anim_per_clause() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = crate::animtree::PlayerAnims::load(&fs).unwrap();
        for event in [
            "fireweapon",
            "reload",
            "dropweapon",
            "raiseweapon",
            "jump",
            "jumpbk",
            "land",
        ] {
            let block = anims.script.event(event).expect("a shipped event block");
            for clause in &block.clauses {
                assert!(
                    clause.legs.len() <= 1 && clause.torso.len() <= 1,
                    "{event} has a clause of {} legs and {} torso anims",
                    clause.legs.len(),
                    clause.torso.len()
                );
            }
        }
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
            "pt_stand_shoot" => Some(253),
            "pt_reload_stand_auto" => Some(229),
            _ => None,
        }
    }

    /// The clip lengths those names would have, so a test can hold an event
    /// anim without an animtree. The two the combat captures measured: 13 and
    /// 31 frames at 30 fps.
    fn length(name: &str) -> Option<u32> {
        match name {
            "pt_stand_shoot" => Some(400),
            "pt_reload_stand_auto" => Some(1000),
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
            length,
        );
        assert_eq!(st.legs() & 511, 200);

        // Still inside the duration: the continuous state cannot take it.
        st.set(&stand, 50, resolve);
        assert_eq!(st.legs() & 511, 200);

        st.set(&stand, 100, resolve);
        assert_eq!(st.legs() & 511, 122, "the event expired");
    }

    /// What carentan's `sustained_fire` step reads: six shots, one index, the
    /// toggle flipping on every one of them, and the channel clearing to index
    /// 0 with one more flip when the last one's clip runs out.
    #[test]
    fn a_repeated_event_anim_restarts_and_then_clears_the_torso() {
        let mut st = AnimState::default();
        let shot = Selection {
            legs: None,
            torso: Some(anim("pt_stand_shoot", None)),
        };
        // The sample the step opens on, before the first shot: the capture
        // carries one and the flip into the first shot is only visible
        // against it.
        let mut wire = vec![st.torso()];
        // Six shots 200 ms apart, near the capture's own 183 ms tap period,
        // sampled every 50 ms the way its snapshots sampled it.
        for ms in (0..2500).step_by(50) {
            if ms < 1200 && ms % 200 == 0 {
                st.event(&shot, ms, resolve, length);
            }
            st.clear_torso(ms);
            wire.push(st.torso());
        }
        let indices: BTreeSet<i32> = wire.iter().map(|v| v & 511).collect();
        assert_eq!(indices, BTreeSet::from([0, 253]));
        let flips = wire.windows(2).filter(|w| (w[0] ^ w[1]) & 512 != 0).count();
        assert_eq!(flips, 7, "one flip per shot, plus the clear");
        assert_eq!(*wire.last().unwrap() & 511, 0, "the channel ends cleared");
    }

    /// A clause with no `duration` holds for the clip's own length plus the
    /// lerp allowance, floored at [`MIN_EVENT_HOLD_MS`], and nothing takes the
    /// channel before that.
    #[test]
    fn an_event_clause_without_a_duration_holds_for_the_clip_length() {
        // 400 + 50 is under the floor, so the shot takes the floor.
        let mut st = AnimState::default();
        let shot = Selection {
            legs: None,
            torso: Some(anim("pt_stand_shoot", None)),
        };
        st.event(&shot, 0, resolve, length);
        st.clear_torso(499);
        assert_eq!(st.torso() & 511, 253, "still inside the floor");
        st.clear_torso(500);
        assert_eq!(st.torso() & 511, 0);

        // 1000 + 50 is over it, so the reload holds for its own clip: the
        // floor must not shorten a long anim, and the 2650 ms state it runs
        // inside must not lengthen it.
        let mut st = AnimState::default();
        let reload = Selection {
            legs: None,
            torso: Some(anim("pt_reload_stand_auto", None)),
        };
        st.event(&reload, 0, resolve, length);
        st.clear_torso(1049);
        assert_eq!(st.torso() & 511, 229);
        st.clear_torso(1050);
        assert_eq!(st.torso() & 511, 0);
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

    /// A condition the machine has no kind for makes its clause unselectable.
    /// Dropping the condition instead would make the clause match everything
    /// it was written to gate: the shipped `DEATH` and `pain` blocks test
    /// `enemy_weapon` and `impact_point`, and reading those as unconditional
    /// would hand every death the first listed clause.
    #[test]
    fn a_clause_with_an_unknown_condition_never_matches() {
        let script = r#"
ANIMATIONS

STATE COMBAT
{
	idle
	{
		enemy_weapon kar98k_mp
		{
			both pb_never
		}
		default
		{
			both pb_stand_alert
		}
	}
}
"#;
        let s = AnimScript::parse(script).unwrap();
        let idle = &s.state("combat").unwrap()[0];
        assert_eq!(
            idle.clauses[0].conditions[0].kind,
            CondKind::Unknown("enemy_weapon".into()),
            "the kind is kept, with the spelling the file used"
        );
        assert_eq!(
            s.select("combat", &rifleman(Movetype::Idle))
                .legs
                .map(|a| a.name),
            Some("pb_stand_alert".to_string()),
            "the unknown clause is skipped, not taken"
        );
    }

    /// The shipped script names no condition this parser has no kind for, so
    /// nothing in it is silently unselectable. `enemy_weapon` and
    /// `impact_point` are the two the legend lists and the `DEATH` and `pain`
    /// blocks would use, and every clause carrying them is commented out
    /// (script lines 1032-1094 and 1116-1167). A name appearing here later is
    /// a clause some stage will never reach. Needs the paks.
    #[test]
    fn the_shipped_script_names_no_condition_we_cannot_evaluate() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let text = fs
            .read("mp/playeranim.script")
            .expect("mp/playeranim.script in the paks");
        let s = AnimScript::parse(&String::from_utf8_lossy(&text)).unwrap();
        let mut unknown: Vec<String> = s
            .states
            .values()
            .flatten()
            .chain(s.events.values())
            .flat_map(|b| &b.clauses)
            .flat_map(|c| &c.conditions)
            .filter_map(|c| match &c.kind {
                CondKind::Unknown(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        unknown.sort();
        unknown.dedup();
        assert!(unknown.is_empty(), "unmodelled conditions: {unknown:?}");
    }
}
