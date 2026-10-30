//! A menu expression's value (`exp text(...)`, `visible when(...)`), as
//! 1.7's evaluator (`iw3mp.exe` 0x53f640) computes it: the statement's
//! entries in source order onto an operator stack ranked by
//! `s_operatorPrecedence` (0x7245c0) and a stack of operand lists, which a
//! comma joins and a call takes whole (docs/research/cod4-hud.md 12.2).
//! What the dvars, local variables, stats, team and string tables hold
//! comes from an [`Env`].

use crate::assets::ui::ExpEntry;

/// An operand: `expDataType` 0 int, 1 float, 2 string.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i32),
    Float(f32),
    Str(String),
}

impl Value {
    /// As an int: a float truncates, a string reads as `atoi`.
    pub fn int(&self) -> i32 {
        match self {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i32,
            Value::Str(s) => atoi(s),
        }
    }

    /// As a float: a string reads as `atof`.
    pub fn float(&self) -> f32 {
        match self {
            Value::Int(i) => *i as f32,
            Value::Float(f) => *f,
            Value::Str(s) => atof(s) as f32,
        }
    }

    /// As text: an int as `%i`, a float as `%f`.
    pub fn string(&self) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format!("{f:.6}"),
            Value::Str(s) => s.clone(),
        }
    }

    /// True when the value as an int is not 0.
    pub fn truth(&self) -> bool {
        self.int() != 0
    }
}

/// The game state the cgame calls of the expressions read (`flashbanged()`,
/// `inKillcam()`, `player( ... )`, `team( score )`, `timeLeft()` and
/// kin), as the client last saw it; all 0 off a game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameView {
    /// `CG_Flashbanged`, `CG_ScopeIsOverlayed`,
    /// `CG_IsScoreboardDisplayed`, `cg.inKillCam`,
    /// `CG_IsSelectingLocation`, `CG_JavelinADS`.
    pub flashbanged: bool,
    pub scoped: bool,
    pub scoreboard: bool,
    pub killcam: bool,
    pub selecting_location: bool,
    pub ads_javelin: bool,
    /// `player( dead )`, `( clipAmmo )`, `( nightvision )`.
    pub dead: bool,
    pub clip_ammo: i32,
    pub night_vision: bool,
    /// The local player's score row, `None` before a `score` reply lists
    /// him (`UI_GetOurClientScore`).
    pub own: Option<ScoreRow>,
    /// The scores in rank order, for `scoreAtRank( n )`.
    pub ranked: Vec<i32>,
    /// `cg.teamScores` by `team_t`.
    pub team_scores: [i32; 4],
    /// `(cgs.gameEndTime - nextSnap.serverTime) / 1000`, `None` with no
    /// snapshot.
    pub time_left: Option<i32>,
    /// `predictedPlayerState.otherFlags & 2`.
    pub following: bool,
    /// `pm_type == PM_INTERMISSION`.
    pub intermission: bool,
    /// `CG_ActionSlotIsUsable` for slots 1..4.
    pub action_slots: [bool; 4],
    /// `CG_GetHudAlphaDPad`, `AmmoCounter`, `Compass`.
    pub fade_dpad: f32,
    pub fade_weapon: f32,
    pub fade_compass: f32,
    /// `Con_IsGameMessageWindowActive` by window.
    pub message_windows: [bool; 4],
}

/// `score_t`'s fields `player( ... )` reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScoreRow {
    pub score: i32,
    pub ping: i32,
    pub deaths: i32,
    pub kills: i32,
}

/// What an expression reads.
pub trait Env {
    /// A dvar's value as a string, `None` when it does not exist.
    fn dvar(&self, name: &str) -> Option<String>;
    /// A UI local variable (`setLocalVarInt` and kin).
    fn local(&self, name: &str) -> Option<Value>;
    /// `stat(n)`: the profile's stat `n`.
    fn stat(&self, index: i32) -> i32;
    /// The local player's `team_t`: 0 free, 1 axis, 2 allies, 3
    /// spectator.
    fn team(&self) -> i32;
    /// `StringTable_Lookup(table, column, value, ret)`.
    fn table_lookup(&self, table: &str, column: i32, value: &str, ret: i32) -> String;
    /// `milliseconds()`.
    fn millis(&self) -> i32 {
        0
    }
    /// `gametype()`: the level's `g_gametype` (the internal game type).
    fn gametype(&self) -> String {
        String::new()
    }
    /// `gameTypeName()`: the game type's display name, translated
    /// (`UI_GetGameTypeDisplayName`).
    fn gametype_name(&self) -> String {
        String::new()
    }
    /// `gameTypeDescription()`: `cg_objectiveText` as the cgame
    /// localizes it (`CG_GetGametypeDescription`).
    fn gametype_description(&self) -> String {
        String::new()
    }
    /// The cgame's state; `None` reads every call as 0.
    fn game(&self) -> Option<&GameView> {
        None
    }
    /// `keyBinding( cmd )`: the first key bound to `cmd` as
    /// `UI_GetKeyBindingLocalizedStringSingle` names it, `KEY_UNBOUND`'s
    /// text for none.
    fn key_binding(&self, _cmd: &str) -> String {
        String::new()
    }
    /// `SEH_LocalizeTextMessage`: a `\x14`-joined list of reference keys,
    /// `\x15` before a plain token, translated.
    fn localize(&self, text: &str) -> String {
        text.to_string()
    }
    /// `menuIsOpen( name )`.
    fn menu_open(&self, _name: &str) -> bool {
        false
    }
    /// `uiActive()`: a UI menu is up. A script menu draws only while one
    /// is, so the default holds; the HUD's menus (`compass_old`,
    /// `weaponinfo`, `dpad`) hide while it does.
    fn ui_active(&self) -> bool {
        true
    }
}

/// An operator or entry vcod does not evaluate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported(pub String);

