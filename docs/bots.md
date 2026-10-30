# Bots

vcod-server can fill a server with bots. Retail Call of Duty has none, so
everything here is vcod's own. A bot is a real client on the server: it takes
a slot, joins through the stock team and weapon menus, and shows up in the
scoreboard and in `status` like anyone else. Retail clients see bots as
ordinary players.

This page covers running and managing bots, and [how a bot
thinks](#how-a-bot-thinks). How they find their way around a
map is in [research/bot-navigation.md](research/bot-navigation.md), and how
they play each gametype's objectives is in
[research/bot-objectives.md](research/bot-objectives.md).

## Quick start

Start a server with six fighting bots:

```
vcod-server mp_carentan --gametype tdm --bots 6 --bots-shoot --set rconPassword=secret
```

Or keep a server busy on its own: with a floor of 10, an empty server runs a
5v5 of bots, and each human who joins takes a bot's place. `--max-clients`
defaults to 8, so raise it above the floor.

```
vcod-server mp_carentan --gametype tdm --max-clients 16 --set vc_bot_minplayers=10 --set rconPassword=secret
```

Everything below can also be changed on a running server over rcon, from a
retail client's console (`rcon <command>` after `rconPassword secret`) or from
vcod's.

## Command line

| Flag | Does |
|---|---|
| `--bots N` | Starts the server with N bots. Each takes a real slot, so N can't exceed `--max-clients`. |
| `--bots-shoot` | Lets those bots fire. Without it they walk the map and play objectives but never shoot (`vc_bot_shoot 0`). |
| `--set NAME=VALUE` | Sets any cvar below before the first map loads, as retail's `+set` does. |

## rcon commands

rcon is refused until the server has `rconPassword` set.

| Command | Does |
|---|---|
| `addbot [allies\|axis\|auto] [count] [skill N]` | Adds `count` bots (default 1). The words can come in any order. `auto` (the default) puts each bot on whichever team balance asks for. Naming a team puts the bots there and pins them, so balance never moves them. `skill N` sets the bots' skill level (0-3, see [Skill](#skill)); `vc_bot_skill` never changes it afterwards. |
| `removebot [name\|all\|count]` | With no argument, removes one bot. A name removes that bot; the `[BOT] ` prefix is optional and case doesn't matter. `all` removes every bot. A number removes that many, lowest score first. |
| `listbots` | Prints every bot's slot, team, score, whether it is pinned, its skill level and its name, then the human and bot counts and the current `vc_bot_*` values. |

Examples:

```
rcon addbot 4
rcon addbot axis 2
rcon addbot allies 3 skill 3
rcon removebot NadeSpammer
rcon removebot all
rcon listbots
```

## Cvars

Set them with `rcon set <cvar> <value>` (or `seta`), or with `--set` at
startup. A change takes effect within half a second, with no restart.

| Cvar | Default | Does |
|---|---|---|
| `vc_bot_minplayers` | 0 | The minimum player count. 0 turns it off. |
| `vc_bot_autobalance` | 1 | Keeps the teams even in team gametypes. |
| `vc_bot_shoot` | 1 | 0 stops bots from firing. `--bots` without `--bots-shoot` sets it to 0. |
| `vc_bot_names` | 1 | 0 names new bots `bot<slot>` instead of drawing a funny name. |
| `vc_bot_vehicles` | 1 | United Offensive only. 0 stops bots getting into vehicles; a bot already in one rides on until it gets out. |
| `vc_bot_skill` | 1 | The skill level, 0 to 3, of every bot that `addbot ... skill N` didn't set. See [Skill](#skill). |

## Skill

Four levels set how fast a bot reacts and turns, how well it aims, how long
it remembers an enemy it lost sight of, how far it hears gunfire and how
wide it sees:

| Level | Name | Reaction | Turn rate | Walking turn | Aim error at first sight, settled | Hearing | Sight cone |
|---|---|---|---|---|---|---|---|
| 0 | easy | 400-650 ms | 200°/s | 360°/s | 64, 5 units | 0.75x | 100° |
| 1 | normal (default) | 200-350 ms | 300°/s | 480°/s | 36, 4 units | 1x | 120° |
| 2 | hard | 150-300 ms | 440°/s | 600°/s | 32, 2.5 units | 1.25x | 140° |
| 3 | expert | 100-150 ms | 600°/s | 720°/s | 24, 1.5 units | 1.5x | 160° |

The aim error is how far from the target's chest a shot would land; it
shrinks every tick the bot stays on the target and grows with range, with
the target's speed across the bot's view, and while the bot runs. A bot
fires before its aim has settled: an automatic sprays early, a bolt rifle
waits for a closer line. On its sights a bot now and then strafes slowly,
backs away from a close enemy with a bolt rifle, or lies down at long
range; a higher level does so more often. A higher level also remembers a
lost enemy and a heard shot longer.

Out of a fight a walking bot turns its view at the walking rate and keeps
running along its path while the view comes round; the tick a fight, a
look toward a hit or a peek from cover lets go, it faces its path at
once. On open floor it
heads straight for a node further along its path instead of stepping
along the navigation grid.

A bot spots a new enemy only inside its sight cone, centred on where it
looks; a bot in a vehicle seat sees all round. It keeps fighting the enemy
it already has while he stays in sight, unless another comes much closer.
An enemy who shoots it comes first: the bot turns on him, and if it can't
see him it turns toward where the shot came from. Switching back to an
enemy it was just fighting, or onto one who just hit it, costs less
reaction time and aim than a fresh one. Gunfire it hears takes it toward
the sound from any direction. How the levels play against each other is measured in
[research/bot-combat.md](research/bot-combat.md).

`vc_bot_skill` changes every bot without its own level at once, mid-fight
included. To mix levels, give each `addbot` its own:

```
rcon addbot axis 4 skill 0
rcon addbot allies 4 skill 3
```

## How the minimum player count works

With `vc_bot_minplayers` set to N, the server keeps N players in the game,
topping up with bots:

- With no humans, it runs N bots, split across the teams.
- When a human picks a team, one bot leaves, taken off the larger team.
- When a human leaves, a bot comes back.
- When N or more humans are playing, no floor bots remain.

A human counts toward the floor only once they have picked a team. A
spectator, or someone still in the team menu, does not push a bot out.

Bots added with `addbot` or `--bots` count separately: `removebot` lowers
that count, but the floor still tops the server back up to N.

## Team balance

With `vc_bot_autobalance 1`, in every team gametype (everything except `dm`
and 1.1's `bel`), new bots join the smaller team, and bots move so the two
teams differ by at most one player. The rules:

- Humans are never moved.
- Bots pinned with `addbot allies` or `addbot axis` are never moved.
- A bot only switches team while it is dead or spectating, never in the
  middle of a fight. In round-based gametypes like `sd` that usually means
  between rounds.

## Slots

Bots never take a private slot (`sv_privateClients`). `addbot` may fill
every public slot, but the minimum player floor always leaves one public
slot free, so a human can still join a full server and push a bot out.

The total player limit is `sv_maxclients`, up to 64 (the network protocol's
limit), and bots count toward it. Like on retail, `sv_maxclients` is latched,
so raise it on a running server like this:

```
rcon set sv_maxclients 32
rcon map_restart
```

The new limit takes effect at the restart or at the next map load. Players
stay connected through it.

## Names

A bot is named `[BOT] <name>`, drawn at random from a pool of 120 names in
`crates/server/src/botmgmt.rs`, such as `[BOT] Sir Camps-a-Lot`,
`[BOT] NadeSpammer` and `[BOT] PronePatrol`. No two connected bots share a
name. If more bots are connected than the pool has unused names, later ones
get a number, like `[BOT] NadeSpammer 2`. Every name fits retail's 31-byte
name limit with the prefix and a two-digit number.

The name draw uses its own random generator, so turning names on or off never
changes how a seeded server plays.

## Supported versions and gametypes

Bots play on 1.1, 1.5 and United Offensive servers, in every stock gametype:
`dm`, `tdm`, `sd`, `re` and `bel`, and UO's `ctf`, `dom`, `hq` and `bas`.
They also play CoD2 1.3 and CoD4 1.7 servers ([CoD2](#cod2), [CoD4](#cod4)).
They are not very good. They find their way around stock maps, play each
gametype's objectives, and fight when `vc_bot_shoot` is on.
On objective gametypes a team spreads its bots over different routes,
reshuffles who guards what each round, sends all its S&D attackers to one
site and covers a planter, and in Retrieval escorts the carrier and holds
the goal ahead of him
([research/bot-objectives.md](research/bot-objectives.md) section 9).

On a United Offensive map with vehicles, a bot with no fight and no
objective close by walks to a vehicle near it and gets in with the use key,
like a player. A driver drives toward its objective or an enemy, waits a
moment for a teammate walking up to board, holds or backs off for a
teammate in its way, and in a tank stops to lay the turret on an enemy or
an enemy vehicle and fire. A gunner fires the vehicle's machine gun. A bot
on foot carrying a rocket launcher fires it at an enemy-crewed vehicle in
its sight. Bots get out when they arrive, when stuck, or after 90 seconds
(section 10 of the same page).

## How a bot thinks

The brain is hand-written rules. There is no learning and no per-map
script. CoD1, UO, CoD2 and CoD4 share one brain (`crates/server/src/bots.rs`).

### The loop

Each server frame (50 ms) the server fills a `BotView` for every bot and
calls `Bot::think` with it. `think` returns one usercmd: move keys, view
angles, buttons and the selected weapon. The server runs that cmd through
the game's own `ClientThink` and pmove, the path a human's cmd takes.

`think` reads the view and nothing else. It cannot move the body, look
through a wall or skip a physics rule, so anything a bot does a player
could do with the same keys. The server decides what goes in the view:

- the bot's own state (health, ammo, weapons held, stance, ground, ladder)
- the enemies a line-of-sight trace reaches inside its sight cone
- gunfire within its hearing range
- the objective's state for the gametype
- probes the brain asks for on some ticks, such as a drop or a lob ahead

### What a tick decides

`think` works down this list and stops at the first case that applies.

1. Dead. The bot waits 1.5 s, then presses use once a second until the
   stock script respawns it. Whatever it was doing ends with the life.
2. Spectating or at intermission. It presses nothing.
3. In a UO vehicle seat. The vehicle code drives or guns.
4. Stuck. A bot that has stood 10 s where the graph knows no way out (a
   dead end, a pit, a ring of parked vehicles), or has had nothing to
   fire for 10 s, kills itself to respawn. It holds on instead where a
   death sits out the round (`sd`, `re`).
5. A grenade throw in progress. A small state machine switches to the
   grenade, pulls it back, cooks it where the game has a cook, throws,
   and switches back.
6. Otherwise it fights if it has an enemy, plays the objective if there
   is one, and roams toward fights if not.

### Fighting

- It keeps the enemy it is fighting while he stays in sight. A new one
  takes over only when he stands under 60% of the current one's range
  (`RETARGET_RATIO`), because each switch costs reaction time and aim.
- Whoever just hit it comes first. Hit by someone it cannot see, it turns
  toward the shot.
- A new target costs a reaction delay (100-650 ms by skill). The aim then
  starts off the target's chest and closes in each tick the bot stays on
  him. The error grows with range, with the target's speed across the
  view, and while the bot runs. Automatics fire early, bolt rifles wait
  for a better line.
- Around the shooting it strafes on its sights, backs off a close enemy
  with a bolt rifle, lies down at long range, takes cover and leans,
  retreats when hurt, and peeks for or hunts an enemy it lost.

The [skill levels](#skill) change only the numbers: reaction, turn rate,
aim error, hearing, sight cone and memory. Routes and objective play are
the same at every level.

### Getting around

- At map load the server builds a navigation graph. It floods the
  walkable space from every spawn point, and each link (walk, jump, drop,
  ladder, mantle) goes in only after a simulated body has travelled it
  under that game's pmove. A jump a bot cannot make is not in the graph.
- A* plans over the graph. Searches share a per-tick budget and may take
  several ticks, so twelve bots planning at once do not spike the frame.
- The follower turns the path into keys. It steers at the next node,
  jumps by the same rule that proved the jump, waits out the jump
  cooldown, climbs at the rate the ladder was proven at, and takes a
  risky drop only when no other route exists.
- A teammate standing still in the way, or a parked vehicle, blocks the
  graph nodes it covers for a while, and the bot plans round it.

The graph build and the follower are in
[research/bot-navigation.md](research/bot-navigation.md).

### Objectives

Each gametype has its own goal rules: take and carry the bomb or flag,
plant and defuse, capture and hold a flag or the radio. The objective
decides where the bot walks. When an enemy shows up the fight takes over,
and the bot goes back to the objective after it. Per gametype detail is in
[research/bot-objectives.md](research/bot-objectives.md).

### One brain, four games

A thin adapter per game sits between the brain and the server. It
rewrites the cmd into that game's button bits, renumbers the game's
weapons for the brain, and answers the game's team, weapon and class
menus. A few behaviours differ by game, never by map. The `BotWorld`
trait in `crates/server/src/bots/driver.rs` holds them as switches each
game sets, such as CoD4's `MANTLE_RESTS` and `PARTS_MATES_FLAT`. The
graph's simulated walks use each game's own movement rules.

### How a change is judged

Retail has no bots, so there is nothing to compare against. A behaviour
change runs `bot_match` on the same seeds before and after (630 matches
for CoD4) and lands only when stalls drop and kills hold. Results are
recorded in [research/bot-combat.md](research/bot-combat.md) and
[research/bot-objectives.md](research/bot-objectives.md).

## CoD2

`vcod-server <map> --game cod2 --bots N [--bots-shoot]` adds `N` bots,
named from CoD1's pool (`[BOT] ...`; `vc_bot_names 0` gives `bot0`,
`bot1`, ...), to a CoD2 1.3 server once the level is up. They
answer `serverinfo_<gametype>` with `close`, the team menu with
`autoassign` in `dm` and alternately `allies` and `axis` otherwise, and the
weapon menu with a weapon its nationality offers and the `scr_allow_*`
cvars allow (CoD2's stock lists, shotgun included), the one the fewest
other bots carry, so a side's bots spread over the menu. The brain is the CoD1
one: its cmds are rewritten into CoD2's button bits (attack, melee 0x4,
use 0x8, reload 0x10, leans, the two stances, the sight 0x1000, jump
0x400) and moved by CoD2's own pmove through the level's `ClientThink`,
before each frame's `G_RunFrame`. rcon's `addbot`, `removebot` (also
`kickbot`) and `listbots` and the `vc_bot_*` cvars (floor, balance,
shoot, names, skill) work as on CoD1 (docs/usage.md, "Bots").

As on CoD1 the bots never vote and are not counted as voters, so one
player's `callvote` plus his own yes passes a vote on a server full of
bots ([cod2-protocol.md](research/cod2-protocol.md) 3.4.2, "Votes").
The headless `spectate` example's `--join` client answers every vote
with `vote yes` a second after it starts, so it never holds one
either; `--no-vote` turns that off.

A new level (`map`, a passed `callvote map` or `typemap`, `map_restart`,
`fast_restart`, `map_rotate`, a gametype's level end) keeps the bots:
each forgets the menus it answered and answers the ones the level opens
again under the same indices, so they rejoin as on their first connect
(`Cod2Bots::new_level`, CoD1's `Bot::rearm` on a gamestate). A new map
drops the old graph and builds the new map's. An `sd` round's `n` keeps
`pers[]` and opens no menu; the bots play on. The `spectate --join`
client answers the same menus each time they open, so it rejoins too.

They play `sd`, `ctf` and `hq` with CoD1's objective brain, off what
CoD2's stock scripts hold (research/bot-objectives.md section 11): the
attackers plant in a bombzone and the lead defender defuses, both by
holding use in the `trigger_use_touch`; a `ctf` bot takes the other
flag, returns its own and captures at its own flag at home; an `hq` bot
takes the radio up and holds it. They throw CoD2's offhand frag the way
CoD1 bots throw (at an enemy behind cover or one just lost, on a lob
whose arc comes down by him), through the frag key; there is no cook in
CoD2's stock frag.

The graph floods the CoD2 collision (the engine's `ClipMap`, its
`SURF_LADDER` brushes as the ladders) with walks under 1.5's movement
rules, which CoD2's starts from; it builds in a few seconds and is not
saved. It keeps no leap (a jump at a ledge's lip from a standstill):
a CoD2 bot never reads the lip, so each is walked again as a plain jump
and kept only where that lands (research/bot-objectives.md 11.8). Then every node is tried for a mantle in the eight compass ways
with CoD2's own pmove (a wall flagged `MANTLE_ON` or `MANTLE_OVER` within
64, the body run at it and the jump key pressed when pmove offers the
climb); each climb that ends on the ground is a mantle edge, the floor it
lands on is flooded too, and a bot on one runs at the wall and jumps on
the offer. mp_toujane's graph has 352 of them, mp_carentan's 334 (`dm`).
In `dm` and `tdm` a bot that sees an enemy in a free mounted gun's arc,
with the spot behind the gun a short clear walk away, mans it, fires on
the foes in the arc and lets go after 4 s with none
(research/bot-combat.md section 14).
A bot held up for a second by a teammate standing still in its way
plans round him for 10 s, and the teammate steps aside, as CoD4's do
(research/bot-objectives.md 11.5).
`--max-clients` bounds them.

They fight as CoD1 bots do (research/bot-combat.md sections 4, 5 and
8): they hold cover spots and peek from them with CoD2's leans (0.5 of
the lean standing, as 1.5's), back off to cover when hurt or dry, hear
gunfire and blasts, and turn on an enemy who hits them from out of
sight. The hit is read off the playerstate as a CoD2 client reads it:
a new `damageEvent` is a hit, `damageYaw` and `damagePitch` the way it
came, and the attacker the last client whose `finishPlayerDamage` hurt
the bot.

A bot with a smoke grenade (CoD2's stock kit gives one to a
submachine gun or shotgun) throws it through the smoke key, on a lob
that lands on the line to an enemy it sees 400 to 1500 units off, a
third of the way out (at least 250, at least 150 short of him), when it
heads for its plant or the defuse within 800 units, covers a planting
teammate, or starts a retreat. CoD1's bots throw no smoke, so the rule
is vcod's own. From 2 s to 35 s after the burst a cloud 150 units round,
48 up, blocks every bot's sight through it (the stock
`fx/explosions/smoke_grenade.efx` volumes form 3 to 8 s in, live 30 to
40 s and spread 150 to 200 units); retail's own smoke test for the
crosshair (`SV_FX_GetVisibility`) is not ported, so a human's view is
not blocked for the crosshair names.

Before (with the harness below) and after, the
same seeds (`MAPS=mp_toujane,mp_carentan GAMETYPE=<gt> bot_match 1 2`,
six bots, 6000 ticks a seed, both seeds summed, 2026-10-22). The
harness now splits a 12 s stall as CoD1's does (`BotBody::holds`:
engaged, holding cover, retreating or on its objective on any tick of
the window is `held`), and counts the cover-hold ticks, the ticks
leaning, the retreats started, the smokes thrown and the hits from an
enemy the bot fired back at within 1 s.

| map, gametype | kills | hits / rounds | stalled, held | cover, lean ticks | smokes | answered |
|---|---|---|---|---|---|---|
| mp_toujane dm | 120 / 115 | 244 / 1320, 204 / 1049 | 0, 0 / 0, 1 159 | 17 133, 4 055 | 13 | 58% / 62% |
| mp_carentan dm | 107 / 100 | 269 / 1277, 199 / 1161 | 0, 0 / 0, 2 465 | 19 896, 4 655 | 20 | 68% / 67% |
| mp_toujane tdm | 85 / 91 | 195 / 971, 190 / 1001 | 282, 328 / 35, 1 605 | 14 825, 3 814 | 13 | 59% / 60% |
| mp_carentan tdm | 68 / 79 | 165 / 708, 154 / 791 | 0, 0 / 0, 560 | 9 831, 1 949 | 9 | 64% / 62% |
| mp_toujane sd | 60 / 43 | 135 / 456, 115 / 418 | 0, 3 979 / 0, 6 764 | 299, 77 | 23 | 36% / 53% |
| mp_carentan sd | 47 / 46 | 95 / 1035, 72 / 760 | 0, 6 434 / 0, 7 154 | 3 439, 699 | 26 | 40% / 49% |
| mp_toujane ctf | 90 / 87 | 174 / 826, 176 / 774 | 0, 7 833 / 0, 7 707 | 2 163, 1 080 | 8 | 50% / 64% |
| mp_carentan ctf | 78 / 78 | 182 / 683, 164 / 608 | 0, 7 352 / 0, 9 721 | 2 334, 476 | 7 | 60% / 58% |
| mp_toujane hq | 93 / 68 | 190 / 1059, 158 / 1012 | 0, 4 141 / 0, 5 116 | 2 858, 570 | 15 | 45% / 53% |
| mp_carentan hq | 72 / 72 | 154 / 614, 163 / 738 | 0, 7 095 / 0, 6 866 | 1 532, 286 | 11 | 44% / 67% |

`sd` plants and defuses: mp_toujane 13 and 5 before, 12 and 9 after
(rounds axis-allies 5-8, 10-2); mp_carentan 2 and 0 both times. With
the split every stalled bot-tick the earlier table counted in `sd`,
`ctf` and `hq` is a hold: before this change a guard at its site, flag
or radio, now also a bot on its cover spot; none is a stuck bot. The
282 stalled before and 35 after on mp_toujane `tdm`, and 337 after on
mp_carentan `hq`, are bots with no hold in the window.

Measured with `GAME=cod2 COD2_DIR=<install> cargo run --release -p
vcod-server --example bot_match 1 2` (six bots, 5 minutes a seed,
2026-10-22):

| map, gametype | kills | deaths not by another player | hits / rounds | stalled bot-ticks |
|---|---|---|---|---|
| mp_toujane dm | 187 | 0 | 337 / 1348 | 0 of 62 658 |
| mp_carentan dm | 179 | 0 | 277 / 1210 | 0 of 63 073 |
| mp_burgundy dm | 194 | 0 | 287 / 1168 | 0 of 62 287 |
| mp_toujane tdm | 150 | 0 | 189 / 967 | 0 of 64 464 |
| mp_burgundy tdm | 144 | 0 | 213 / 842 | 0 of 64 833 |

A stalled bot-tick is one whose last 12 s stayed within 150 units with no
target tracked. mp_toujane's bots spent 1 423 and 1 149 bot-ticks on its
ladders.

The objectives, frags and mantles, before and
after on the same seeds (`MAPS=mp_toujane,mp_carentan GAMETYPE=<gt>
bot_match 1 2`, six bots, 6000 ticks a seed, both seeds summed). The
harness drains the console before each frame since this change, as
`main.rs` does, so the script's `map_restart` ends an `sd` round; the
before column was run with that harness too. Score is axis-allies:
rounds in `sd`, captures in `ctf`, points in `hq`.

| map, gametype | kills before / after | hits / rounds before | after | score before | after | plants, defuses | frags thrown | mantles | stalled bot-ticks after |
|---|---|---|---|---|---|---|---|---|---|
| mp_toujane sd | 64 / 56 | 126 / 623 | 103 / 428 | 5-10 | 6-8 | 14, 6 | 6 | 5 | 2 978 of 40 454 |
| mp_carentan sd | 44 / 47 | 74 / 374 | 47 / 983 | 6-5 | 1-11 | 2, 0 | 22 | 36 | 2 000 of 54 701 |
| mp_toujane ctf | 95 / 90 | 155 / 764 | 145 / 657 | 0-0 | 5-1 | | 11 | 22 | 3 743 of 50 606 |
| mp_carentan ctf | 76 / 87 | 116 / 498 | 150 / 593 | 0-0 | 3-4 | | 8 | 31 | 6 101 of 51 511 |
| mp_toujane hq | 87 / 85 | 131 / 737 | 141 / 933 | 0-0 | 155-178 | | 31 | 51 | 1 682 of 51 391 |
| mp_carentan hq | 63 / 76 | 95 / 548 | 117 / 694 | 0-0 | 167-149 | | 13 | 54 | 3 455 of 54 064 |
| mp_toujane dm | 117 / 124 | 186 / 1193 | 197 / 1003 | | | | 15 | 84 | 0 of 46 133 |
| mp_carentan dm | 113 / 113 | 161 / 856 | 171 / 1156 | | | | 19 | 87 | 0 of 48 923 |

Before, no stalled bot-tick showed in any of them: the bots roamed. The
stalls after are mostly guards holding a site, a flag or the radio with
no enemy in sight, which the harness counts the same. On mp_carentan
`sd` the allies win by elimination 11 rounds of 12 and plant twice: the
axis defenders hold their sites in the open and are shot from range,
where most of the 983 rounds miss.

### Weapons and skill (2026-10-23)

The CoD1 brain fights a CoD2 gun as it reads it off the weapon file:

- It aims by the gun, not the view: the offset of the last cmd's shot
  angles (the aim block's sway, bob and gun spring, `fire_angles`) off the
  view it left from is the brain's `gun_offset`, as CoD1's reticle is, so
  a recoiling BAR or a swaying scope is led back onto the target.
- A bolt rifle's shot cycle is `fireTime` plus `rechamberTime` (kar98k
  1330 ms, not 330), so it reads as a bolt rifle: the longer trigger
  reach a bolt gets and the step back up close. The shotgun pumps too
  but stays a close gun.
- Fully down a scope (`adsOverlayReticle`) with an enemy in sight it
  holds its breath (`+holdbreath`) from a recovered breath for 3 s,
  short of `player_breath_hold_time`'s 4.5 s and its gasp.
- With every primary dry and no enemy in sight it walks to the nearest
  loaded gun lying within 600 units on a clear walk, the dry gun in
  hand, and taps use once the cursor hint names it (the swap drops the
  dry one); a gun not taken in 6 s is passed over for 20 s, and the
  CoD1 brain's suicide when dry waits while it fetches.
- Stances are CoD1's: a crouch now and then in a fight and a lie-down
  at range on the sights. SMGs and the automatic rifles fire CoD1's
  bursts (2 to 7 ticks close, 1 to 3 past 800 units), semi-automatics
  tap at `fireTime`.

`bot_match GAME=cod2` now splits kills, hits, rounds, the share fired
fully sighted and the time from the killer's first hit to the kill by
weapon class and by skill (`SKILLS=a,b,...` one level a bot,
`AXIS_SKILL=`/`ALLIES_SKILL=` by side), counts each life's spawn weapon
and the guns picked up. A lethal hit logs `K` alone and counts as a hit;
a shotgun's pellets each count. Before and
after, `MAPS=mp_toujane,mp_carentan,mp_burgundy`, seeds 1 and 2, six
bots, 6000 ticks, `dm`, `tdm` and `sd` summed:

| class | kills before / after | hits / rounds before | after | sighted before / after |
|---|---|---|---|---|
| smg | 332 / 170 | 1130 / 4770 (0.24) | 576 / 2526 (0.23) | 59% / 58% |
| auto rifle (BAR, bren, mp44) | 126 / 66 | 447 / 1907 (0.23) | 242 / 1059 (0.23) | 64% / 64% |
| semi (garand, carbine, g43, svt40) | 80 / 123 | 191 / 782 (0.24) | 309 / 981 (0.31) | 78% / 74% |
| bolt | 12 / 121 | 13 / 51 (0.25) | 156 / 422 (0.37) | 69% / 75% |
| sniper | 13 / 77 | 16 / 66 (0.24) | 87 / 242 (0.36) | 88% / 90% |
| shotgun | 64 / 58 | 252 / 350 | 196 / 318 | 67% / 74% |
| pistol | 1 / 8 | 5 / 39 | 27 / 111 | 41% / 27% |

All kills 695 before, 725 after. Before, each bot drew its weapon on its
own and the draws bunched: of 340 `dm` lives 118 were on the mp40 and
none on a kar98k; after, 351 lives spread over ten weapons (kar98k 61,
mp40 61, kar98k sniper 57, thompson 45, down to the enfield scope's 10). Median time-to-kill holds at 400 to 650 ms for the
automatics and 200 to 400 for the semi-automatics; a bolt rifle's or a
scope's median is 0, the first hit killing. No bot picked up a gun in
these runs: none ran every primary dry. `a_dry_bot_fetches_a_dropped_gun`
holds the fetch.

The skill levels, the same seeds (kills, deaths, hits / rounds):

| run | level 0 | 1 | 2 | 3 |
|---|---|---|---|---|
| `dm`, eight bots two a level, before | 69, 119, 162 / 481 | 103, 110, 260 / 803 | 125, 101, 322 / 983 | 134, 102, 216 / 693 |
| same, after | 59, 120, 140 / 526 | 107, 105, 302 / 844 | 123, 108, 309 / 970 | 145, 101, 312 / 1061 |
| `tdm` axis 0, allies 3, before / after | 39 / 57 kills | | | 207 / 182 kills |
| `tdm` axis 3, allies 0, before / after | 55 / 80 kills | | | 194 / 184 kills |

Each level kills more than the one below and an expert side takes 70 to
84% of a `tdm`'s kills. Accuracy does not rise with the level (0.27 to
0.36 a level, the weapon drawn moves it more): the levels change the
reaction, turn, aim error and memory (research/bot-combat.md section 3),
and on CoD2's sights the gun's spread and kick are most of a miss. The
presets are CoD1's, shared, and left as they are.

## CoD4

`vcod-server <map> --game cod4 --bots N [--bots-shoot]` adds `N` bots to
a CoD4 1.7 server once the level is up, named as on CoD2, alternately
`allies` and `axis` (`autoassign` in `dm`). They answer the stock menus
as a client's buttons do (`_menus.gsc`): the team menu with their team,
then the class menu the script opens (`changeclass_marines`,
`changeclass_opfor`) through `changeclass` with a stock class, the one
the fewest other bots carry (assault, spec ops, heavy gunner,
demolitions, sniper). The brain is CoD1's, as on CoD2: the shared driver
and follower over the shared graph, which floods the map's clip map from
its spawn points and adds the mantles 1.7's own pmove climbs (mp_crash:
8 458 nodes, 1 222 mantle edges, every one of its 177 spawns in one
component). Its cmds are rewritten into 1.7's button bits (attack 0x1,
melee 0x4, use 0x8, reload 0x10, leans, the two stances, the sight 0x800,
jump 0x400, the frag key 0x4000 for a throw) with the moves whole, as 1.7's
wire carries only their sign, and run through the level's `ClientThink`.
1.7 numbers every attachment as a weapon of its own, past the brain's 63,
so each bot's weapons are renumbered for its brain in the order it came
to hold them (`bots::cod4::Arms`). A new level (a rotation, `map`,
`map_restart`, `fast_restart`, an `sd` or `sab` round) keeps the bots;
each answers the menus the level opens again. rcon's `addbot`,
`removebot` (`kickbot`), `listbots` and the `vc_bot_*` cvars work as on
CoD1. They play the objectives with the shared brain
(`docs/research/bot-objectives.md` 12): in `sd` and `sab` the team's bot
nearest a lying bomb walks into it, its carrier plants at a zone through
the use key and the briefcase while the rest guard the zone, the bot
nearest a planted bomb on the other side defuses it, and the planters
guard it; in `dom` they take flags as in United Offensive, and in `koth`
they gather at the radio, take it by standing in it and guard it, each
zone stood in from a floor inside its trigger. They throw frags through
the frag key as CoD2's bots do, and a dead bot that follows a teammate
is left alone (bot-objectives.md 12.4). A bot climbs a mantle edge once,
crouches when a climb forces it (`EV_STANCE_FORCE_CROUCH`, as a
player's client does) until it can stand, and a throw a death cut short
does not carry into the next life (12.5). The graph keeps a mantle only
where a bot run straight at its landing is offered the climb, and a bot
in a `dom` flag or the `koth` radio that the other team stands in too
(the capture held) walks the zone until one side is dead or gone
(12.6). A bot held up for a second by a teammate standing still in its
way plans round him for 10 s, and the teammate steps aside, out of a
door he held (12.7). On a mantle edge a bot runs at the landing the
graph's climb reached, not at the node past it, and a zone's spot comes
from the floor the spawns stand on, never from grounds out of bounds
that a ladder's ends flooded (mp_shipment's yard; 12.8). A few jump
edges the graph proved under CoD1's pmove are past a CoD4 body, and a
bot jumps at them until it gives up (12.9); the graph keeps no leap
(12.14), no edge but a mantle longer than the follower's reach, and
clips the level's solid brush entities as a bot's pmove does
(mp_cargoship's `pipe_shootable`), and a CoD4 bot climbs on past a
ladder's top as a CoD2 one does (12.15). The graph probes for mantles again off the floors each mantle's top leads to, a bot takes a mantle down from rest facing the wall, and a teammate standing still on a stair's top above a bot holds it up as one beside it does (12.16). The way round a still teammate keeps off every node a body's box there would meet, not only the nearest (12.17). A bot that walked its plan out to an objective's spot and comes no nearer it in 3 s, the objective not taken, guards round the spot for 15 s before it tries again (12.19). `VC_NAV_COD4=1` walks the graph under
1.7's own pmove instead, the map's script brush models clipped; over
six seeds of all maps it kills 0.4% more and stalls 3% more, so it is
off (12.10 to 12.12).

Where no objective deals out roles (`war`, `dm`), a CoD4 bot mans the
stock maps' mounted SAWs (cod4-gsc.md 9.2; `bots::cod4::gun`): it takes
a free gun whose arc covers the enemy it fights, or a lane its team saw
an enemy down in the last 8 s, past 250 units from the gun, when the
spot behind the gun is within 400 units, level and a clear walk. With
no enemy seen for 20 s it also takes a gun on another floor whose spot
lies within 800 units and on the graph, when no other bot of its team
stands nearer the spot, walking there over the graph once it has
nothing else to do (no objective, fight or lost enemy to look for) and
giving up after 15 s; a team sends one bot that way to a gun at most
every 3 minutes, and none for 3 minutes after its gunner let the gun go
with nobody in its arc. It
taps use once the cursor hint names the gun, turns the barrel 12
degrees a tick onto the nearest enemy in the arc it can see and fires
within 2 degrees. It lets go when flanked (the enemy it sees, or one
that hit it in the last 300 ms, outside the arc within 1 200 units) or
after 3 s with nobody in the arc, and takes no gun for 20 s after. A
live frag under its hint with 1.5 s or more on the fuse is taken with
the frag key and thrown at once back toward its thrower, else the
enemy in sight, lofted 12 degrees (cod4-gsc.md 9.3). `GUNS=0` and
`THROWBACK=0` on `bot_match` turn either off; its line counts
`mounts` and `throwbacks`.

Six bots, `war`, 6 000 ticks, seeds 1-4, 2026-10-26, before (`GUNS=0
THROWBACK=0`) and after:

| map | kills before | kills after | hits before / after | mounts | throw backs | stalled bot-ticks before / after |
|---|---|---|---|---|---|---|
| mp_backlot | 44 44 45 47 (180) | 44 44 47 47 (182) | 687 / 679 | 0 | 2 | 0 / 0 |
| mp_carentan | 42 46 60 40 (188) | 54 43 47 48 (192) | 722 / 775 | 14 | 3 | 34 / 34 |

No bot manned mp_backlot's two SAWs in these seeds: both stand
upstairs (the spot behind the crouch SAW 393 at z 258, the ground floor
at about 70), and the pick took only a gun a straight walk on the bot's
own floor reaches. Of 135 473 picks a bot weighed on mp_backlot over
seeds 1-4, 124 992 were past 400 units and 10 480 a floor off. The walk
over the graph to a gun on another floor, `war` and `dm`, six bots,
6 000 ticks, seeds 1-12 summed, 2026-10-26, before (the same tree with
the walk off) and after:

| gametype | map | kills before / after | hits before / after | mounts before / after |
|---|---|---|---|---|
| dm | mp_backlot | 601 / 612 | 2 250 / 2 345 | 0 / 9 |
| dm | mp_carentan | 639 / 615 | 2 246 / 2 175 | 43 / 50 |
| war | mp_backlot | 509 / 509 | 1 939 / 2 006 | 1 / 5 |
| war | mp_carentan | 559 / 530 | 2 081 / 2 070 | 46 / 45 |

2 266 kills against 2 308, 1.8% fewer. `war` mp_carentan's 29 fewer
come with one mount fewer: a single walk sends a match down another
course, so a row moves about 5% on its own. A first cut (any lane seen
in the last 8 s, no wait since the last enemy, any bot, 60 s between
walks to an idle gun) mounted mp_backlot's guns 58 and 25 times and cost
6% of the kills, 12% on `war` mp_carentan; walks up to 1 500 units
ahead of a chase cost a third of `war` mp_carentan's. Before any of
this, a first cut with no rest after a release and a 15 s lane memory
mounted 208 times in one mp_carentan seed, the gunner idling off and on
the same gun; it cost 35 of that map's 188 kills and 11 224 stalled
bot-ticks, which the 20 s rest removed.

Since CoD4 rounds and blasts damage the stock maps' destructible cars
(`cod4-combat.md` 13), a car stops a round: over the same twelve seeds
the four rows above killed 1 982 against 2 266 while the bots' sight
took no car into account and they fired into cars between them and
their enemy. A bot now sees an enemy only past every car that would
stop its round (`Cod4Bots::shot_clear`, the gunner's arc too): 2 255
kills (dm mp_backlot 581, mp_carentan 586; war mp_backlot 527,
mp_carentan 561) and 8 439 hits against 8 596, no script error.

Since a CoD4 round leaves from the eye with its walk bob and lean along
the gun's angles (the sway, the movement tilt, the idle drift, the
recoil spring; `cod4-combat.md` 3.2) and goes on through script models
and brush models (13.1), the same twelve seeds, 2026-10-27, before
and after:

| gametype | map | kills before / after | hits before / after | accuracy before / after |
|---|---|---|---|---|
| dm | mp_backlot | 576 / 562 | 2 203 / 2 170 | 0.238 / 0.228 |
| dm | mp_carentan | 579 / 601 | 2 049 / 2 228 | 0.248 / 0.270 |
| war | mp_backlot | 526 / 518 | 2 002 / 1 996 | 0.242 / 0.232 |
| war | mp_carentan | 561 / 565 | 2 045 / 1 999 | 0.286 / 0.326 |

2 246 kills against 2 242 and 8 393 hits against 8 299, no script
error. The bots do not compensate for the sway, but a bot holds its view
on its enemy, so the turn sway and the movement tilt stay small; the
cars' sight test took a car's bounds as stopping a round.

The sight test now traces a car's collision triangles, a part
`hidePart` took away (a broken window) skipped as a round skips it
(`Cod4Bots::shot_clear`, `cod4-combat.md` 13.1), so a bot sees and
shoots through a car's empty window frames. The same twelve seeds,
2026-10-27, before / after:

| gametype | map | kills | hits | accuracy |
|---|---|---|---|---|
| dm | mp_backlot | 562 / 571 | 2 170 / 2 275 | 0.228 / 0.242 |
| dm | mp_carentan | 601 / 622 | 2 228 / 2 258 | 0.270 / 0.273 |
| war | mp_backlot | 518 / 510 | 1 996 / 1 985 | 0.232 / 0.240 |
| war | mp_carentan | 565 / 581 | 1 999 / 2 119 | 0.326 / 0.314 |

2 284 kills against 2 246 and 8 637 hits against 8 393, no script
error. A round still goes through a car's intact panels at the damage
left (13.1), which the sight does not count on.

`GAME=cod4 COD4_DIR=<install> cargo run --release -p vcod-server
--example bot_match` measures them (`MAPS=`, `GAMETYPE=`, `SET=` before
the load) and adds a line per level and every script error the levels
raised (`LEAVE=<tick>,...` drops a bot at each tick, a client leaving
mid-match), and counts the objectives' changes off the scripts (bomb
`pickup`, `plant`, `defuse`, `explode`, zone `capture`; `OBJ=0` keeps
the bots off the objectives; `STALLS=1` prints every 20th tick of a
stalled sample: the bot, its spot, `pm_flags`, ground and mantle state);
`GAME=cod4 ... --example nav_build [map...]` prints the graph's
line, built over the clip map alone (a live level adds its brush
entities). Six bots, two-minute limits (`sd` one-minute rounds, two a match;
`sab` two two-minute rounds), a rotation of mp_backlot, mp_strike and
mp_crash from mp_crash, 8 000 ticks (6 min 40 s), a bot dropped at ticks
1 500 and 4 500, seed 1, 2026-10-25. `sv_pure 0` and `sv_pure 1` (ranked,
the default) end and rotate alike; the `sv_pure 0` arm:

| gametype | levels (map loads, round restarts) | kills | hits / rounds | stalled bot-ticks |
|---|---|---|---|---|
| war | 2, 0 | 20 | 59 / 936 | 0 of 29 720 |
| dm | 2, 0 | 31 | 119 / 673 | 1 of 28 188 |
| sd | 2, 3 | 14 | 59 / 445 | 315 of 28 883 |
| dom | 2, 0 | 20 | 78 / 546 | 97 of 29 747 |
| sab | 1, 0 | 21 | 49 / 332 | 2 220 of 30 121 |
| koth | 2, 0 | 20 | 92 / 692 | 1 977 of 29 772 |

In that soak `koth` raised a script error on each level's first spawns,
which read spawn points the per-frame update had not reached yet. Retail
1.7 raises the same error for a client that spawns that early, so the
bots now enter a fresh level 3 s past its first frame, as a loading
client does, and their first spawns run clean (`cod4-gsc.md` 8). About
two thirds of each level is play, the rest the prematch
countdown and the intermission. The soak above predates the objectives
and the frags; bot-objectives.md 12.3 and 12.4 have the later runs.

## Known limits

- `status` shows a bot's address as `127.0.0.1:-1`.
- In UO, bots drive vehicles along the infantry graph over ground the hull
  fits, so a tank can still wedge itself; a driver backs off and gets out
  after four stalls or 8 s without headway, a rider with nobody to shoot
  gives up the same 8 s, and every rider gets out of a
  hull that has scraped something for 5 s, before the engine's 7 s stuck
  kill. A tank sliding into a wall with nobody aboard still destroys
  itself. They never change seats, use the hatch or drive a
  vehicle at an enemy to run him over. A bot fires rockets at a vehicle
  only with a launcher already in hand; outside `bas` none fetches one.
- Skill levels change how bots aim, react and remember, and how often they
  move on their sights in a fight, not where they go or what they choose
  to do: an expert takes the same routes and plays the objectives the same
  way as an easy bot.
- A bot picks its cover against the one enemy it is fighting; another enemy may still see it there. It takes no cover while planting, carrying or holding a capture zone, nor within 300 units of the enemy.
- A bot in cover swaps between the spot's two ways out (both lean sides, or standing and a crouched lean) from one peek to the next, sometimes moves to the next spot over while it reloads, and holds the spot for a time that depends on its skill once its enemy is out of sight. To go after an enemy it lost, it heads for a nearby spot that looks onto where he was, not onto that spot itself, and gives up sooner the higher its skill.
