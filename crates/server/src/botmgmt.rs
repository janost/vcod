//! Bot management, a vcod extension (retail has no bots): the name pool,
//! the cvars that drive it and the planner that turns a roster into joins,
//! drops and team moves. The server's glue lives in `server/bot_admin.rs`;
//! this half is pure so the rules can be tested without a level.

/// What every managed bot's name starts with.
pub const PREFIX: &str = "[BOT] ";

/// The longest name retail's `ClientUserinfoChanged` keeps: it copies the
/// userinfo `name` into a 32-byte buffer and stops at 31 bytes
/// (game.mp.i386.so 0x520f4; bot-objectives.md, "Bot management").
pub const MAX_NAME_BYTES: usize = 31;

/// The pool names are drawn from, without repeats among connected bots.
/// Each fits [`MAX_NAME_BYTES`] behind [`PREFIX`] with a two-digit suffix.
pub const NAMES: &[&str] = &[
    "Sir Camps-a-Lot",
    "NadeSpammer",
    "PronePatrol",
    "Kar98Karen",
    "TommyGunTommy",
    "BAR Tender",
    "Garand Ping",
    "Lag Leader",
    "Spawn Camper",
    "Doughboy420",
    "PanzerPanic",
    "StenStan",
    "MP40 Max",
    "Sgt. Strafe",
    "Cpl. Crouch",
    "Pvt. Pinecone",
    "Lt. Leanalot",
    "Major Lag",
    "Captain Clutch",
    "General Nuisance",
    "Colonel Kernel",
    "Bunny Hopper",
    "Hedgerow Hero",
    "Bocage Bob",
    "Carentan Carl",
    "Brecourt Bruce",
    "Hurtgen Hank",
    "Dawnville Dan",
    "Harbor Hugo",
    "Chateau Charlie",
    "Railyard Ray",
    "Stalingrad Stu",
    "Pavlov's Dog",
    "Depot Dave",
    "Powcamp Pete",
    "Rocket Rodney",
    "Mortar Morty",
    "Flak Jacket",
    "Dog Tag Doug",
    "Helmet Hair",
    "Iron Sights",
    "No Scope Joe",
    "Quick Scoper",
    "Tracer Trace",
    "Ricochet Rick",
    "Ammo Dump",
    "Last Clip",
    "Reload Rhonda",
    "Bayonet Betty",
    "Bash Brother",
    "Knife Party",
    "Grenadier Greg",
    "Potato Masher",
    "Cooked Nade",
    "Smoke Show",
    "Foxhole Fred",
    "Trench Foot",
    "Sandbag Sam",
    "Bunker Buster",
    "Bridge Too Far",
    "Paratrooper Pat",
    "Glider Gus",
    "D-Day Danny",
    "Omaha Owen",
    "Utah Ursula",
    "Juno Juniper",
    "Gold Beach Gary",
    "Sword Swinger",
    "Medic Pls",
    "Bandage Barry",
    "Morale Officer",
    "Ration Randy",
    "Spam and Eggs",
    "Chocolate Ration",
    "Canteen Kid",
    "Mess Hall Mel",
    "Field Radio",
    "Static Steve",
    "Morse Coder",
    "Recon Rachel",
    "Binocular Bill",
    "Scout Sniper",
    "Window Warden",
    "Rooftop Rudy",
    "Ladder Larry",
    "Doorway Dweller",
    "Corner Camper",
    "Peekaboo Pete",
    "Lean Machine",
    "Dolphin Diver",
    "Crawl Space",
    "Tank Trap",
    "Jeep Thief",
    "Flak 88 Fan",
    "Sherman Tanked",
    "Tiger Tamer",
    "Panther Prowl",
    "Halftrack Hal",
    "MG42 Maniac",
    "Bipod Bertha",
    "Overheat Otto",
    "Belt Fed Ben",
    "Turret Hugger",
    "Killcam Karl",
    "Respawn Ricky",
    "Friendly Fire",
    "Teamkill Tim",
    "Objective Ollie",
    "Bomb Planter",
    "Defuse Dexter",
    "Flag Runner",
    "Radio Hog",
    "Base Raider",
    "Dom Dominator",
    "Headshot Harry",
    "Kneecap Kevin",
    "Lucky Shot",
    "Wallbang Walter",
    "Fragtastic",
    "Noob Tube",
    "Get Rekt Rudolf",
];