/// `operationEnum`, named after `g_expOperatorNames` (`iw3mp.exe`
/// 0x723f38, cod4-hud.md 12.2). Operators run to `>>`; every number from
/// `sin` on is a call.
mod op {
    pub const NOOP: i32 = 0;
    pub const CLOSE: i32 = 1;
    pub const MUL: i32 = 2;
    pub const DIV: i32 = 3;
    pub const MOD: i32 = 4;
    pub const ADD: i32 = 5;
    pub const SUB: i32 = 6;
    pub const NOT: i32 = 7;
    pub const LT: i32 = 8;
    pub const LE: i32 = 9;
    pub const GT: i32 = 10;
    pub const GE: i32 = 11;
    pub const EQ: i32 = 12;
    pub const NE: i32 = 13;
    pub const AND: i32 = 14;
    pub const OR: i32 = 15;
    pub const OPEN: i32 = 16;
    pub const COMMA: i32 = 17;
    pub const BIT_AND: i32 = 18;
    pub const BIT_OR: i32 = 19;
    pub const BIT_NOT: i32 = 20;
    pub const SHL: i32 = 21;
    pub const SHR: i32 = 22;
    pub const SIN: i32 = 0x17;
    pub const COS: i32 = 0x18;
    pub const MIN: i32 = 0x19;
    pub const MAX: i32 = 0x1a;
    pub const MILLISECONDS: i32 = 0x1b;
    pub const DVAR_INT: i32 = 0x1c;
    pub const DVAR_BOOL: i32 = 0x1d;
    pub const DVAR_FLOAT: i32 = 0x1e;
    pub const DVAR_STRING: i32 = 0x1f;
    pub const STAT: i32 = 0x20;
    pub const UI_ACTIVE: i32 = 0x21;
    pub const FLASHBANGED: i32 = 0x22;
    pub const SCOPED: i32 = 0x23;
    pub const SCOREBOARD_VISIBLE: i32 = 0x24;
    pub const IN_KILLCAM: i32 = 0x25;
    pub const PLAYER: i32 = 0x26;
    pub const SELECTING_LOCATION: i32 = 0x27;
    pub const TEAM: i32 = 0x28;
    pub const OTHER_TEAM: i32 = 0x29;
    pub const MARINES_FIELD: i32 = 0x2a;
    pub const OPFOR_FIELD: i32 = 0x2b;
    pub const MENU_IS_OPEN: i32 = 0x2c;
    pub const WRITING_DATA: i32 = 0x2d;
    pub const ALONE_IN_PARTY: i32 = 0x32;
    pub const ADS_JAVELIN: i32 = 0x33;
    pub const WEAP_LOCK_BLINK: i32 = 0x34;
    pub const WEAP_ATTACK_TOP: i32 = 0x35;
    pub const WEAP_ATTACK_DIRECT: i32 = 0x36;
    pub const SECONDS_AS_TIME: i32 = 0x37;
    pub const TABLE_LOOKUP: i32 = 0x38;
    pub const LOC_STRING: i32 = 0x39;
    pub const LOCAL_VAR_INT: i32 = 0x3a;
    pub const LOCAL_VAR_BOOL: i32 = 0x3b;
    pub const LOCAL_VAR_FLOAT: i32 = 0x3c;
    pub const LOCAL_VAR_STRING: i32 = 0x3d;
    pub const TIME_LEFT: i32 = 0x3e;
    pub const SECONDS_AS_COUNTDOWN: i32 = 0x3f;
    pub const GAME_MSG_WND_ACTIVE: i32 = 0x40;
    pub const INT: i32 = 0x41;
    pub const STRING: i32 = 0x42;
    pub const FLOAT: i32 = 0x43;
    pub const GAME_TYPE_NAME: i32 = 0x44;
    pub const GAME_TYPE: i32 = 0x45;
    pub const GAME_TYPE_DESCRIPTION: i32 = 0x46;
    pub const SCORE_AT_RANK: i32 = 0x47;
    pub const FRIENDS_ONLINE: i32 = 0x48;
    pub const SPECTATING_CLIENT: i32 = 0x49;
    pub const STAT_RANGE_ANY_BITS_SET: i32 = 0x4a;
    pub const KEY_BINDING: i32 = 0x4b;
    pub const ACTION_SLOT_USABLE: i32 = 0x4c;
    pub const HUD_FADE: i32 = 0x4d;
    pub const MAX_RECOMMENDED_PLAYERS: i32 = 0x4e;
    pub const ACCEPTING_INVITE: i32 = 0x4f;
    pub const IS_INTERMISSION: i32 = 0x50;
    /// Entries in both 1.7 tables.
    pub const COUNT: i32 = 81;
}

/// The rank `s_operatorPrecedence` (`iw3mp.exe` 0x7245c0) gives `op`: the
/// lower binds tighter.
fn rank(op: i32) -> i32 {
    use op::*;
    match op {
        NOOP => i32::MAX,
        CLOSE => 0,
        NOT | BIT_NOT => 9,
        MUL | DIV | MOD => 11,
        ADD | SUB => 13,
        SHL | SHR => 14,
        LT..=GE => 15,
        EQ | NE => 16,
        BIT_AND => 17,
        BIT_OR => 18,
        AND | OR => 25,
        COMMA => 80,
        OPEN => 99,
        _ => CALL_RANK,
    }
}

const CALL_RANK: i32 = 5;

/// Whether the operator on top of the stack runs before `incoming` goes on
/// (`iw3mp.exe` 0x53f5d0): a tighter one does, a call never does (it waits
/// for its `)`), and a repeated `/`, `%` or `-` does, so those chain left
/// to right (the test at 0x53f5b0) where the rest of a rank chains right
/// to left.
fn runs_before(stacked: i32, incoming: i32) -> bool {
    let tighter = rank(stacked) < rank(incoming) && rank(stacked) != CALL_RANK;
    let chained = stacked == incoming && matches!(incoming, op::DIV | op::MOD | op::SUB);
    tighter || chained
}

