# Bots

vcod-server can fill a server with bots in all four games: CoD 1.1 and
1.5, United Offensive, CoD2 1.3 and CoD4 1.7. Retail has none, so
everything here is vcod's own. A bot is a real client: it takes a slot,
joins through the stock menus, and shows in the scoreboard and in
`status` like anyone else. Retail clients see bots as ordinary players.

This page covers running bots, [how a bot thinks](#how-a-bot-thinks) and
[what each game's bots do](#per-game). Navigation is in
[research/bot-navigation.md](research/bot-navigation.md), objective play
in [research/bot-objectives.md](research/bot-objectives.md), combat and
the measuring harness in [research/bot-combat.md](research/bot-combat.md).

## Quick start

Six fighting bots:

```
vcod-server mp_carentan --gametype tdm --bots 6 --set rconPassword=secret
```

The same on the other games (the install picks the game, or pass
`--game`):

```
vcod-server mp_cassino --mod-dir uo --gametype tdm --bots 6
vcod-server mp_toujane --game cod2 --gametype tdm --bots 6
vcod-server mp_crash --game cod4 --bots 6
```

A server that keeps itself busy: with a floor of 10, an empty server runs
a 5v5 of bots, and each human who joins takes a bot's place.
`--max-clients` defaults to 8, so raise it above the floor.

```
vcod-server mp_carentan --gametype tdm --max-clients 16 --set vc_bot_minplayers=10 --set rconPassword=secret
```

Everything below also works on a running server over rcon, from a retail
or vcod console: `rconPassword secret` then `rcon <command>` on CoD1 and
UO, `rcon login secret` then `rcon <command>` on CoD2 and CoD4.

## Command line

| Flag | Does |
|---|---|
| `--bots N` | Start with N bots. Each takes a real slot, so N can't exceed `--max-clients`. |
| `--bots-peaceful` | Keep those bots from firing: they walk the map and play objectives but never shoot (`vc_bot_shoot 0`). The old `--bots-shoot` still parses and changes nothing. |
| `--set NAME=VALUE` | Set any cvar below before the first map loads. |

The bots join once the level is up. The navigation graph builds on its
own thread on the first tick, a few seconds on a big map; bots wander
until it is ready. The server keeps each graph it built on disk
(`--nav-cache`, [usage.md](usage.md#server)), so the next start of the
same binary on the same map loads it in milliseconds.

## rcon commands

rcon is refused until the server has a password (`rconPassword`;
`rcon_password` on CoD2 and CoD4).

| Command | Does |
|---|---|
| `addbot [allies\|axis\|auto] [count] [skill N]` | Adds `count` bots (default 1). The words can come in any order. `auto`, the default, puts each bot where balance asks. A named team puts the bots there and pins them: balance never moves them. `skill N` (0-3) fixes their [skill](#skill); `vc_bot_skill` then leaves them alone. |
| `removebot [name\|all\|count]` | No argument removes one bot. A name removes that bot (the `[BOT] ` prefix optional, case ignored), `all` every bot, a number that many, lowest score first. CoD2 and CoD4 also take `kickbot`. |
| `listbots` | Every bot's slot, team, score, pin, skill level and name, then the human and bot counts and the `vc_bot_*` values. |

```
rcon addbot 4
rcon addbot axis 2
rcon addbot allies 3 skill 3
rcon removebot NadeSpammer
rcon removebot all
rcon listbots
```

## Cvars

Set them with `rcon set <cvar> <value>` (or `seta`), or `--set` at start.
A change applies within half a second.

| Cvar | Default | Does |
|---|---|---|
| `vc_bot_minplayers` | 0 | [Minimum player count](#minimum-player-count). 0 is off. |
| `vc_bot_autobalance` | 1 | [Keeps the teams even](#team-balance) in team gametypes. |
| `vc_bot_shoot` | 1 | 0 stops bots firing. `--bots-peaceful` sets it to 0. |
| `vc_bot_names` | 1 | 0 names bots that join from then on `bot<slot>`; bots already in keep their names. |
| `vc_bot_skill` | 1 | The [skill level](#skill), 0 to 3, of every bot `addbot ... skill N` didn't set. |
| `vc_bot_retreat` | 1 | 0 stops bots breaking off a fight when hurt or dry and walking to health items; a retreat under way ends. |
| `vc_bot_vehicles` | 1 | United Offensive: 0 keeps bots out of vehicles: none gets in, and a bot already in one gets out. |

## Skill

Four levels set how fast a bot reacts and turns, how well it aims, how
long it remembers a lost enemy, how far it hears and how wide it sees:

| Level | Name | Reaction | Turn rate | Walking turn | Aim error at first sight, settled | Hearing | Sight cone |
|---|---|---|---|---|---|---|---|
| 0 | easy | 400-650 ms | 200°/s | 360°/s | 64, 5 units | 0.75x | 100° |
| 1 | normal (default) | 200-350 ms | 300°/s | 480°/s | 36, 4 units | 1x | 120° |
| 2 | hard | 150-300 ms | 440°/s | 600°/s | 32, 2.5 units | 1.25x | 140° |
| 3 | expert | 100-150 ms | 600°/s | 720°/s | 24, 1.5 units | 1.5x | 160° |

- The aim error is how far from the aimed point a shot would land on the
  mean. It shrinks every tick the bot stays on the target and grows with
  range and while the bot runs. It drifts slowly round the point rather
  than jumping about.
- The aimed point sits somewhere on the upper body, drawn once per
  engagement. A target running across the view is trailed, and when he
  turns back the aim runs on past the turn for a moment; both shrink with
  skill.
- The turn onto a target is a fast flick that lands a little short or
  past, then a smaller correction. A higher level turns faster and lands
  closer. The turn rate in the table is a flick's mean; it peaks near
  twice that.
- The bot pulls the gun's sway and recoil back part of the way, a few
  ticks late: half of it 200 ms late at easy, 90% of it 100 ms late at
  expert.
- A higher level also moves on its sights more often (a slow strafe, a
  bolt rifle's back-off, a lie-down at range) and remembers a lost enemy
  and a heard shot longer.
- Easy also lets the trigger up for a moment after every burst or shot,
  so it fires a fifth fewer rounds (bot-combat.md section 3.2).
- A UO vehicle's guns react and miss by the same numbers, the vehicle's
  speed counting for the bot's run, so a higher level kills more from a
  machine gun seat (bot-objectives.md section 10.15).
- Otherwise the levels change only these numbers. Routes, objective
  play and choices are the same at every level: an expert is faster and
  sharper, not smarter.
- `vc_bot_skill` changes every bot without its own level at once,
  mid-fight included. To mix levels, give each `addbot` its own:

```
rcon addbot axis 4 skill 0
rcon addbot allies 4 skill 3
```

How the levels play against each other:
[research/bot-combat.md](research/bot-combat.md) section 3.

## Minimum player count

With `vc_bot_minplayers` at N the server keeps N players in the game,
topping up with bots:

- No humans: N bots, split across the teams.
- A human picks a team: one bot leaves, off the larger team.
- A human leaves: a bot comes back.
- N or more humans: no floor bots.

A human counts only once they pick a team; a spectator or someone in the
team menu does not push a bot out. Bots from `addbot` or `--bots` count
separately: `removebot` lowers that count, but the floor still tops the
server back up to N.

## Team balance

With `vc_bot_autobalance 1`, in every team gametype (all but `dm` and
1.1's `bel`), new bots join the smaller team and bots move so the teams
differ by at most one.

- Humans never move.
- Bots pinned with `addbot allies` or `addbot axis` never move.
- A bot switches only while dead or spectating, never during the
  intermission. In `sd` and `re` a bot killed mid-round switches at once
  and, as a human who changes team while dead, sits the round out and
  spawns on its new team in the next.

## Slots

- Bots never take a private slot (`sv_privateClients`).
- `addbot` may fill every public slot; the minimum player floor always
  leaves one free, so a human can still join and push a bot out.
- `sv_maxclients` (up to 64) counts bots. It is latched, as on retail:
  `rcon set sv_maxclients 32` then `rcon map_restart`. Players stay
  connected through the restart.

## Names

A bot is named `[BOT] <name>`, drawn from a pool of over a hundred
(`[BOT] Sir Camps-a-Lot`, `[BOT] NadeSpammer`, `[BOT] PronePatrol`). No
two connected bots share a name; past the pool's unused names a number
is added (`[BOT] NadeSpammer 2`). Every name fits retail's 31-byte limit
with the prefix and a two-digit number. The draw has its own random
generator, so `vc_bot_names` never changes how a seeded server plays.
`status` prints a bot's address in retail's form for a test client.

## How a bot thinks

The brain is hand-written rules, with no learning and no per-map script,
and all four games share it. Retail has no bots, so vcod's own seeded
matches judge a change: it lands only when stalls drop and kills hold
([research/bot-combat.md](research/bot-combat.md) section 1).

Each server frame (50 ms) the server fills a view for every bot, and the
brain answers with one usercmd: move keys, view angles, buttons and the
selected weapon. That cmd goes through the game's own movement code, the
path a human's cmd takes. The brain reads the view and nothing else, so
it cannot see through a wall or skip a physics rule. The view holds the
bot's own state, the enemies a line-of-sight trace reaches inside its
sight cone, the gunfire it hears and the objective's state.

Each tick a bot does the first of these that applies:

1. Dead: it waits 1.5 s, then presses use once a second until the stock
   script respawns it.
2. Spectating or at intermission: it presses nothing.
3. In a UO vehicle seat: the vehicle code drives or guns.
4. Stuck for 10 s where the graph knows no way out, or with nothing to
   fire for 10 s: it kills itself to respawn, except where a death sits
   out the round (`sd`, `re`).
5. In a grenade throw: it switches to the grenade, pulls back, cooks
   where the game has a cook, throws and switches back.
6. Otherwise it fights an enemy, plays the objective, or roams toward
   fights.

### Fighting

- It spots enemies inside its sight cone and keeps the one it fights
  while he stays in sight. Whoever just hit it comes first, and gunfire
  it hears draws it from any direction (bot-combat.md section 8).
- A new target costs a reaction delay. It fires before its aim settles,
  and on CoD1 and CoD2 it aims by where the gun points, pulling sway and
  recoil back late and in part, as a hand does (section 18).
- Around the shooting it strafes, takes cover behind low walls, retreats when
  hurt, and peeks for or hunts an enemy it lost (sections 4, 5 and 10).
- It throws frags at an enemy behind cover or one just lost, and runs
  from a live frag near it. With friendly fire on it throws none that
  would burst by a teammate (section 12).

### Getting around

- At map load the server builds a navigation graph from every spawn
  point. Each link (walk, jump, drop, ladder, mantle) goes in only after
  a simulated body has travelled it under the game's movement rules
  ([research/bot-navigation.md](research/bot-navigation.md) section 2).
- A bot follows its path with the keys a player would press, and takes a
  risky drop only when no other route exists (bot-navigation.md section
  3). Paths and probes are worked out on up to four threads without
  changing how any bot moves ([usage.md](usage.md#environment-variables)).
- It plans round a teammate standing in its way or a parked UO vehicle,
  and on CoD1, 1.5 and UO round any body. Two bots meeting head-on with
  no way round take turns, on a ladder too (section 3.8).

### Team play

Each gametype has its own goal: take and carry the bomb or flag, plant
and defuse, capture and hold a flag or the radio. An enemy in sight
takes over, and the bot goes back to the objective after the fight. A
team spreads its bots over different routes
([research/bot-objectives.md](research/bot-objectives.md) section 9).
Guards take their own posts round an objective, most of them farther out
on the ways in, and half walk back in when it comes under attack
(sections 13 and 14). No post stands where a guard's body would close a
door or a corridor one body wide (section 4.2.1).

### Yells

On CoD 1.1, 1.5, UO and CoD2 a bot yells through the stock quick
messages, the rows a player picks from the `V` menu, so every client
hears the team's voice line, retail ones included: "Follow me!" or "Move
in!" as an `sd` or `re` round starts, "Enemy spotted!", "Grenade!" and
"Enemy down!". Not every time, and one voice a team at a time
(bot-combat.md section 21). CoD4 bots send none.

## Per game

Behaviour differs by game, never by map.

### CoD 1.1, 1.5 and United Offensive

- Gametypes: every stock one, `dm`, `tdm`, `sd`, `re` and `bel`, and
  UO's `ctf`, `dom`, `hq` and `bas` (bot-objectives.md sections 4 to 7).
- In Retrieval the team escorts the carrier and holds the goal ahead of
  him; of the defenders, the one nearest the goal holds it, until another
  is well nearer, while the rest go after the carrier. A dropped objective the use key cannot reach is
  waited for where it goes back.
- In `bas` a bot throws a frag into a charge room where a teammate just
  died before it goes in, when it finds a lob in.
- UO grenades are cooked; 1.1 has none to cook. A UO bot sprints only on
  a level, straight run.
- UO vehicles: a bot with nothing nearer to do boards a vehicle near it.
  Drivers head for the objective or an enemy and wait for a teammate
  walking up. A tank stops to shell enemies, and in `bas` the other
  side's bases. A jeep's driver runs over an enemy on foot in front of
  it. Gunners fire the machine gun and take the driver's seat when it
  empties (in a jeep only with nobody to shoot), and a bot on foot fires a rocket
  launcher at an enemy-crewed vehicle (bot-objectives.md sections 10,
  10.12 to 10.14).
- A bot on foot keeps out of a burning hull's reach until its second
  blast (section 10.10).

Cannot:

- Drive anywhere but along the infantry graph. A hull's route keeps to
  the graph's open ground and off the other vehicles where they stood
  when it planned, but it does not give way to one on the move, so a
  tank can still wedge itself. Its crew gets out before the engine's
  stuck kill, but a tank sliding into a wall with nobody aboard still
  destroys itself (sections 10.3, 10.4, 10.13 and 10.16).
- Change seats other than to an empty driver's seat, or use the hatch.
- Fetch a rocket launcher outside `bas`.
- Throw a satchel or call artillery at a `bas` base.

### CoD2

- Gametypes: every stock one, with objective play in `sd`, `ctf` and
  `hq` (bot-objectives.md section 11).
- Menus: it answers the gametype, team and weapon menus, taking the
  weapon the fewest other bots carry. A new level keeps the bots, and
  each answers the menus again (bot-objectives.md section 8.1).
- Votes: bots never vote and are not counted as voters, so one player's
  `callvote` plus his own yes passes on a server full of bots
  ([research/cod2-protocol.md](research/cod2-protocol.md) section 3.4.2).
- Movement: the graph floods CoD2's collision under 1.5's movement rules
  and adds every mantle CoD2's own pmove climbs.
- Weapons: bolt rifles fight as bolt rifles, automatics fire bursts, and
  a scoped bot holds its breath. Frags go through the offhand key. Smoke
  is thrown as cover and blocks every bot's sight, but not a human's
  crosshair names (bot-combat.md section 22).
- It mans mounted guns in `dm` and `tdm` (bot-combat.md section 14),
  and with every gun dry it takes a loaded one lying near it.

### CoD4

- Gametypes: `war`, `dm`, `sd`, `sab`, `dom` and `koth`.
- Menus: the team menu, then the class menu with a stock class, the one
  the fewest other bots carry. A new level, round or rotation keeps the
  bots (bot-objectives.md section 8.1).
- Objectives: the bomb in `sd` and `sab`, the flags in `dom`, the radio
  in `koth` (bot-objectives.md section 12).
- Movement: the graph floods the clip map under 1.5's movement rules and
  adds the mantles 1.7's own pmove climbs. It keeps no leap.
- It throws frags, and throws live ones back.
- It mans mounted SAWs in `war` and `dm`, and a destructible car blocks
  its sight where it would stop a round (bot-combat.md section 23).

Cannot:

- Call in hardpoints (UAV, airstrike, helicopter) or use special
  grenades.
- Compensate for gun sway.
- Make a few jump edges the graph proved under CoD1's movement rules; a
  bot tries them until it gives up (bot-objectives.md section 12.9).

Kills and accuracy measured on CoD2 and CoD4:
[research/bot-measurements-cod2-cod4.md](research/bot-measurements-cod2-cod4.md).

## Known limits

- A bot picks cover against the one enemy it fights; another may still
  see it there. It takes no cover while planting, carrying or holding a
  capture zone, nor within 300 units of the enemy.
- Cover is a low wall it ducks behind, never a corner it leans round
  (bot-combat.md section 4.7). It swaps between standing and a crouched
  lean from one peek to the next where the spot has both, and sometimes
  moves to the next spot while it reloads. Once its enemy is out of
  sight it holds the spot for a time set by its skill. To chase a lost
  enemy it heads for a nearby spot that looks onto where he was, and
  gives up sooner the higher its skill.