/// The bot-management cvars, all vcod-only rows of `cvars/registry.rs`.
pub mod cvar {
    /// A floor on players: humans plus bots, topped up with bots. 0 is off.
    pub const MIN_PLAYERS: &str = "vc_bot_minplayers";
    /// Moves bots between teams so the two differ by at most one.
    pub const AUTOBALANCE: &str = "vc_bot_autobalance";
    /// Whether bots fight; 0 leaves them walking the map.
    pub const SHOOT: &str = "vc_bot_shoot";
    /// Draws names from [`super::NAMES`]; 0 names them `bot<slot>`.
    pub const NAMES: &str = "vc_bot_names";
    /// The skill level new and unpinned bots play at
    /// (`crate::bots::Skill::level`).
    pub const SKILL: &str = "vc_bot_skill";
    /// Whether bots break contact when hurt or dry and detour to health
    /// items (`crate::bots::Bot::allow_retreat`).
    pub const RETREAT: &str = "vc_bot_retreat";
}

/// What the cvars ask for, read once a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub min_players: usize,
    pub autobalance: bool,
    pub shoot: bool,
    pub names: bool,
    /// Clamped to `crate::bots::MAX_LEVEL`.
    pub skill: u8,
    pub retreat: bool,
}

/// One side of a team game, or neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Allies,
    Axis,
    /// Spectating, in `dm`, or not yet on a team.
    None,
}

impl Side {
    /// The team menu's response for this side.
    pub fn response(self) -> &'static str {
        match self {
            Side::Allies => "allies",
            Side::Axis => "axis",
            Side::None => "autoassign",
        }
    }

    pub fn parse(s: &str) -> Option<Side> {
        match s.to_ascii_lowercase().as_str() {
            "allies" => Some(Side::Allies),
            "axis" => Some(Side::Axis),
            "auto" | "autoassign" => Some(Side::None),
            _ => None,
        }
    }

    fn other(self) -> Side {
        match self {
            Side::Allies => Side::Axis,
            Side::Axis => Side::Allies,
            Side::None => Side::None,
        }
    }
}

/// One connected client as the planner sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub slot: usize,
    pub bot: bool,
    /// The team it plays on, or for a bot still joining the one it asks for.
    pub side: Side,
    pub score: i32,
    /// Alive and in the fight: a bot is moved only when this is false.
    pub playing: bool,
    /// A bot `addbot` put on a team by name; balance never moves it.
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Add(Side),
    Remove(usize),
    Move(usize, Side),
}

/// How many bots the server keeps: the manual count (`--bots`, `addbot`,
/// `removebot`) within the slots bots may hold, or the floor's top-up,
/// which leaves one of those slots free for a joining human, whichever is
/// more. `room` counts the free public slots plus those bots hold now.
pub fn target(manual: usize, min_players: usize, humans: usize, room: usize) -> usize {
    let floor = min_players
        .saturating_sub(humans)
        .min(room.saturating_sub(1));
    manual.min(room).max(floor)
}