/// What a call takes off the operand stack.
enum Takes {
    Nothing,
    /// The top list's value when it holds one, else 0 (and the list
    /// stays).
    One,
    /// The whole top list.
    All,
}

fn takes(call: i32) -> Takes {
    use op::*;
    match call {
        MIN | MAX | TABLE_LOOKUP | LOC_STRING | STAT_RANGE_ANY_BITS_SET => Takes::All,
        MILLISECONDS
        | UI_ACTIVE
        | FLASHBANGED
        | SCOPED
        | SCOREBOARD_VISIBLE
        | IN_KILLCAM
        | SELECTING_LOCATION
        | WRITING_DATA..=ALONE_IN_PARTY
        | ADS_JAVELIN
        | WEAP_ATTACK_TOP
        | WEAP_ATTACK_DIRECT
        | TIME_LEFT
        | GAME_TYPE_NAME
        | GAME_TYPE
        | GAME_TYPE_DESCRIPTION
        | FRIENDS_ONLINE
        | SPECTATING_CLIENT
        | MAX_RECOMMENDED_PLAYERS
        | ACCEPTING_INVITE
        | IS_INTERMISSION => Takes::Nothing,
        _ => Takes::One,
    }
}

/// The statement's value; `Ok(None)` where 1.7's evaluator (`iw3mp.exe`
/// 0x53f640) fails: no operand list left, more than one ("stray
/// operands", 0x6e960c), or one list of several ("More than one operand",
/// 0x6e9634). A failed value reads as false or `""`.
pub fn eval(statement: &[ExpEntry], env: &dyn Env) -> Result<Option<Value>, Unsupported> {
    // Nothing below fails, and reading an env has no effect, so the first
    // bad entry may be found before anything runs.
    for entry in statement {
        match entry {
            ExpEntry::Op(n) if !(0..op::COUNT).contains(n) => {
                return Err(Unsupported(format!("operator {n}")));
            }
            ExpEntry::Unknown => return Err(Unsupported("an unread entry".into())),
            _ => {}
        }
    }
    let mut stacks = Stacks {
        env,
        pending: Vec::new(),
        lists: Vec::new(),
    };
    for entry in statement {
        stacks.feed(entry);
    }
    while !stacks.pending.is_empty() {
        stacks.run_top();
    }
    let mut lists = stacks.lists;
    Ok(match lists.pop() {
        Some(mut last) if lists.is_empty() && last.len() == 1 => last.pop(),
        _ => None,
    })
}

/// [`eval`] read as true: a value whose int is not 0.
pub fn truth(statement: &[ExpEntry], env: &dyn Env) -> Result<bool, Unsupported> {
    Ok(eval(statement, env)?.is_some_and(|v| v.truth()))
}

/// [`eval`] read as text, `""` when it fails.
pub fn string(statement: &[ExpEntry], env: &dyn Env) -> Result<String, Unsupported> {
    Ok(eval(statement, env)?.map_or_else(String::new, |v| v.string()))
}

/// [`eval`] read as a float, 0 when it fails.
pub fn float(statement: &[ExpEntry], env: &dyn Env) -> Result<f32, Unsupported> {
    Ok(eval(statement, env)?.map_or(0.0, |v| v.float()))
}

/// The two stacks a statement runs on: operators waiting their turn, and
/// operand lists (one value each until a comma joins two).
struct Stacks<'a> {
    env: &'a dyn Env,
    pending: Vec<i32>,
    lists: Vec<Vec<Value>>,
}

impl Stacks<'_> {
    fn feed(&mut self, entry: &ExpEntry) {
        let value = match entry {
            ExpEntry::Int(i) => Value::Int(*i),
            ExpEntry::Float(f) => Value::Float(*f),
            ExpEntry::Str(s) => Value::Str(s.clone()),
            ExpEntry::Op(incoming) => {
                // `(` goes straight on (0x53f6df); anything else first
                // lets the stacked operators that bind tighter run.
                if *incoming != op::OPEN {
                    while self
                        .pending
                        .last()
                        .is_some_and(|&top| runs_before(top, *incoming))
                    {
                        self.run_top();
                    }
                }
                self.pending.push(*incoming);
                return;
            }
            ExpEntry::Unknown => unreachable!("eval refuses an unread entry"),
        };
        self.lists.push(vec![value]);
    }

    fn take_one(&mut self) -> Value {
        if self.lists.last().is_some_and(|l| l.len() == 1) {
            self.lists
                .pop()
                .and_then(|mut l| l.pop())
                .unwrap_or(Value::Int(0))
        } else {
            Value::Int(0)
        }
    }

    fn take_all(&mut self) -> Vec<Value> {
        self.lists.pop().unwrap_or_else(|| vec![Value::Int(0)])
    }

    fn put(&mut self, v: Value) {
        self.lists.push(vec![v]);
    }

    /// Pops the top operator and runs it (`iw3mp.exe` 0x53e860).
    fn run_top(&mut self) {
        let Some(top) = self.pending.pop() else {
            return;
        };
        match top {
            op::NOOP | op::OPEN => {}
            // A `)` runs everything down to and including the `(` or the
            // call it closes.
            op::CLOSE => {
                while let Some(&next) = self.pending.last() {
                    self.run_top();
                    if next == op::OPEN || next >= op::SIN {
                        break;
                    }
                }
            }
            op::COMMA => {
                let tail = self.take_all();
                let mut head = self.take_all();
                head.extend(tail);
                self.lists.push(head);
            }
            op::NOT => {
                let empty = match self.take_one() {
                    Value::Str(s) => s.is_empty(),
                    n => n.float() == 0.0,
                };
                self.put(Value::Int(empty.into()));
            }
            op::BIT_NOT => {
                let n = self.take_one().int();
                self.put(Value::Int(!n));
            }
            op::SUB => {
                let rhs = self.take_one();
                if !self.lists.is_empty() {
                    let lhs = self.take_one();
                    self.put(binary(op::SUB, lhs, rhs));
                    return;
                }
                // A `-` with nothing to its left negates; a string has no
                // negative and leaves nothing.
                match rhs {
                    Value::Int(i) => self.put(Value::Int(i.wrapping_neg())),
                    Value::Float(f) => self.put(Value::Float(-f)),
                    Value::Str(_) => {}
                }
            }
            op::MUL..=op::SHR => {
                let rhs = self.take_one();
                let lhs = self.take_one();
                self.put(binary(top, lhs, rhs));
            }
            call => {
                let args = match takes(call) {
                    Takes::Nothing => Vec::new(),
                    Takes::One => vec![self.take_one()],
                    Takes::All => self.take_all(),
                };
                let v = call_value(self.env, call, &args);
                self.put(v);
            }
        }
    }
}