/// The joins, drops and moves that take `members` to `want` bots. In a team
/// game with balance on, a join goes to the smaller side, a drop comes off
/// the larger one, and a bot that is not playing moves while the two sides
/// differ by more than one. Drops take the lowest score first.
pub fn plan(members: &[Member], want: usize, teamed: bool, balance: bool) -> Vec<Action> {
    let balance = teamed && balance;
    let mut sides = [0i32; 2];
    let idx = |s: Side| match s {
        Side::Allies => Some(0),
        Side::Axis => Some(1),
        Side::None => None,
    };
    for m in members {
        if let Some(i) = idx(m.side) {
            sides[i] += 1;
        }
    }
    let larger = |sides: &[i32; 2]| {
        if sides[1] > sides[0] {
            Side::Axis
        } else {
            Side::Allies
        }
    };
    let mut bots: Vec<&Member> = members.iter().filter(|m| m.bot).collect();
    // Lowest score first, the higher slot on a tie (the later joiner).
    bots.sort_by(|a, b| a.score.cmp(&b.score).then(b.slot.cmp(&a.slot)));
    let mut out = Vec::new();
    if bots.len() < want {
        for i in 0..want - bots.len() {
            let side = if balance {
                // Allies on a tie, as `--bots` always dealt them.
                let s = if sides[0] <= sides[1] {
                    Side::Allies
                } else {
                    Side::Axis
                };
                sides[idx(s).unwrap()] += 1;
                s
            } else if (bots.len() + i).is_multiple_of(2) {
                // `dm` too: the side picks the weapon menu's nationality.
                Side::Allies
            } else {
                Side::Axis
            };
            out.push(Action::Add(side));
        }
        return out;
    }
    if bots.len() > want {
        for _ in 0..bots.len() - want {
            let from = balance
                .then(|| larger(&sides))
                .filter(|_| sides[0] != sides[1]);
            let pick = bots
                .iter()
                .position(|b| from.is_none_or(|s| b.side == s))
                .unwrap_or(0);
            let b = bots.remove(pick);
            if let Some(i) = idx(b.side) {
                sides[i] -= 1;
            }
            out.push(Action::Remove(b.slot));
        }
        return out;
    }
    if balance {
        while (sides[0] - sides[1]).abs() > 1 {
            let from = larger(&sides);
            let Some(pick) = bots
                .iter()
                .position(|b| b.side == from && !b.playing && !b.pinned)
            else {
                break;
            };
            let b = bots.remove(pick);
            let to = from.other();
            sides[idx(from).unwrap()] -= 1;
            sides[idx(to).unwrap()] += 1;
            out.push(Action::Move(b.slot, to));
        }
    }
    out
}