fn flag(b: bool) -> Value {
    Value::Int(b.into())
}

fn opposing(team: i32) -> i32 {
    match team {
        1 => 2,
        2 => 1,
        other => other,
    }
}

/// A call's value over what it took (cod4-hud.md 12.2 lists what each
/// reads). A call of one operand reads `args[0]`; [`Takes`] always gives
/// it one.
fn call_value(env: &dyn Env, call: i32, args: &[Value]) -> Value {
    use op::*;
    let arg = || args.first().cloned().unwrap_or(Value::Int(0));
    let name = || match args.first() {
        Some(Value::Str(s)) => Some(s.as_str()),
        _ => None,
    };
    let game = env.game();
    let seen = |pick: fn(&GameView) -> bool| flag(game.is_some_and(pick));
    match call {
        SIN => Value::Float(arg().float().sin()),
        COS => Value::Float(arg().float().cos()),
        MIN | MAX => {
            let keep_new = |old: f32, new: f32| if call == MIN { new < old } else { new > old };
            let v = args
                .iter()
                .map(Value::float)
                .reduce(|best, v| if keep_new(best, v) { v } else { best });
            Value::Float(v.unwrap_or(0.0))
        }
        MILLISECONDS => Value::Int(env.millis()),
        DVAR_INT | DVAR_BOOL | DVAR_FLOAT | DVAR_STRING => {
            let text = name().and_then(|n| env.dvar(n)).unwrap_or_default();
            match call {
                DVAR_FLOAT => Value::Float(atof(&text) as f32),
                DVAR_STRING => Value::Str(text),
                _ => Value::Int(atoi(&text)),
            }
        }
        STAT => Value::Int(env.stat(arg().int())),
        UI_ACTIVE => flag(env.ui_active()),
        FLASHBANGED => seen(|g| g.flashbanged),
        SCOPED => seen(|g| g.scoped),
        SCOREBOARD_VISIBLE => seen(|g| g.scoreboard),
        IN_KILLCAM => seen(|g| g.killcam),
        SELECTING_LOCATION => seen(|g| g.selecting_location),
        ADS_JAVELIN => seen(|g| g.ads_javelin),
        SPECTATING_CLIENT => seen(|g| g.following),
        IS_INTERMISSION => seen(|g| g.intermission),
        PLAYER => player_field(env, &arg()),
        TEAM | OTHER_TEAM | MARINES_FIELD | OPFOR_FIELD => {
            let team = match call {
                TEAM => env.team(),
                OTHER_TEAM => opposing(env.team()),
                MARINES_FIELD => 2,
                _ => 1,
            };
            team_field(env, team, &arg())
        }
        MENU_IS_OPEN => flag(name().is_some_and(|n| env.menu_open(n))),
        // A PC client has no party, friends list, invites, recommended
        // player count or Javelin lock: these read 0.
        WRITING_DATA..=ALONE_IN_PARTY
        | FRIENDS_ONLINE
        | MAX_RECOMMENDED_PLAYERS
        | ACCEPTING_INVITE
        | WEAP_LOCK_BLINK
        | WEAP_ATTACK_TOP
        | WEAP_ATTACK_DIRECT => Value::Int(0),
        SECONDS_AS_TIME => {
            let minutes = (arg().int() as f32 / 60.0).round() as i32;
            let (days, hours) = (minutes / 1440, minutes % 1440 / 60);
            Value::Str(format!("{days}d {hours}h {}m", minutes % 60))
        }
        SECONDS_AS_COUNTDOWN => {
            let secs = arg().int();
            Value::Str(if secs < 0 {
                String::new()
            } else {
                format!("{:2}:{:02}", secs / 60, secs % 60)
            })
        }
        TABLE_LOOKUP => Value::Str(match args {
            [table, column, value, ret] => {
                env.table_lookup(&table.string(), column.int(), &value.string(), ret.int())
            }
            _ => String::new(),
        }),
        LOC_STRING => Value::Str(env.localize(&loc_message(args))),
        LOCAL_VAR_INT | LOCAL_VAR_BOOL | LOCAL_VAR_FLOAT | LOCAL_VAR_STRING => {
            let var = name().and_then(|n| env.local(n));
            match call {
                LOCAL_VAR_FLOAT => Value::Float(var.map_or(0.0, |v| v.float())),
                LOCAL_VAR_STRING => Value::Str(var.map_or_else(String::new, |v| v.string())),
                _ => Value::Int(var.map_or(0, |v| v.int())),
            }
        }
        TIME_LEFT => Value::Int(game.and_then(|g| g.time_left).unwrap_or(0)),
        GAME_MSG_WND_ACTIVE => {
            let window = usize::try_from(arg().int()).ok();
            flag(
                game.zip(window)
                    .and_then(|(g, w)| g.message_windows.get(w).copied())
                    .unwrap_or(false),
            )
        }
        INT => Value::Int(arg().int()),
        STRING => Value::Str(arg().string()),
        FLOAT => Value::Float(arg().float()),
        GAME_TYPE_NAME => Value::Str(env.gametype_name()),
        GAME_TYPE => Value::Str(env.gametype()),
        GAME_TYPE_DESCRIPTION => Value::Str(env.gametype_description()),
        // Ranks count from 1 and must be ints.
        SCORE_AT_RANK => Value::Int(match arg() {
            Value::Int(rank) if rank >= 1 => game
                .and_then(|g| g.ranked.get(rank as usize - 1).copied())
                .unwrap_or(0),
            _ => 0,
        }),
        STAT_RANGE_ANY_BITS_SET => flag(match args {
            [from, to, mask] => {
                let mask = mask.int();
                (from.int()..=to.int()).any(|i| env.stat(i) & mask != 0)
            }
            _ => false,
        }),
        KEY_BINDING => Value::Str(name().map_or_else(String::new, |c| env.key_binding(c))),
        ACTION_SLOT_USABLE => {
            let slot = arg().int();
            let usable = game.is_some_and(|g| {
                usize::try_from(slot - 1)
                    .ok()
                    .and_then(|i| g.action_slots.get(i))
                    .copied()
                    .unwrap_or(false)
            });
            flag(usable)
        }
        // The cgame's alpha for a HUD part; 0 before the cgame is up.
        HUD_FADE => Value::Float(match (game, name()) {
            (Some(g), Some(part)) => match part.to_ascii_lowercase().as_str() {
                "dpad" => g.fade_dpad,
                "weapon" => g.fade_weapon,
                "compass" => g.fade_compass,
                "scoreboard" => 1.0,
                _ => 0.0,
            },
            _ => 0.0,
        }),
        _ => unreachable!("operator {call:#x} past the 1.7 table"),
    }
}

/// `team( ... )` and kin: `name` the team's `TEAM_*` string, `score` its
/// score, another name 0, a non-string `""`.
fn team_field(env: &dyn Env, team: i32, field: &Value) -> Value {
    let Value::Str(field) = field else {
        return Value::Str(String::new());
    };
    if field.eq_ignore_ascii_case("name") {
        Value::Str(team_name(team).into())
    } else if field.eq_ignore_ascii_case("score") {
        Value::Int(env.game().map_or(0, |g| g.team_scores[(team & 3) as usize]))
    } else {
        Value::Int(0)
    }
}

/// `player( ... )`: the local player's team names, his state and his score
/// row; another name 0, a non-string `""`.
fn player_field(env: &dyn Env, field: &Value) -> Value {
    let Value::Str(field) = field else {
        return Value::Str(String::new());
    };
    let game = env.game();
    let row = game.and_then(|g| g.own).unwrap_or_default();
    match field.to_ascii_lowercase().as_str() {
        "teamname" => Value::Str(team_name(env.team()).into()),
        "otherteamname" => Value::Str(team_name(opposing(env.team())).into()),
        "dead" => flag(game.is_some_and(|g| g.dead)),
        "nightvision" => flag(game.is_some_and(|g| g.night_vision)),
        "clipammo" => Value::Int(game.map_or(0, |g| g.clip_ammo)),
        "score" => Value::Int(row.score),
        "kills" => Value::Int(row.kills),
        "deaths" => Value::Int(row.deaths),
        "ping" => Value::Int(row.ping),
        _ => Value::Int(0),
    }
}

/// The message `locString( ... )` hands the localizer (cod4-hud.md 12.2):
/// a string is a reference key behind its `@`, joined to what precedes it
/// by `\x14` (a one-character string adds nothing); any other value is
/// plain text behind `\x15`. A separator inside a piece reads as `.`.
fn loc_message(args: &[Value]) -> String {
    let mut msg = String::new();
    let append = |msg: &mut String, text: &str| {
        msg.extend(text.chars().map(|c| {
            if ('\x14'..='\x16').contains(&c) {
                '.'
            } else {
                c
            }
        }));
    };
    for arg in args {
        match arg {
            Value::Str(key) if key.len() > 1 => {
                if !msg.is_empty() {
                    msg.push('\x14');
                }
                let mut rest = key.chars();
                rest.next();
                append(&mut msg, rest.as_str());
            }
            Value::Str(_) => {}
            plain => {
                let text = plain.string();
                if !text.is_empty() {
                    msg.push('\x15');
                }
                append(&mut msg, &text);
            }
        }
    }
    msg
}

/// `CG_GetTeamName`.
pub fn team_name(team: i32) -> &'static str {
    match team {
        1 => "TEAM_AXIS",
        2 => "TEAM_ALLIES",
        3 => "TEAM_SPECTATOR",
        _ => "TEAM_FREE",
    }
}

/// A string read as the number it spells for arithmetic: an int when
/// `atoi` and `atof` agree, else a float.
fn number_of(text: &str) -> Value {
    let whole = atoi(text);
    let real = atof(text);
    if real == f64::from(whole) {
        Value::Int(whole)
    } else {
        Value::Float(real as f32)
    }
}

/// A binary operator's value (cod4-hud.md 12.2). Strings keep their own
/// meaning under `+` (joined as text), `==` and `!=` of two strings (any
/// case) and `&&` / `||` beside a number (a string is true when not
/// empty); anywhere else a string reads as [`number_of`]. Two ints stay
/// ints except under `/`; any float makes the operation float.
fn binary(op: i32, lhs: Value, rhs: Value) -> Value {
    use Value::Str;
    let text = |v: &Value| matches!(v, Str(_));
    match op {
        op::ADD if text(&lhs) || text(&rhs) => return Str(lhs.string() + &rhs.string()),
        op::EQ | op::NE => {
            if let (Str(a), Str(b)) = (&lhs, &rhs) {
                return flag(a.eq_ignore_ascii_case(b) == (op == op::EQ));
            }
        }
        op::AND | op::OR if text(&lhs) != text(&rhs) => {
            let holds = |v: &Value| match v {
                Str(s) => !s.is_empty(),
                n => n.float() != 0.0,
            };
            let (a, b) = (holds(&lhs), holds(&rhs));
            return flag(if op == op::AND { a && b } else { a || b });
        }
        _ => {}
    }
    let numeric = |v: Value| match v {
        Str(s) => number_of(&s),
        n => n,
    };
    match (numeric(lhs), numeric(rhs)) {
        (Value::Int(a), Value::Int(b)) => int_binary(op, a, b),
        (a, b) => float_binary(op, &a, &b),
    }
}