/// A name for a new bot, [`PREFIX`] included: a pool name no connected bot
/// carries, drawn with `roll`, or once the pool is spent the first
/// numbered copy of one not taken (`[BOT] NadeSpammer 2`).
pub fn draw_name(taken: &[String], roll: &mut dyn FnMut(usize) -> usize) -> String {
    let full = |n: &str| format!("{PREFIX}{n}");
    let free: Vec<&str> = NAMES
        .iter()
        .copied()
        .filter(|n| !taken.iter().any(|t| *t == full(n)))
        .collect();
    if !free.is_empty() {
        return full(free[roll(free.len())]);
    }
    let base = NAMES[roll(NAMES.len())];
    (2..)
        .map(|k| format!("{PREFIX}{base} {k}"))
        .find(|n| !taken.contains(n))
        .expect("a free number")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(slot: usize, bot: bool, side: Side, score: i32) -> Member {
        Member {
            slot,
            bot,
            side,
            score,
            playing: true,
            pinned: false,
        }
    }

    #[test]
    fn the_pool_holds_a_hundred_distinct_names_that_fit() {
        let mut seen = std::collections::BTreeSet::new();
        for n in NAMES {
            assert!(seen.insert(n.to_ascii_lowercase()), "{n} twice");
            let full = format!("{PREFIX}{n} 99");
            assert!(full.len() <= MAX_NAME_BYTES, "{full:?} is too long");
            // Info string separators, colour codes and the rcon command
            // separator would break the userinfo or the console.
            assert!(!n.contains(['\\', '"', ';', '^', '%']), "{n}");
            assert_eq!(n.trim(), *n);
        }
        assert!(seen.len() >= 100, "{} names", seen.len());
    }

    #[test]
    fn names_do_not_repeat_until_the_pool_is_spent() {
        let mut taken = Vec::new();
        let mut state = 7usize;
        let mut roll = |n: usize| {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            (state >> 8) % n
        };
        for _ in 0..NAMES.len() {
            let n = draw_name(&taken, &mut roll);
            assert!(!taken.contains(&n));
            taken.push(n);
        }
        let extra = draw_name(&taken, &mut roll);
        assert!(extra.ends_with(" 2"), "{extra}");
        assert!(!taken.contains(&extra));
        taken.push(extra.clone());
        let base = extra.trim_end_matches(" 2").to_string();
        // The same base drawn again moves on to the next number.
        let mut first = |_: usize| NAMES.iter().position(|n| base.ends_with(n)).unwrap();
        assert_eq!(draw_name(&taken, &mut first), format!("{base} 3"));
    }

    #[test]
    fn the_floor_tops_up_and_leaves_a_slot() {
        assert_eq!(target(0, 0, 3, 10), 0);
        assert_eq!(target(0, 6, 2, 10), 4);
        assert_eq!(target(0, 6, 6, 10), 0);
        assert_eq!(target(0, 6, 8, 10), 0);
        // A floor that fills the server leaves one slot for a human.
        assert_eq!(target(0, 20, 0, 8), 7);
        // Manual bots may fill it, and outrank a lower floor.
        assert_eq!(target(8, 0, 0, 8), 8);
        assert_eq!(target(12, 0, 0, 8), 8);
        assert_eq!(target(3, 2, 1, 8), 3);
    }

    #[test]
    fn joins_go_to_the_smaller_side() {
        let m = [
            member(0, false, Side::Allies, 0),
            member(1, false, Side::Allies, 0),
        ];
        assert_eq!(
            plan(&m, 3, true, true),
            [
                Action::Add(Side::Axis),
                Action::Add(Side::Axis),
                Action::Add(Side::Allies)
            ]
        );
        assert_eq!(
            plan(&[], 2, true, false),
            [Action::Add(Side::Allies), Action::Add(Side::Axis)]
        );
        assert_eq!(plan(&[], 1, false, true), [Action::Add(Side::Allies)]);
    }

    #[test]
    fn drops_come_off_the_larger_side_lowest_score_first() {
        let m = [
            member(0, false, Side::Allies, 0),
            member(1, true, Side::Allies, 5),
            member(2, true, Side::Allies, 2),
            member(3, true, Side::Axis, -1),
        ];
        assert_eq!(plan(&m, 2, true, true), [Action::Remove(2)]);
        // Without balance the lowest score goes, whatever its side.
        assert_eq!(plan(&m, 2, true, false), [Action::Remove(3)]);
        assert_eq!(
            plan(&m, 0, true, true),
            [Action::Remove(2), Action::Remove(1), Action::Remove(3)]
        );
    }

    #[test]
    fn only_a_bot_out_of_the_fight_moves() {
        let mut m = vec![
            member(0, false, Side::Allies, 0),
            member(1, false, Side::Allies, 0),
            member(2, true, Side::Allies, 3),
            member(3, true, Side::Allies, 1),
            member(4, true, Side::Axis, 0),
        ];
        assert_eq!(plan(&m, 3, true, true), []);
        m[2].playing = false;
        assert_eq!(plan(&m, 3, true, true), [Action::Move(2, Side::Axis)]);
        m[2].pinned = true;
        assert_eq!(plan(&m, 3, true, true), []);
        // Humans are never moved.
        let h = [
            member(0, false, Side::Allies, 0),
            member(1, false, Side::Allies, 0),
            member(2, false, Side::Allies, 0),
        ];
        assert_eq!(plan(&h, 0, true, true), []);
    }
}