/// The comparisons and logic, alike for ints and floats.
fn compare<T: PartialOrd + Default>(op: i32, a: T, b: T) -> Option<bool> {
    let zero = T::default();
    Some(match op {
        op::LT => a < b,
        op::LE => a <= b,
        op::GT => a > b,
        op::GE => a >= b,
        op::EQ => a == b,
        op::NE => a != b,
        op::AND => a != zero && b != zero,
        op::OR => a != zero || b != zero,
        _ => return None,
    })
}

/// Integer arithmetic wraps; `/` gives a float, and `/` or `%` by 0 gives 0.
fn int_binary(op: i32, a: i32, b: i32) -> Value {
    if let Some(holds) = compare(op, a, b) {
        return flag(holds);
    }
    Value::Int(match op {
        op::DIV => return Value::Float(if b == 0 { 0.0 } else { a as f32 / b as f32 }),
        op::MUL => a.wrapping_mul(b),
        op::MOD if b == 0 => 0,
        op::MOD => a.wrapping_rem(b),
        op::ADD => a.wrapping_add(b),
        op::SUB => a.wrapping_sub(b),
        op::BIT_AND => a & b,
        op::BIT_OR => a | b,
        op::SHL => a.wrapping_shl(b as u32),
        op::SHR => a.wrapping_shr(b as u32),
        _ => 0,
    })
}

/// Float arithmetic; `&` and `|` work on each side's int, the shifts give 0.
fn float_binary(op: i32, lhs: &Value, rhs: &Value) -> Value {
    let (a, b) = (lhs.float(), rhs.float());
    if let Some(holds) = compare(op, a, b) {
        return flag(holds);
    }
    match op {
        op::MUL => Value::Float(a * b),
        op::DIV | op::MOD if b == 0.0 => Value::Float(0.0),
        op::DIV => Value::Float(a / b),
        op::MOD => Value::Float(a % b),
        op::ADD => Value::Float(a + b),
        op::SUB => Value::Float(a - b),
        op::BIT_AND => Value::Int(lhs.int() & rhs.int()),
        op::BIT_OR => Value::Int(lhs.int() | rhs.int()),
        _ => Value::Int(0),
    }
}

/// C's `atoi`: leading space, a sign, digits; 0 for none.
pub fn atoi(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for c in digits.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + i64::from(c - b'0')).min(i64::from(u32::MAX));
    }
    (if neg { -v } else { v }) as i32
}

/// C's `atof`: the longest leading decimal number; 0 for none.
pub fn atof(s: &str) -> f64 {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    if matches!(b.first(), Some(b'-' | b'+')) {
        end = 1;
    }
    let digits = |end: &mut usize| {
        while b.get(*end).is_some_and(u8::is_ascii_digit) {
            *end += 1;
        }
    };
    digits(&mut end);
    if b.get(end) == Some(&b'.') {
        end += 1;
        digits(&mut end);
    }
    let mut best = t[..end].parse::<f64>().unwrap_or(0.0);
    if matches!(b.get(end), Some(b'e' | b'E')) {
        let mut e = end + 1;
        if matches!(b.get(e), Some(b'-' | b'+')) {
            e += 1;
        }
        let start = e;
        digits(&mut e);
        if e > start
            && let Ok(v) = t[..e].parse::<f64>()
        {
            best = v;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use ExpEntry::{Int as I, Op, Str as S};

    struct E;
    impl Env for E {
        fn dvar(&self, name: &str) -> Option<String> {
            match name {
                "ui_class_offset" => Some("200".into()),
                "g_TeamName_Allies" => Some("MPUI_MARINES_SHORT".into()),
                _ => None,
            }
        }
        fn local(&self, name: &str) -> Option<Value> {
            match name {
                "ui_highlight" => Some(Value::Int(2)),
                "ui_team" => Some(Value::Str("marines".into())),
                _ => None,
            }
        }
        fn stat(&self, index: i32) -> i32 {
            if index == 201 { 25 } else { 0 }
        }
        fn team(&self) -> i32 {
            0
        }
        fn table_lookup(&self, table: &str, column: i32, value: &str, ret: i32) -> String {
            format!("{table}:{column}:{value}:{ret}")
        }
        fn gametype(&self) -> String {
            "war".into()
        }
        fn gametype_name(&self) -> String {
            "Team Deathmatch".into()
        }
        fn gametype_description(&self) -> String {
            "Win by scoring 750 points.".into()
        }
    }

    fn s(v: &str) -> ExpEntry {
        S(v.into())
    }

    #[test]
    fn the_stock_shapes_evaluate() {
        // `( localVarInt( ui_highlight ) == 2 && localVarString(
        // ui_choicegroup ) == ""`, the closing paren left off as the zone
        // stores it.
        let highlight = [
            Op(16),
            Op(58),
            s("ui_highlight"),
            Op(1),
            Op(12),
            I(2),
            Op(14),
            Op(61),
            s("ui_choicegroup"),
            Op(1),
            Op(12),
            s(""),
        ];
        assert_eq!(truth(&highlight, &E), Ok(true));
        // `"@" + dvarString( g_TeamName_Allies )`.
        let text = [Op(16), s("@"), Op(5), Op(31), s("g_TeamName_Allies"), Op(1)];
        assert_eq!(string(&text, &E).unwrap(), "@MPUI_MARINES_SHORT");
        // `team( name ) == "TEAM_FREE" && localVarString( ui_team ) ==
        // "marines"`.
        let team = [
            Op(16),
            Op(40),
            s("name"),
            Op(1),
            Op(12),
            s("TEAM_FREE"),
            Op(14),
            Op(61),
            s("ui_team"),
            Op(1),
            Op(12),
            s("marines"),
        ];
        assert_eq!(truth(&team, &E), Ok(true));
        assert_eq!(
            truth(&[Op(16), Op(32), I(257), Op(1), Op(11), I(1)], &E),
            Ok(false)
        );
        // `tableLookup( t, 1, (dvarInt( ui_class_offset ) + 1), 5 )`: the
        // comma joins four operands, the call takes them whole.
        let lookup = [
            Op(16),
            Op(56),
            s("mp/classTable.csv"),
            Op(17),
            I(1),
            Op(17),
            Op(16),
            Op(28),
            s("ui_class_offset"),
            Op(1),
            Op(5),
            I(1),
            Op(1),
            Op(17),
            I(5),
            Op(1),
        ];
        assert_eq!(string(&lookup, &E).unwrap(), "mp/classTable.csv:1:201:5");
        // `toInt( min( stat(201) , 30 ) )`.
        let max = [
            Op(16),
            Op(65),
            Op(25),
            Op(32),
            I(201),
            Op(1),
            Op(17),
            I(30),
            Op(1),
            Op(1),
        ];
        assert_eq!(eval(&max, &E), Ok(Some(Value::Int(25))));
    }

    /// The `class` menu's header: `exp text( gameTypeName() )` and
    /// `exp text( gameTypeDescription() )`, calls of no operand.
    #[test]
    fn the_game_type_calls_read_the_env() {
        assert_eq!(
            string(&[Op(16), Op(0x44), Op(1)], &E).unwrap(),
            "Team Deathmatch"
        );
        assert_eq!(
            string(&[Op(16), Op(0x46), Op(1)], &E).unwrap(),
            "Win by scoring 750 points."
        );
        let war = [Op(16), Op(0x45), Op(1), Op(12), s("WAR")];
        assert_eq!(truth(&war, &E), Ok(true));
    }

    /// The cgame calls the HUD, scoreboard and create-a-class menus make
    /// (`g_expOperatorNames`, `iw3mp.exe` 0x723f38).
    #[test]
    fn the_cgame_calls_read_the_game_view() {
        struct G(GameView);
        impl Env for G {
            fn dvar(&self, n: &str) -> Option<String> {
                E.dvar(n)
            }
            fn local(&self, n: &str) -> Option<Value> {
                E.local(n)
            }
            fn stat(&self, i: i32) -> i32 {
                if i == 3151 { 4 } else { 0 }
            }
            fn team(&self) -> i32 {
                2
            }
            fn table_lookup(&self, _: &str, _: i32, _: &str, _: i32) -> String {
                String::new()
            }
            fn game(&self) -> Option<&GameView> {
                Some(&self.0)
            }
            fn key_binding(&self, cmd: &str) -> String {
                if cmd == "+actionslot 1" {
                    "N".into()
                } else {
                    "Unbound".into()
                }
            }
            fn localize(&self, text: &str) -> String {
                format!("<{text}>")
            }
        }
        let g = G(GameView {
            killcam: true,
            own: Some(ScoreRow {
                score: 30,
                ping: 50,
                deaths: 2,
                kills: 3,
            }),
            ranked: vec![70, 30],
            team_scores: [0, 100, 250, 0],
            time_left: Some(125),
            fade_weapon: 0.5,
            action_slots: [true, false, false, false],
            ..GameView::default()
        });
        let call = |op: i32, arg: ExpEntry| eval(&[Op(16), Op(op), arg, Op(1)], &g).unwrap();
        assert_eq!(
            eval(&[Op(16), Op(0x25), Op(1)], &g),
            Ok(Some(Value::Int(1)))
        );
        assert_eq!(call(0x28, s("score")), Some(Value::Int(250)));
        assert_eq!(call(0x29, s("score")), Some(Value::Int(100)));
        assert_eq!(call(0x26, s("kills")), Some(Value::Int(3)));
        assert_eq!(
            call(0x26, s("otherteamname")),
            Some(Value::Str("TEAM_AXIS".into()))
        );
        assert_eq!(call(0x47, I(1)), Some(Value::Int(70)));
        assert_eq!(call(0x47, I(3)), Some(Value::Int(0)));
        assert_eq!(call(0x3f, I(125)), Some(Value::Str(" 2:05".into())));
        assert_eq!(call(0x3f, I(-1)), Some(Value::Str(String::new())));
        assert_eq!(call(0x37, I(90061)), Some(Value::Str("1d 1h 1m".into())));
        assert_eq!(call(0x4b, s("+actionslot 1")), Some(Value::Str("N".into())));
        assert_eq!(call(0x4c, I(1)), Some(Value::Int(1)));
        assert_eq!(call(0x4c, I(5)), Some(Value::Int(0)));
        assert_eq!(call(0x4d, s("weapon")), Some(Value::Float(0.5)));
        assert_eq!(
            eval(&[Op(16), Op(0x3e), Op(1)], &g),
            Ok(Some(Value::Int(125)))
        );
        // `statRangeAnyBitsSet( 3150, 3152, 4 )`.
        let bits = [
            Op(16),
            Op(0x4a),
            I(3150),
            Op(17),
            I(3152),
            Op(17),
            I(4),
            Op(1),
        ];
        assert_eq!(eval(&bits, &g), Ok(Some(Value::Int(1))));
        // `locString( "@MPUI_A", 3, "@MPUI_B" )`.
        let loc = [
            Op(16),
            Op(0x39),
            s("@MPUI_A"),
            Op(17),
            I(3),
            Op(17),
            s("@MPUI_B"),
            Op(1),
        ];
        assert_eq!(
            eval(&loc, &g),
            Ok(Some(Value::Str("<MPUI_A\x153\x14MPUI_B>".into())))
        );
        // No game view: every call reads 0.
        assert_eq!(
            eval(&[Op(16), Op(0x25), Op(1)], &E),
            Ok(Some(Value::Int(0)))
        );
    }

    #[test]
    fn precedence_follows_the_table() {
        // `*` binds before `+`; `2 + 3 * 4` is 14.
        assert_eq!(
            eval(&[I(2), Op(5), I(3), Op(2), I(4)], &E),
            Ok(Some(Value::Int(14)))
        );
        // Equal ranks run right to left unless the operator repeats:
        // `10 - 4 - 3` is 3, and so is `10 - 4 + 3`, 10 - (4 + 3).
        assert_eq!(
            eval(&[I(10), Op(6), I(4), Op(6), I(3)], &E),
            Ok(Some(Value::Int(3)))
        );
        assert_eq!(
            eval(&[I(10), Op(6), I(4), Op(5), I(3)], &E),
            Ok(Some(Value::Int(3)))
        );
        // `/` of two ints is a float; a negated leading operand.
        assert_eq!(eval(&[I(1), Op(3), I(4)], &E), Ok(Some(Value::Float(0.25))));
        assert_eq!(eval(&[Op(6), I(4)], &E), Ok(Some(Value::Int(-4))));
        assert_eq!(
            eval(&[Op(81), s("x"), Op(1)], &E),
            Err(Unsupported("operator 81".into()))
        );
    }

    /// Operator edges no stock menu reaches, pinned on their values.
    #[test]
    fn operator_edges_hold() {
        use Value::{Float as F, Int as N, Str as T};
        let f = |v: f32| ExpEntry::Float(v);
        let cases: Vec<(Vec<ExpEntry>, Option<Value>)> = vec![
            (vec![s("a"), Op(14), I(2)], Some(N(1))),
            (vec![s(""), Op(15), f(0.0)], Some(N(0))),
            (vec![s("a"), Op(14), s("b")], Some(N(0))),
            (vec![s("2"), Op(2), s("3")], Some(N(6))),
            (vec![s("2.5"), Op(5), I(1)], Some(T("2.51".into()))),
            (vec![I(1), Op(5), f(0.5)], Some(F(1.5))),
            (vec![I(1), Op(5), s("x")], Some(T("1x".into()))),
            (vec![f(0.5), Op(5), s("x")], Some(T("0.500000x".into()))),
            (vec![s("ABC"), Op(12), s("abc")], Some(N(1))),
            (vec![s("2"), Op(12), I(2)], Some(N(1))),
            (vec![f(7.5), Op(4), I(2)], Some(F(1.5))),
            (vec![I(7), Op(4), I(0)], Some(N(0))),
            (vec![I(7), Op(3), I(0)], Some(F(0.0))),
            (vec![I(1), Op(21), I(4)], Some(N(16))),
            (vec![f(1.0), Op(21), I(4)], Some(N(0))),
            (vec![f(6.9), Op(18), I(3)], Some(N(2))),
            (vec![I(16777217), Op(19), f(0.0)], Some(N(16777217))),
            (vec![Op(20), I(5)], Some(N(-6))),
            (vec![Op(7), s("")], Some(N(1))),
            (vec![Op(7), f(0.0)], Some(N(1))),
            (vec![Op(6), f(2.0)], Some(F(-2.0))),
            (vec![Op(6), s("x")], None),
            (vec![I(2), Op(2), Op(6), I(3)], Some(N(-3))),
            (vec![I(8), Op(3), I(2), Op(3), I(2)], Some(F(2.0))),
            (vec![I(1), Op(17), I(2)], None),
            (vec![Op(16), Op(0x41), I(1), Op(17), I(2), Op(1)], None),
            (
                vec![Op(16), Op(0x19), I(3), Op(17), f(1.5), Op(17), I(2), Op(1)],
                Some(F(1.5)),
            ),
            (vec![Op(16), Op(0x1a), Op(1)], Some(F(0.0))),
            (vec![Op(16), Op(0x38), I(1), Op(1)], Some(T(String::new()))),
            (vec![Op(16), Op(0x28), I(1), Op(1)], Some(T(String::new()))),
            (
                vec![Op(16), Op(0x2b), s("NAME"), Op(1)],
                Some(T("TEAM_AXIS".into())),
            ),
            (vec![Op(16), Op(0x2a), s("kills"), Op(1)], Some(N(0))),
            (vec![Op(16), Op(0x47), s("1"), Op(1)], Some(N(0))),
            (
                vec![Op(16), Op(0x3d), s("ui_highlight"), Op(1)],
                Some(T("2".into())),
            ),
            (
                vec![Op(16), Op(0x1e), s("ui_class_offset"), Op(1)],
                Some(F(200.0)),
            ),
            (
                vec![
                    Op(16),
                    Op(0x39),
                    s("@"),
                    Op(17),
                    s("@A\x15B"),
                    Op(17),
                    f(0.5),
                    Op(1),
                ],
                Some(T("A.B\x150.500000".into())),
            ),
            (
                vec![Op(16), I(1), Op(5), I(2), Op(1), Op(2), I(3)],
                Some(N(9)),
            ),
            (vec![], None),
        ];
        for (st, want) in cases {
            assert_eq!(eval(&st, &E), Ok(want), "{st:?}");
        }
        assert_eq!(
            eval(&[I(1), ExpEntry::Unknown], &E),
            Err(Unsupported("an unread entry".into()))
        );
    }

    #[test]
    fn c_number_parsing() {
        assert_eq!(atoi("  -12ab"), -12);
        assert_eq!(atoi("x"), 0);
        assert_eq!(atof("1.5e2x"), 150.0);
        assert_eq!(atof(".5"), 0.5);
        assert_eq!(number_of("2"), Value::Int(2));
        assert_eq!(number_of("2.5"), Value::Float(2.5));
    }
}
