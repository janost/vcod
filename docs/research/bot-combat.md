# Bot combat and the match harness

How well vcod's bots fight, and how a change to them is measured. Retail
has no bots, so nothing here is checked against a retail binary: the
yardstick is vcod's own seeded matches. The brain is
`crates/server/src/bots.rs`; movement is in `bot-navigation.md`,
objectives in `bot-objectives.md`.

Contents: 1 the harness, 2 baseline, 3 skill levels, 4 cover and lean,
5 retreat, 6 one table of every arm, 7 team spread, 8 target choice and
sight, 9 aim and footwork, 10 varied peeks and hunts, 11 the walking
view, 12 frags, 13 broken fights, 14-17 CoD2 bots (guns, tick order,
pmove walks, the pass's rules and mp_ship's ladders), 18 the hand's aim.

## 1. The harness

`cargo run --release -p vcod-server --example bot_match [seed...]` runs one
in-process match per map and seed (default 1..=8), every bot firing, on
the stock gametype scripts, until the script calls `exitLevel` or `TICKS`
(12000, ten minutes of 50 ms ticks) run out. `JOBS` (4) matches run at
once; the nav graph is built once and shared. It is the one bot measuring
tool (`team_tally`, `cover_ab` and `bot_fights` are folded in); a new
measure goes in it. A match is deterministic in its seed. The example
passes `2 * seed + 1` to `Server::test_seed_rng`, which sets the low bit
itself, so neighbouring seeds don't replay one match.

Knobs:

| Knob | Default | Effect |
|---|---|---|
| `MAPS=` (`MAP=`) | mp_carentan | comma list |
| `GAMETYPE=` | tdm | |
| `BOTS=` | 6 | |
| `TICKS=`, `JOBS=` | 12000, 4 | |
| `SET=name=value,...` | | any cvar, e.g. `scr_tdm_timelimit=5` |
| `UO=1` | | a United Offensive server (`COD_DIR` at a UO install) |
| `VERBOSE=1` | | every kill line |
| `AXIS_SKILL=` / `ALLIES_SKILL=` | | add the `BOTS` through `addbot <side> <n> skill <level>`, half a side |
| `SKILLS=a,b,...` | | one bot per entry, by balance (for `dm`) |

Without a skill knob the bots join as `--bots` does, at `vc_bot_skill`.

Feature arms, each `all` (default), `none`, `axis`, `allies` or `alt`
(axis on odd seeds, allies on even, so the map's asymmetry cancels over
an even seed count). Off means:

| Arm | Section | Hook | Off |
|---|---|---|---|
| `COVER=` | 4 | `Server::test_bot_cover` | no cover spots |
| `RETREAT=` | 5 | `test_bot_retreat` | no retreat |
| `TEAMPLAY=` | `bot-objectives.md` 9, section 7 | `test_bot_team_play` | a bot ranks by slot, every attacker keeps by an `re` carrier, nobody covers a planter, plans pay no crowd cost, a carrier fights as any bot, every noise draws the bot |
| `AWARE=` | 8 | `test_bot_aware` | sees all round, fights the nearest enemy in sight, pays full reaction and error on every switch, ignores hits from out of sight |
| `FOOTWORK=` | 9 | `test_bot_footwork` | stands still on its sights, never backs off, never lies down |
| `WARY=` | 10 | `test_bot_wary` | peeks from a spot the one way, peeks on through the 3 s hold, walks onto a lost enemy's spot until it forgets it |
| `AIM=` | 18 | `test_bot_human_aim` | the aim before section 18: straight turns, an error resampled every 4-8 ticks, every sway and kick taken back at once |

A default arm never calls its hook, so an all-default run plays the stock
game bit for bit (checked: seeds 1 and 2 print the lines of the harness
before the arms). `dm` has no sides: there an odd slot takes the arm
`axis` would and an even slot `allies`'s, so `alt` swaps them by seed.
With an arm one-sided each match prints an `arm kills` line.
With an arm one-sided the totals add a row per arm, named by the
one-sided features on for it (`control` for none), and the wins and
actions by arm.

Everything it counts comes off the script's `logPrint` lines (the
`games_mp.log` lines a retail server writes) or the bot bodies:

| Column | From | Meaning |
|---|---|---|
| kills, deaths | `K;` | A kill by another player. A suicide (attacker is the victim or the world, `-1`; on the victim's row) and a team kill (same team, not `dm`) are counted apart and are not kills. World damage (a fall) is left out. |
| damage | `D;`, `K;` | Damage dealt to others. A non-lethal hit logs `D;`, the lethal one `K;` alone. |
| ttk | `D;`, `K;` | Time to kill: from the attacker's first hit on that victim in the victim's life to the kill, in ms. A one-hit kill is 0. Mean and median; the mean carries the long tails of a fight broken off and resumed. |
| wins | `W;` | The winning team per line: one per round in `sd` and `re`, one per match in the others (`dm` names the player). |
| score | `getTeamScore` | Axis and allies at the end (before `exitLevel` zeroes them). Rounds won in `sd` and `re`. |
| actions | `A;` | Objective actions by team: `bomb_plant`, `bomb_defuse`, `re_pickup`, `re_capture`, UO's `ctf_take` and kin. |
| stalled, held | bodies | Bot-ticks whose last 12 s lie within 150 units of the first, as `ship_stalls` counts them (playing, bipod up; a bipod down clears the window). `held` is those whose bot held still on purpose on any tick of the 12 s (`BotBody::holds`): engaged, on a held cover spot, retreating or at the retreat's cover, in a vehicle seat or holding the hull for crew, or on its objective (a plant or defuse under way, the script holding the body, or standing at a guard ring, a site, the HQ radio; `BotBody::on_objective`). `stalled` is the rest. A hunt's vantage is no hold: arriving drops the recall and the bot walks on. |
| ended | `exitLevel` | Matches that reached their end inside `TICKS`. |
| life | bodies | Mean life: bot-ticks playing over the lives they made up (a life still running at the end is left out). |
| wounds, dead in 5 s | bodies | A life's first tick under 50 health while `BotBody::engaged`, and the share of those whose bot died within 5 s. |
| retreats hurt, dry | bodies | `BotBody::retreating` going from 0 to 1 (hurt) or 2 (dry): retreats started (section 5). |
| cover hold, lean | `bot_cover_state` | Bot-ticks playing on a held cover spot, and with `ps.lean` off zero (section 4). |
| answered | `D;`, bodies | Of the hits a bot takes from an enemy (one per attacker until answered or 1 s passes), the share it answers within 1 s: a tick after the hit's with `BotBody::firing` and that attacker as `BotBody::target`. Fire at someone else does not count; a bot killed inside the second has not answered. Printed with the hit count. |
| blindsided | `K;`, bodies | Of a bot's deaths to an enemy, the share whose killer was never its `BotBody::target` that life, reaction delay included. Printed with the death count. |
| mate, packed | bodies | Each bot-tick playing with a teammate playing adds its 3D distance to the nearest one: `mate` is the mean. `packed` is the share of those bot-ticks with two or more teammates within 400 units. Bots only. `dm` prints 0. |
| grenades thrown | bodies | Frags let go (`BotBody::throws`, counted at the release; a throw put back is not one). |
| grenade hits, kills | `D;`, `K;` | Lines with `MOD_GRENADE_SPLASH` on another player, by the thrower; kills are the `K;` ones. A frag that kills its own thrower is a suicide. |
| own, team | `K;` | Of the `MOD_GRENADE_SPLASH` kills, those of the thrower himself (also in `suicides`) and of a teammate (also in `teamkills`). A cook dropped by a dying thrower counts as his. |
| engagements, clip at start | bodies | `BotBody::engaged` going from off to on with a bullet weapon in hand, and the mean of its clip over its `clipSize` on that tick. |
| shots | bodies | Rounds fired: each tick's drop in the held clip while the weapon stays the same bullet gun (`weaponType bullet`, `BotBody::gun`). Grenades, rockets and mounted MG rounds are not counted. |
| hits | `D;`, `K;` | Lines on another player with a bullet means of death (`MOD_RIFLE_BULLET`, `MOD_PISTOL_BULLET`, `MOD_HEAD_SHOT`). Mounted MG hits land here too. |
| accuracy | | Hits over shots. |
| sight to shot | bodies | From a tick whose `BotBody::target` comes up with none the tick before (reaction delay included) to the first tick that fires a round at that same target, in ms; mean and median. A switch from one target straight to another starts no count. "Fired on" is the share of acquisitions that got a round before the target went (died, hid past `forget_ticks`, or the bot died). |
| sight to hit | bodies, `D;`, `K;` | The same acquisitions, to the first bullet hit (a `D;` or `K;` line) by the bot on that target while it stays the target; "hit" is the share that got one. |
| peeks, re-peeks, dead in 2 s | `bot_tactics` | A peek is a held cover spot's body going from hidden (or walking to the spot) to showing: standing, leaning or leaning crouched (`Bot::tactics`, counted in `cover_keys`). A re-peek is one from the spot (within 12 units) and pose (stance and lean side) of the bot's peek before it this life. Each printed with the share whose bot died within 2 s (first dead tick, any cause). |
| hunts, dead in 2 s | `bot_tactics`, bodies | A hunt that reached where it was headed: a seen enemy's lost spot, or with `Bot::wary` the vantage over it (section 10), within `ARRIVE_RADIUS`. A spot dropped on memory or the hunt's budget is no arrival. Printed with the share dead within 2 s of arriving. |
| fights, target down, bot down, broken, over 10 s, re-engaged | bodies | A fight is a bot's ticks with `BotBody::target` on one slot, merged across gaps under 1 s (another target in between included). It ends with the target down (dead or off the field, by anyone), the bot down, or broken off: neither, and the target untracked for 1 s. `over 10 s` counts fights whose first and last tracked ticks lie 10 s or more apart. A re-engagement is a fight on the target of one the bot broke off within the last 10 s, both still in the lives they fought in; printed with its share of the broken fights (section 13). |
| walking ticks, turn mean, snaps | bodies | The walking view (section 11). A bot-tick counts when `BotBody::walking` holds on it and the tick before: playing, on the ground, off the rungs, steering at its waypoint with no unstick spell, no target tracked (`Bot::engaged` false). `turn mean` is the mean yaw change over those ticks in degrees (wrapped to ±180); `snaps` counts ticks that turned more than 30 degrees, with their share. |

Each match prints one line, then the totals by team, by skill and (arm
one-sided) by arm; with several maps a block per map comes first. Each
row prints seven lines: the log's columns, the bodies', the aim, the
grenades and the clip, the peeks and hunts, the walking view, the fights.

A change to bot behaviour reports its before and after on the same seeds,
map and gametype, in this doc's section for it.

`DIGEST=1` adds a line per match, `digest <map> <gametype> seed <s>
ticks <n> <hex>`: an FNV-1a 64 folded after every tick over each bot slot
in order: its last cmd that ran (time, every button byte, stance and lean
included, weapon, angles, moves) and its playerstate origin and velocity
(by their bits), health and weapon. `DIGEST=<N>` also prints the running
hash every `N` ticks as `digest@<tick> ...`. It reads and draws nothing,
so the other lines are unchanged. A refactor meant to move no bot prints
the same digests; when they part, the first differing `digest@` line
names the `N`-tick window where a bot first did something else. Only
comparable between runs of the same map, gametype, seed, `BOTS` and
knobs; `JOBS` does not change it.

`NETDIGEST=<N>` adds a hash over every datagram the server sent each tick
(address, length, bytes, in send order) as `net@<tick>` and `net <map>
...` lines. It joins one observer client that never picks a team, at a
non-LAN address so rate and `snaps` pacing run; that changes the match, so
its `digest` lines compare only with other `NETDIGEST` runs. A CoD2 server
under it runs `svs.time` on the frame clock (`Cod2Server::test_frame_clock`),
since the wall clock keys the challenge and so every netchan byte.
`NETDUMP=<dir>` writes the datagrams. `VCOD_TICK_TRACE=<dir>` writes each
match's tick step log (`vcod_server::tick_trace`, one line per `SV_Frame`
step and its slot); the first differing line names the step that moved.

History of the stall column: before 2026-10-21 `stalled` was stalled plus
held. Until the nav recheck the same day `held` read the tick alone, so up
to 12 s after a fight read `stalled`: on `mp_depot` `dm` seeds 1..=8 (6
bots, 12 000 ticks) 485 such bot-ticks, every one within 12 s of a held
tick, 0 with the window rule. VERIFIED (measured, master 028799bf,
`ship_stalls` `SHOOT=1` and its `ago` column, bot-navigation.md section 3).

The split, stalled / held. VERIFIED (measured, release `bot_match`,
`COD_DIR` at 1.1, master 3135508b, normal skill):

| Run | mp_carentan | mp_harbor | mp_depot | mp_brecourt | Total |
|---|---|---|---|---|---|
| `tdm` 12 bots, seeds 1..=4 | 925 / 16 428 | 605 / 10 330 | 1 340 / 36 393 | 322 / 7 074 | 3 192 / 70 225 |
| `dm` 10 bots, seeds 1..=4 | 852 / 10 687 | 498 / 6 024 | 598 / 9 786 | | 1 948 / 26 497 |

The largest single match, mp_depot `tdm` seed 2, is 528 / 20 574.
VERIFIED (measured, same runs): 96% (`tdm`) and 93% (`dm`) of the old
column was bots holding on purpose. INFERRED: the before and afters in
6.6, 10.1 and 11 that cite the old column moved mostly through holds.

## 2. Baseline

Master at the harness's commit, `bot_match` defaults (mp_carentan `tdm`,
six bots, seeds 1..=8, 12000 ticks, none ended):

| Team | Kills | Deaths | Damage | TTK mean / median ms |
|---|---|---|---|---|
| allies | 644 | 568 | 87952 | 1603 / 250 |
| axis | 568 | 644 | 82955 | 1889 / 0 |

1212 kills over 80 minutes of play, 88 stalled bot-ticks, no suicides, no
team kills.

## 3. Skill levels

`vc_bot_skill` (0-3, default 1) and `addbot ... skill N` pick one of four
presets of `bots::Skill` (`bots/skill.rs`). Level 1 is `Skill::default()`
field for field (`the_default_level_is_the_default_skill`); its hearing
scale of 1.0 multiplies a noise's radius exactly, so a default server
plays as before the presets (section 2's matches print the same lines).

| Field | 0 easy | 1 normal | 2 hard | 3 expert |
|---|---|---|---|---|
| reaction, ticks (min + rand(spread)) | 8 + 6 | 4 + 4 | 3 + 4 | 2 + 2 |
| turn per tick, degrees | 10 | 15 | 22 | 30 |
| aim error start, floor (units) | 64, 5 | 36, 4 | 32, 2.5 | 24, 1.5 |
| error decay per tick | 0.91 | 0.85 | 0.84 | 0.80 |
| error doubles at range | 900 | 1000 | 1300 | 1600 |
| error while moving at run speed | x1.9 | x1.75 | x1.5 | x1.3 |
| aim trails a crossing target by (s) | 0.09 | 0.06 | 0.045 | 0.03 |
| footwork chance a spell on the sights | 0.25 | 0.45 | 0.6 | 0.75 |
| forget a target, ticks | 16 | 20 | 30 | 40 |
| remember a spot, a noise, ticks | 70, 140 | 100, 200 | 140, 260 | 180, 300 |
| hearing scale | 0.75 | 1 | 1.25 | 1.5 |
| sight cone for a new target, degrees across | 100 | 120 | 140 | 160 |
| walking view's turn per tick, degrees (section 11) | 18 | 24 | 30 | 36 |
| flick landing spread, share of the angle (section 18) | 0.15 | 0.1 | 0.075 | 0.05 |
| sway and recoil taken back, share, ticks late (section 18) | 0.5, 4 | 0.7, 3 | 0.8, 2 | 0.9, 2 |

The measurements in this section predate section 9, when normal's
reaction was 5 + 6, its error start 48 and its decay 0.88.

The miss is `error * (1 + range / error_range) * moving`, `moving` `1 +
(error_moving - 1) * (speed / 190)^2`, about a point that trails a
crossing target by its speed times `track_lag` (section 18; before it,
section 9's `(error + across * track_lag) * ...`). The trigger goes down
every tick while the gun's line passes within `HIT_RADIUS` (10 units) of
the target's chest (`Bot::trigger`); past that, by chance (before section 9
only inside it). So the floor sets the range of sure fire: `floor * (1 +
range / error_range) * moving < 10`. Trap: an easy floor of 10 never met
it; in `dm` its bots scored 26 kills to 518 deaths over eight matches, and
an all-easy `tdm` stalled 3566 bot-ticks against the baseline's 88. The
floor of 5 lets an easy bot fire standing inside 900 units and almost
never on the move.

Measured with `bot_match`, mp_carentan, seeds 1..=8, 12000 ticks.

Mixed `dm`, eight bots, two per level (`GAMETYPE=dm
SKILLS=0,1,2,3,0,1,2,3`; all eight ended at the score limit):

| Level | Kills | Deaths | K/D | Damage | TTK median ms |
|---|---|---|---|---|---|
| 0 | 166 | 518 | 0.32 | 20996 | 200 |
| 1 | 207 | 491 | 0.42 | 32017 | 150 |
| 2 | 605 | 345 | 1.75 | 74568 | 400 |
| 3 | 678 | 302 | 2.25 | 93670 | 250 |

The same on mp_depot: K/D 0.31, 0.62, 1.50, 2.37.

`tdm` three against three, one side at a level against normal, both ways
round (allies lead axis 644 to 568 at equal skill, section 2). Kills of
the side at the level, to the normal side's:

| Level | As axis | As allies |
|---|---|---|
| 0 | 354 : 764 | 406 : 715 |
| 2 | 800 : 346 | 793 : 309 |
| 3 | 799 : 268 | 800 : 209 |

All six at one level (`SET=vc_bot_skill=N`), total kills in the 80
minutes: 1044 at 0, 1212 at 1, 1245 at 2, 1367 at 3; stalled bot-ticks
364, 88, 95, 38.

Hard and expert sit close in `dm` because both react and settle faster
than a fight lasts; the gap shows head to head against normal (3 : 1
kills at hard, 3.4 : 1 at expert). Nothing here is tuned against a human;
how a level feels to play against is pending a hand check.

## 4. Cover and lean

The brain is `crates/server/src/bots/cover.rs`; the server fills
`BotView::cover` in `bots::driver::cover` (`crates/server/src/bots/driver.rs`).

### 4.1 What a cover spot is

A spot is a place the bot's body hides from the threat's eye while a peek
from it still has a line to the point the bot aims at on the threat
(`cover::Sight::peek`). Two peeks:

- `Peek::Stand`: crouched (eye 40) the body is hidden, standing (eye 60)
  the eye sees the threat. A wall waist to chest high.
- `Peek::Lean(side)`: standing, the body is hidden, and leaning `side`
  (retail's lean keys, `wbuttons` 0x10 left and 0x20 right) the eye sees
  the threat. A corner, or a wall edge.

"Hidden" is four bullet segments (`CollisionWorld::shot_trace`, props
included) from the threat's eye, all blocked: to the eye, the chest (two
thirds of the eye's height) and the chest 10 units either side across the
line. The leaned eye is `aim::lean_offset` at the standing lean's limit
(1 on 1.1, 0.5 on 1.5 and UO, `pmove::lean_scale`), cut by the fraction
of `PM_UpdateLean`'s 16-unit box sweep a wall leaves, as pmove cuts it.

### 4.2 The search

`cover::find` walks level edges (no jump, no risky edge, no step past
`STEPSIZE`) out from the node nearest the body, five rings and at most
128 units across, and tests the body's own spot first, then the nodes
nearest first. The first `Peek::Stand` in reach wins; a lean spot is
taken only where there is none (4.5). The server re-checks the spot it
gave last every 250 ms against the threat's current eye and keeps it
while it still hides and peeks; it searches again only when it does not.
Within 300 units of the threat there is no search: the bot fights where
it stands.

Decision: the search runs at fight time, not at graph build. A spot
depends on where the threat stands, so a per-node table would need a
verdict per direction and still a trace at run time to confirm it. A
search costs at most a few dozen candidates of one to six segments,
most rejected on the first, four times a second per fighting bot. The
graph and its build are unchanged, so `bot-navigation.md`'s Build times
and the `nav_census` floors do not move.

### 4.3 What the bot does with it

- Walks to the spot, keys eased inside 48 units so it comes to rest on
  it, sights down beyond 64 units. It aims and fires on the way as the
  fight allows (`Bot::cover_keys`).
- On the spot it holds still. While the weapon is ready it peeks
  (stands, or holds the lean key); while it reloads or the weapon is busy
  past 300 ms (a bolt's rechamber, not a semi-automatic's tap spacing)
  it crouches behind a `Peek::Stand` wall or comes back off a lean.
- With the threat out of sight the spot is held for 3 s
  (`cover::WAIT_TICKS`; a wary bot draws its own, section 10), peeking
  where the threat was last aimed at; the target is not counted lost
  meanwhile, so a re-peek keeps its settled aim (`Bot::track`). Past that
  the hold goes and the bot hunts the remembered spot (section 10).
- No cover while a plant, defuse or carry is under way, on a capture
  zone or radio, with a UO LMG, or held by a link. A guard or a sweep
  takes cover (`Bot::may_cover`). A live frag near clears the lean and
  crouch keys with the rest (`Bot::flee`).

### 4.4 Aim from a leaned eye

A lean moves the eye, and the muzzle with it, up to 20 units sideways
(`G_AddLean`, combat doc 2.1). `BotView::lean_offset` carries it and
`aim_angles` aims from there; from the upright eye a leaned round
passes up to 20 units beside a chest it was aimed at, twice
`HIT_RADIUS`. INFERRED: needed for any leaned shot to land. VERIFIED
(measured, 4.5's second and third rows): the fix alone did not lift the
first cut's score, which its walk dominated.

### 4.5 Measured

Measured with `cover_ab`, since folded into `bot_match` (`COVER=alt` is
its side swap; its seeds went straight to `test_seed_rng`, so seeds 2k and
2k+1 played one match). `tdm`, 8 fighting bots, 6000 ticks a seed, one
team with cover and one without, sides swapped every other seed. Over 20
seeds the per-seed difference has a standard deviation of 12 to 14
deaths, so a total difference under about 60 is noise. Section 6
re-measures on `bot_match`.

| Run (mp_carentan unless named, seeds 1-20) | Cover side deaths | Other side deaths |
|---|---|---|
| cover off on both sides | 1141 | 1152 |
| shipped: a stand spot before a lean | 987 | 1050 |
| shipped, mp_dawnville | 813 | 887 |

- VERIFIED (measured, the rows above): the shipped cover side dies 6%
  fewer times on mp_carentan and 8% on mp_dawnville, winning 12 and 13 of
  20 seeds. Pooled, the mean per-seed gain is 3.4 deaths at a standard
  error of about 2.2: small, not proven.
- VERIFIED (measured): fewer deaths overall than with cover off (2037
  against 2293): fights from cover last longer.
- Rejected variants (cover side / other side deaths, same seeds): first
  cut with a 192-unit reach, full-speed walk and upright-eye aim 1079 /
  957; 4.4's aim fix alone 1114 / 903; 128-unit reach, own spot first,
  eased walk 1027 / 1055; lean spots only 1027 / 1035; stand spots only
  1092 / 1220; walk to the spot only while the weapon is busy 1045 / 1091
  (inside the noise, so the simpler rule stayed). VERIFIED (measured).
- VERIFIED (measured, seed 3, 3000 ticks): with the 192-unit reach 1447 of
  2176 ticks holding a spot (66%) were spent walking to it, at
  `Skill::error_moving` and mostly off the sights; with 128 units, own
  spot first and the eased walk, 3538 of 6321 (56%).
- INFERRED: a leaned body shows its head and the threat aims there
  (`aim_point` falls back to the eye when the chest is hidden), so a lean
  trades body exposure for headshots; behind a low wall the bot drops out
  of sight on every reload and the threat's target lapses after
  `Skill::forget_ticks` while the bot's own does not.

## 5. Retreat

A bot in a fight breaks contact when it is hurt or its clip runs dry,
then reloads, waits and plays on (`crates/server/src/bots/retreat.rs`).
On by default; `RETREAT=` in `bot_match`.

- Hurt: health under 50% of the most (`HURT_PCT`) with an enemy in
  sight, once per wound (`Bot::hurt_at`): a bot that held its cover
  fights on at that health until hit again or healed past it. 1.1, 1.5 and
  UO have no health regeneration; a health pack is the only way back up.
- Dry: the clip at 0 with rounds in the reserve, an enemy in sight and no
  switch under way at the end of the think, so a pistol draw
  (`Bot::sidearm`) comes first. It ends once the clip holds a round and
  the weapon is idle, or the reserve is gone. Once per dry clip
  (`Bot::dry_had`, cleared when the clip holds a round or the reserve is
  gone): one that found no cover reloads where it stands (trap in 6.4).
- Cover (`cover_spot`): the nearest graph node by path from the body,
  within 1200 units of path and 300 nodes tested (on mp_carentan,
  mp_harbor and mp_depot the first hidden node took a median 7 tests, at
  most 205), whose standing eye and chest the threat's eye cannot see by
  `shot_trace` (props included), nearer the bot than the threat and over
  200 units from it. The threat's eye is the enemy's `ps.view().eye` as
  last seen. The cover is kept while it stays hidden from that eye, else
  searched again. No cover ends the retreat: the bot fights on.
- On the way the keys follow the path (`rekey` off the path's yaw) while
  the view and trigger stay on the enemy, sights up. At the cover the
  body stands still (no unstick spell), crouches with nothing in sight,
  tops the clip up and turns to where the threat was. A hurt bot holds
  3 s (`HOLD_TICKS`), less with a health pack near; no retreat runs past
  10 s.
- Health packs: a bot under 70% (`HEAL_PCT`) with nothing in sight walks
  to the nearest `item_health*` within 1200 units across and 200 up or
  down (`ScriptRuntime::health_items`; stock `dm`, `tdm` and `sd` drop one
  on every death, `dropHealth`, cod11-items.md).
- Duties a retreat or health detour never breaks (`duty_bound`): a plant
  or defuse under way, an `re` objective carried, a UO `ctf` flag carried,
  a defuse against the clock (S&D's planted bomb, a charge on a `bas`
  defender's base), and a body inside the `dom` flag zone or `hq` radio
  zone it is taking. Everything else retreats: an S&D attacker on its way
  to a site, a guard, an `re` defender chasing, a `bas` breacher, both
  `bel` sides. On a ladder or with an LMG's bipod down nothing starts.
- `BotBody::retreating` reads 1 (hurt), 2 (dry), 3 (at the cover) or 0;
  `Server::test_bot_retreat` turns it off per bot.

### Measured (2026-10-20)

Measured with `bot_fights` (since folded into `bot_match`): 8 bots, `tdm`,
3000 ticks, mp_carentan, mp_harbor, mp_depot, mp_brecourt, 16 seeds each,
one side retreating. VERIFIED (measured):

- A wounded bot running the retreat dies within 5 s 5 to 10 points less
  often, on all six 1.1 runs (seeds 101-116 and 117-132, each side) and
  on UO (mp_cassino, mp_kursk, seeds 1-8: 83% / 86% with none, 72-73%
  for the retreating side).
- K/D of the treated side against `none`, geometric mean: 1.02 on 1.1,
  0.99 on UO, inside the noise (the same side's K/D swung 0.15 across seed
  sets with only the other side's brain changed).
- Rejected variants (treated side's K/D against `none`, seeds 101-116,
  allies / axis / geometric mean): sights dropped while backing off 1.03 /
  0.93 / 0.98; hurt retreat only past 400 units 1.02 / 0.92 / 0.97; dry
  retreat and health packs without the hurt retreat 1.07 / 0.95 / 1.01.
- 1150 to 1440 hurt and 40 to 100 dry starts in 64 matches with one side
  on; 28 to 32% reach the cover. The rest die on the way, or the retreat
  ends first (a dry clip refilled, or no cover left from a moved threat).

## 6. One table on `bot_match` (2026-10-21)

Sections 2 to 5 and `bot-objectives.md` 9 were measured on four tools with
different seeds, bot counts and lengths; these rows are all `bot_match` at
one tip (master c8c3fd1f plus the harness unification), 8 bots, 6000
ticks a match, 1.1 unless named.

Each arm leaves the other features at their default, on for both sides
(`COVER=alt` runs the retreat on both sides). "Margin" is the arm side's
score less the other's per match, mean and standard error; the symmetric
rows give the same figure for the `alt` split with nothing different, the
noise floor.

### 6.1 `tdm`, mp_carentan, mp_harbor, mp_depot, mp_brecourt, seeds 1..=16 (64 matches a row)

VERIFIED (measured, ours, `bot_match`, 2026-10-21):

| Run | Side | Kills | Deaths | K/D | Life s | Wounds | Dead in 5 s | TTK mean ms | Margin |
|---|---|---|---|---|---|---|---|---|---|
| stock (all on) | both | 5237 | 5244 | 1.00 | 24.8 | 3873 | 72% | 1783 | -1.1 +- 1.3 |
| `COVER=none RETREAT=none` | both | 6596 | 6600 | 1.00 | 19.5 | 4826 | 85% | 1215 | -2.2 +- 2.1 |
| `COVER=alt` | cover | 3045 | 3043 | 1.00 | 21.4 | 2277 | 75% | 1672 | +0.1 +- 2.0 |
| | control | 3040 | 3046 | 1.00 | 21.4 | 2320 | 74% | 1913 | |
| `RETREAT=alt` | retreat | 2736 | 2726 | 1.00 | 24.1 | 2010 | 72% | 1438 | +0.2 +- 1.5 |
| | control | 2724 | 2743 | 0.99 | 23.9 | 2062 | 82% | 1636 | |
| `COVER=alt RETREAT=alt` | both on | 3124 | 2904 | 1.08 | 22.3 | 2104 | 74% | 1142 | +3.5 +- 1.9 |
| | control | 2903 | 3125 | 0.93 | 20.7 | 2377 | 85% | 1833 | |
| one side `vc_bot_skill 2` | hard | 3790 | 1747 | 2.17 | 36.7 | 1228 | 66% | 1150 | +31.9 +- 1.6 |
| | normal | 1747 | 3790 | 0.46 | 17.0 | 2672 | 80% | 3080 | |
| `AWARE=alt` (section 8) | aware | 2882 | 2330 | 1.24 | 27.7 | 1719 | 72% | 1882 | +8.7 +- 1.6 |
| | control | 2326 | 2890 | 0.80 | 22.6 | 2188 | 75% | 2035 | |
| stock, noise answering (section 7) | both | 4667 | 4671 | 1.00 | 28.2 | 3446 | 72% | 1918 | -0.2 +- 1.1 |
| `TEAMPLAY=alt`, answering one side | teamplay | 2313 | 2607 | 0.89 | 25.2 | 1860 | 76% | 2065 | -4.5 +- 1.2 |
| | control | 2603 | 2314 | 1.12 | 27.9 | 1717 | 72% | 1399 | |

The last three rows came later (section 7's branch), where the stock run
before the change gave 5265 kills and 24.9 s lives (after 6.4's dry-start
fix). In 1.1 `tdm` team play is only the noise answering, so
`TEAMPLAY=alt` pits it against the old rule. The skill row sums
`AXIS_SKILL=2` on seeds 1..=8 and `ALLIES_SKILL=2` on 9..=16. The `AWARE`
row is a later tip whose `AWARE=none` run prints 5265 kills.

- VERIFIED (measured, the table): skill is the one large effect: a hard
  side outscores a normal one 2.2 to 1 (section 3 found 2.3 to 2.6).
- VERIFIED (measured, the table): cover alone and the retreat alone move
  no score: both margins sit inside one standard error. Section 4.5's 6%
  does not show here.
- VERIFIED (measured, the table): the retreat cuts wounded deaths within
  5 s from 82% to 72%, K/D unchanged.
- VERIFIED (measured, the table): together on one side they gave +3.5 a
  match (1.8 standard errors) and wounded deaths 74% against 85%.
- VERIFIED (measured, the table): both features on everywhere make the
  game slower: 21% fewer kills than both off, lives 24.8 s against
  19.5 s, and the old `stalled` column (holds included) nearly eightfold
  (104718 bot-ticks against 13666). INFERRED: the extra are bots holding
  cover or a retreat's cover on purpose.

### 6.2 The arms again, seeds 17..=32

6.1's three `alt` runs on fresh seeds (64 matches a row), and both sets
pooled (128 matches). VERIFIED (measured, ours, `bot_match`, 2026-10-21):

| Run | Side | Kills | Deaths | K/D | Dead in 5 s | Margin | Pooled margin |
|---|---|---|---|---|---|---|---|
| `COVER=alt` | cover | 2984 | 2871 | 1.04 | 73% | +1.8 +- 1.8 | +0.9 +- 1.3 |
| | control | 2869 | 2986 | 0.96 | 76% | | |
| `RETREAT=alt` | retreat | 2725 | 2686 | 1.01 | 72% | +0.7 +- 1.3 | +0.4 +- 1.0 |
| | control | 2681 | 2728 | 0.98 | 80% | | |
| `COVER=alt RETREAT=alt` | both on | 3026 | 2894 | 1.05 | 74% | +2.1 +- 1.7 | +2.8 +- 1.3 |
| | control | 2891 | 3027 | 0.96 | 82% | | |

- VERIFIED (measured, the table): pooled, cover and the retreat together
  give a side about 2.8 kills a match (some 2 standard errors, K/D 1.05 to
  1.08); each alone stays within one standard error of nothing. Both are
  on by default.
- VERIFIED (measured, both tables): the retreat cuts wounded deaths
  within 5 s by 8 to 10 points in all four runs.

### 6.3 Team play: `sd` and `re` on the ten 1.1 maps that run both, seeds 1..=6 (60 matches a row)

`TEAMPLAY=` switches `bot-objectives.md` 9's rules per side in one build,
retreat and cover on for everyone. The attackers are the team that plants
or picks up (axis on mp_pavlov in `sd`, on mp_dawnville and mp_railyard in
`re`; allies elsewhere). Wins are rounds.

VERIFIED (measured, ours, `bot_match`, 2026-10-21):

| Run | att W | def W | att share | plants / pickups | defuses / captures | stalled |
|---|---|---|---|---|---|---|
| `sd TEAMPLAY=none` | 104 | 178 | 37% | 200 | 85 | 408827 |
| `sd TEAMPLAY=all` | 166 | 107 | 61% | 229 | 52 | 474275 |
| `re TEAMPLAY=none` | 142 | 197 | 42% | 233 | 25 | 363031 |
| `re TEAMPLAY=all` | 137 | 194 | 41% | 219 | 46 | 346276 |

With `TEAMPLAY=alt` the team-play side took 168 `sd` rounds to 116 and
179 `re` rounds to 157.

- VERIFIED (measured, the table): `sd`: team play lifts the attackers from
  37% to 61% of rounds (`bot-objectives.md` 9 found 47% to 62% without
  cover or retreat).
- VERIFIED (measured, the table): `re`: captures nearly double (25 to 46;
  there 9 to 18), but the attackers' round share does not move.
  INFERRED: most rounds end on the clock or a wipe, not a capture.

### 6.4 United Offensive: `tdm`, mp_cassino and mp_kursk, seeds 1..=8 (16 matches a row)

`UO=1`, `COD_DIR` at a UO install. VERIFIED (measured, ours,
`bot_match`, 2026-10-21; before the dry-start fix below):

| Run | Side | Kills | Deaths | K/D | Life s | Wounds | Dead in 5 s | Margin |
|---|---|---|---|---|---|---|---|---|
| stock | both | 601 | 626 | 0.96 | 44.8 | 417 | 75% | -1.1 +- 1.8 |
| `COVER=none RETREAT=none` | both | 657 | 702 | 0.94 | 39.0 | 470 | 81% | -1.6 +- 2.1 |
| `COVER=alt` | cover | 324 | 332 | 0.98 | 43.3 | 228 | 76% | +0.8 +- 2.6 |
| | control | 321 | 344 | 0.93 | 40.7 | 242 | 78% | |
| `RETREAT=alt` | retreat | 341 | 306 | 1.11 | 44.1 | 207 | 71% | +3.0 +- 3.0 |
| | control | 281 | 354 | 0.79 | 37.7 | 237 | 84% | |

- VERIFIED (measured, the table): kills and deaths differ by suicides and
  team kills; every margin is within about one standard error of nothing;
  the retreat's wounded deaths fall 13 points, as on 1.1.
- Trap, fixed: a dry retreat that found no cover ended on its next tick
  (`cover_spot` `None`) and restarted the tick after on the same clip,
  re-running the cover search every other tick through a reload. On open
  mp_kursk UO's axis (5-round kar98k, gewehr43, luger) started 958 dry
  retreats to the allies' 292 (1.1: 112 and 149). With one dry retreat
  per clip (`Bot::dry_had`, section 5): axis 69, allies 48 (mp_kursk 51
  and 22; mp_cassino 18 and 26 unchanged, its matches identical line for
  line); kills and lives inside the noise, since a retreat with no cover
  never changed the keys (`retreat_keys` returns on `None`). The axis's
  remaining 1.4x is its weapons (28 of 69 starts on the kar98k, 14 on the
  luger, against the allies' 71-round ppsh); the logged `clipSize`s match
  the `pakuo` weapon files (kar98k 5, gewehr43 10). On 1.1 dry starts go
  261 to 241, kills 5237 to 5265, lives 24.8 s to 24.7 s. VERIFIED
  (measured, same commands and seeds, 2026-10-21).

### 6.5 Grenades and the tactical reload (2026-10-21)

The rules (`Bot::throw_spot`, `Bot::think_cook`, `bots::notices_frag`,
`Server::lob_probe`):

- A throw goes at an enemy in sight no round has gone at for 1.5 s
  (`COVER_THROW_TICKS`: behind cover, or never lined up), or at the spot a
  seen enemy was lost at 0.2 to 3 s ago if he was slower than 100 units/s
  then (`LOST_STILL`), 250 to 600 units off, from a body under 60 units/s
  with its eye settled. Never at an enemy the gun can hit. Cooldown 10 s.
- `lob_probe` finds no lob while a live teammate stands within the frag's
  `explosionRadius` of the target.
- A throw under way follows the enemy it started at; a lost spot's throw
  follows nobody. Trap: one held for one enemy went at another who came
  into sight in the hold, released on the old view, and killed its
  thrower.
- A 1.1 or 1.5 frag (no `cookOffHold`) with no clear lob, or the view off
  it, when the 16-tick wait is out is put back: the cmd that lets the
  trigger go asks for the rifle, and the switch cancels the pullback with
  the clip unspent (`cod11-combat.md` 1.14). A UO frag, its fuse running,
  is let go.
- Another's frag is run from only once noticed: in a clear line within 60
  degrees of the view's yaw, or within 150 units. A bot's own is always
  run from.
- A clip under half reloads once no enemy has been in sight for 1 s
  (`QUIET_RELOAD_TICKS`), retreat or not; not the pistol, which goes back
  by then.

Before (the harness commit: a 1-in-4 roll, 20 s cooldown, throws at any
enemy) and after. `bot_match` 1.1, seeds 1..=4, 12000 ticks: `tdm` on
mp_carentan, mp_harbor, mp_depot, mp_brecourt with 12 bots (16 matches,
all ended), `dm` on mp_carentan, mp_harbor, mp_depot with 10 (12, none
ended). VERIFIED (measured, ours, `bot_match`, 2026-10-21):

| Run | Kills | Suicides | Life s | TTK mean / median ms | Wounds | Dead in 5 s | Retreats dry | Stalled | Thrown | Hits | Grenade kills | Clip at start |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `tdm` before | 2908 | 4 | 22.0 | 1270 / 0 | 2160 | 76% | 102 | 65863 | 1941 | 39 | 5 | 94% |
| `tdm` after | 3001 | 0 | 20.2 | 1266 / 0 | 2246 | 76% | 61 | 81750 | 720 | 53 | 7 | 96% |
| `dm` before | 3116 | 0 | 20.1 | 1253 / 300 | 2333 | 74% | 132 | 45053 | 1729 | 9 | 1 | 90% |
| `dm` after | 3163 | 0 | 19.7 | 1335 / 300 | 2376 | 75% | 75 | 45452 | 1052 | 9 | 0 | 93% |

- VERIFIED (measured, the table): a `tdm` frag hits someone 7.4% of the
  time against 2.0%, with a third of the throws; grenade kills stay rare
  (7 of 3001). Dry retreats fall 40% in both gametypes; the clip a fight
  starts on rises 2 to 3 points.
- VERIFIED (measured, same `dm` seeds on mp_carentan and mp_harbor, each
  throw and probe logged): with only the new targets, 7 of 10 grenade
  kills were the thrower's own, all released with no clear lob after the
  wait (the target swap above, or a body that moved through the switch
  to where a wall stopped every lob). With the put-back rule, none.
- VERIFIED (measured, `tdm` before the noticing rule): frags run from
  wherever they lay killed nobody: 718 thrown, 0 grenade kills.
- `dm` frags still hit almost nobody (9 of 1052). INFERRED: with no
  teams every enemy in sight is a candidate, so a throw lands in front of
  him and he notices it.
- `tdm`'s `stalled` rises 24% while `dm`'s holds. INFERRED: bots stand
  through a throw's switch and hold, and through quiet reloads.

The lob probe flies the frag to its burst. `Server::lob_probe` takes a lob
only when the frag, flown by the missile code from the release to the end
of its fuse (`Missiles::predict_flight`: `Missile::step` against the
world, no bodies or models), stays farther than its `explosionRadius`
from the thrower's box at every frame the fuse can end on: from
`COOK_MOST_MS` (2250 ms) short of the full fuse for a UO cook, at the full
fuse elsewhere. Section 12 replaced the old arc check (which ended at the
first contact) with this.

- VERIFIED (measured, ours, release, stock UO paks, `ship_stalls` per-tick
  dump, seed 19 `tdm` on 2180b31d): with the arc check, a stick grenade
  lobbed from (2583, -925, 0) at an enemy 550 off met a wall about 310
  out, past the check's 60 per cent, came back and rolled to (2558,
  -1126, 2); the wounded thrower ran to (2518, -791, 4), 337 off, and
  died of its 9 points.
- VERIFIED (`weapons/mp/stielhandgranate_mp` in `pakuo04.pk3`):
  `explosionRadius 350`, `explosionInnerDamage 120`,
  `explosionOuterDamage 5`, `fuseTime 4`.
- VERIFIED (measured, ours, release, stock UO paks, burst test off and
  on): `ship_stalls` `UO=1 MAP=mp_cassino GAMETYPE=tdm SHOOT=1 TICKS=1800`
  seeds 1 to 63 odd: own frags 1 to 0, frag kills 4 to 3, kills 502 to
  507. `bot_match` `UO=1 MAPS=mp_cassino,mp_kursk BOTS=10` seeds 1 to 8:
  thrown 176 to 151, hits 74 to 46, grenade kills 7 to 5, kills 1613 to
  1586; `suicides` (43 and 47) counts every self-kill, not split by cause.

### 6.6 Aim and footwork: `tdm` and `dm`, 12 and 10 bots (2026-10-21)

Section 9's change, before (master d82876e1) and after on the same seeds.
`tdm`: mp_carentan, mp_harbor, mp_depot, mp_brecourt, `BOTS=12`, seeds
1..=8 (32 matches a row, all ended at the score limit). `dm`: mp_carentan,
mp_harbor, mp_depot, `BOTS=10`, seeds 1..=4 (12 matches, none ended).
"Sight" is sight to shot, mean / median ms; "fired" the share of
acquisitions fired on. Stalled is per 1000 match ticks, the old column
with holds (section 1).

VERIFIED (measured, ours, `bot_match`, 2026-10-21):

| Run | Kills | Allies K/D | Life s | Wounds | Dead in 5 s | TTK mean ms | Shots | Accuracy | Sight ms | Fired | Stalled |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `tdm` before | 5903 | 1.01 | 21.4 | 4354 | 75% | 1361 | 24084 | 69.6% | 1701 / 1450 | 19% | 459 |
| `tdm` after | 5870 | 1.04 | 21.6 | 4312 | 73% | 1483 | 45943 | 37.4% | 963 / 750 | 32% | 555 |
| `dm` before | 3116 | | 20.1 | 2333 | 74% | 1253 | 15562 | 52.1% | 2228 / 1900 | 50% | 313 |
| `dm` after | 3237 | | 19.2 | 2440 | 72% | 1487 | 27289 | 32.1% | 1185 / 850 | 66% | 367 |
| `dm` after, `FOOTWORK=none` | 3284 | | 19.0 | 2478 | 73% | 1542 | 27366 | 32.6% | 1192 / 850 | 68% | 318 |

`FOOTWORK=alt`, `tdm` as above (32 matches); margin is the footwork
side's score less the other's per match:

| Run | Side | Kills | Deaths | K/D | Life s | Dead in 5 s | Accuracy | Sight ms | Margin |
|---|---|---|---|---|---|---|---|---|---|
| after | footwork | 2858 | 2963 | 0.96 | 20.4 | 73% | 39.3% | 1022 / 800 | -3.1 +- 3.7 |
| | control | 2958 | 2858 | 1.03 | 21.2 | 74% | 38.1% | 976 / 800 | |
| strafe only (no back-off, no prone) | footwork | 2962 | 3024 | 0.98 | 21.8 | 73% | 38.3% | 976 / 750 | -1.8 +- 2.8 |
| | control | 3019 | 2967 | 1.02 | 22.2 | 72% | 36.9% | 977 / 750 | |
| moving error linear in speed | footwork | 2816 | 2986 | 0.94 | 21.4 | 75% | 38.6% | 1032 / 800 | -5.2 +- 4.1 |
| | control | 2981 | 2817 | 1.06 | 22.7 | 72% | 37.9% | 1024 / 800 | |

- VERIFIED (measured, the tables): sight to shot falls from a median
  1450 ms to 750 ms in `tdm` and 1900 to 850 in `dm`; a fresh target is
  fired on two thirds more often (19% to 32% in `tdm`). Accuracy falls
  from 70% to 37%: rounds go out before the aim settles, and an automatic
  fires twice the rounds for the same kills.
- VERIFIED (measured, the tables): kills, lives, TTK and wounded deaths
  move a few percent; the teams stay even (allies less axis +3.2 +- 3.6 a
  match after, +1.2 +- 3.4 before). With every weapon class given one
  reach of 3 the allies led by +6.8 +- 4.1, so the class reaches do not
  tilt it.
- VERIFIED (measured, the tables): the footwork side gives up 3 kills a
  match, inside one standard error; with the moving error linear in speed
  it gave up 5. Neither back-off nor prone alone accounts for the gap
  (-6.2 +- 4.2 without the back-off, -4.8 +- 3.7 without prone, both on
  the linear build).
- VERIFIED (measured, the merged tip with team spread, grenades and
  awareness, `tdm` seeds 1..=4, 16 matches): 2899 kills, allies K/D 1.09,
  life 26.5 s, 74% of wounded dead in 5 s, TTK mean 1803 ms, accuracy
  32.6%, sight to shot 960 / 750 ms with 56% fired on, 95% of hits turned
  on and 45% answered, 10% of deaths blindsided, stalled 514 per 1000
  ticks.
- VERIFIED (measured): `stalled` rises a fifth in `tdm`; cover holds rise
  a quarter (921k to 1150k bot-ticks) and dry retreats 193 to 507.
  INFERRED: twice the rounds empty twice the clips, and a dry clip holds a
  cover spot to reload (section 5).

## 7. A side spread over the map (2026-10-21)

A playtest report: with many bots in `tdm` nearly a whole team moved
together. `bot_match`'s `mate` and `packed` columns (section 1) count
it.

### 7.1 Why

- VERIFIED (measured, ours, `bot_match`, 12 bots, `tdm`, mp_carentan,
  mp_harbor, mp_brecourt, mp_depot, seeds 1..=6, 6000 ticks): 72% of
  playing bot-ticks had two or more teammates within 400 units, the
  nearest one 210 units off on average. The packing did not loosen
  over a life: 68% in a life's first 5 s, 73-74% from 5 s on (that
  split measured with the crowd cost below on, which moved nothing).
  INFERRED: `tdm.gsc`'s `getSpawnpoint_NearTeam` puts a respawn near
  the living teammates, and the brain kept the team there.
- VERIFIED (same runs, the brain's goal tallied each tick with a
  temporary counter): a seen enemy was the goal 43% of bot-ticks (83%
  of those packed), a remembered sighting 14% (79%), a heard noise 16%
  (57%), the roam 11% (33%).
- VERIFIED (same runs): with no noise heard at all the share packed fell
  to 29% and the nearest teammate stood 493 units off, and the seen
  enemy's ticks fell to 42% packed. INFERRED: the noise was what
  gathered a side. `bots::loudest` gives every bot the noise loudest to
  it out of every shot within `HEAR_GUNFIRE` (2000) units, teammates'
  shots included, so a whole side within 2000 units of a fight ran to
  the same spot, met the enemy there as one, and its shots drew the
  rest (`Server::bot_think`'s view pass, `view.noise`).
- VERIFIED (same runs): the route spreading of `bot-objectives.md` 9
  (`team::crowd`) switched on for `tdm` moved nothing (72.4% packed,
  mate 206): the team's paths shared a goal, not a corridor.

### 7.2 The rule

A bot on a side with team play on hears a noise only when fewer than
`team::ANSWERS` (2) of its playing teammates stand nearer it, the
shooter who made it not counted (`team::answers`, applied in the view
pass before `bots::loudest`). The two nearest answer a shot; the rest
keep to their roam or their own noise. Off with `TEAMPLAY=none` (a bot
then hears every noise, as before). `dm` has no sides and is unchanged.

Variants measured on 7.1's runs (kills over the 24 matches, share
packed, mean nearest teammate), each VERIFIED (measured, ours,
`bot_match`). Rows marked * ran with the crowd cost on in `tdm` as
well, which alone moved nothing (with it, the shipped rule read 2519,
35.9%, 417):

| Variant | Kills | Packed | Mate |
|---|---|---|---|
| before | 3404 | 72.1% | 210 |
| no noise at all * | 2241 | 29.3% | 493 |
| enemy shots only * | 3296 | 62.2% | 272 |
| the 3 nearest answer | 2823 | 51.4% | 335 |
| the 2 nearest answer (shipped) | 2580 | 39.5% | 398 |
| the 2 nearest answer, the others go 500 units to one side of it * | 3127 | 58.8% | 262 |
| the 2 nearest answer, the others 900 units to one side * | 2577 | 44.3% | 326 |

- VERIFIED (the table): every variant traded kills for spread along one
  curve; sending the others to a flank point bought nothing over
  dropping the noise for them. The plain rule stayed.

### 7.3 Before and after

`tdm`, mp_carentan, mp_harbor, mp_brecourt, mp_depot, 6000 ticks a
match, team play on for both sides. VERIFIED (measured, ours,
`bot_match`, before on master d82876e1, after on `feat/bot-spread`):

| Bots, seeds | Run | Kills | Life s | Mate | Packed | Stalled |
|---|---|---|---|---|---|---|
| 6, 1..=8 | before | 1712 | 28.9 | 572 | 19.1% | 28361 |
| | after | 1780 | 27.6 | 570 | 19.4% | 28073 |
| 8, 1..=16 | before | 5265 | 24.9 | 355 | 42.7% | 120746 |
| | after | 4667 | 28.2 | 472 | 29.9% | 85739 |
| 12, 1..=6 | before | 3404 | 21.7 | 210 | 72.1% | 71300 |
| | after | 2580 | 28.6 | 398 | 39.5% | 34176 |
| 12, 7..=12 | before | 3560 | 20.3 | 199 | 75.2% | 66364 |
| | after | 2529 | 29.3 | 397 | 40.8% | 38703 |
| 16, 1..=6 | before | 4419 | 20.2 | 139 | 85.8% | 87379 |
| | after | 3336 | 29.4 | 354 | 47.2% | 47855 |

Per map, 12 bots, seeds 1..=6 (before, after):

| Map | Kills | Mate | Packed |
|---|---|---|---|
| mp_carentan | 871, 699 | 167, 292 | 77.1%, 51.0% |
| mp_harbor | 920, 701 | 177, 335 | 76.8%, 46.1% |
| mp_brecourt | 781, 537 | 309, 612 | 61.5%, 22.7% |
| mp_depot | 832, 643 | 185, 352 | 73.0%, 38.7% |

Objective levels, 8 bots, mp_carentan, mp_harbor, mp_brecourt,
mp_depot, mp_dawnville, seeds 1..=6 (30 matches a row):

| Run | Kills | Wins axis, allies | Actions | Mate | Packed |
|---|---|---|---|---|---|
| `sd` before | 856 | 64, 86 | 120 plants, 31 defuses | 183 | 48.6% |
| `sd` after | 842 | 63, 82 | 123 plants, 31 defuses | 174 | 48.6% |
| `re` before | 988 | 96, 77 | 129 pickups, 26 captures | 210 | 63.2% |
| `re` after | 1020 | 105, 71 | 131 pickups, 23 captures | 204 | 63.7% |

- VERIFIED (the tables): from 8 bots up a side spreads: at 12 the
  share packed halves (72-75% to 40-41%) and the nearest teammate
  stands twice as far off; at 16, 86% to 47%. Three a side never had
  two others near and does not change.
- VERIFIED (the tables): the cost is fights. With 12 bots kills fall
  24-29% and lives run 7-9 s longer; with 8, 11%. K/D, the share of
  wounded bots dead in 5 s (71-75% against 74-75%) and TTK barely move.
  `stalled` falls 29-52%, INFERRED: fewer bots holding cover in one big
  fight.
- VERIFIED (the tables): `sd` and `re` keep their plants, defuses,
  pickups, captures and wins within a few of before; their bots gather
  at the objectives, so the spread does not move there.
- VERIFIED (measured, `TEAMPLAY=alt`): a spread side loses to one that
  converges: 8 bots, seeds 1..=16, -4.5 +- 1.2 a match (K/D 0.89 against
  1.12, section 6.1); 12 bots, seeds 1..=12, -11.5 +- 2.0 (0.82 against
  1.21); the 3-nearest rule at 12 bots, -9.7 +- 2.4. INFERRED: a whole
  side arriving at once outnumbers whoever it meets. Both sides play by
  the same rule in a real match, and a human now meets two or three bots
  at a fight instead of a side.
## 8. Target choice and sight (2026-10-21)

Before (`AWARE=none`): the nearest enemy in line of sight within
`SHOOT_RANGE`, all round, picked afresh every `ENEMY_REFRESH_MS` (100),
full reaction and `error_start` on every slot change, hits ignored; a bot
between two enemies swapped targets and paid the delay each time.

Now (`crates/server/src/bots.rs`, `server.rs`, `spectate.rs`):

- `ClientSim::last_hit`: each `SimOp::Damaged` with another client as
  attacker and damage above 0 records his slot, the frame's `sv_time` and
  the damage direction; a spawn clears it. `Server::bot_hurt` hands it to
  the brain as `BotView::hurt` for `HURT_MS` (2 s) while the attacker
  plays.
- `bots::pick_enemy`: the attacker of that hit when the eye line reaches
  him, else a new enemy nearer than `RETARGET_RATIO` (0.6) of the tracked
  target's range, else the tracked target while in sight, else the
  nearest.
- A sight cone, `Skill::fov_deg` across (100, 120, 140, 160 by level,
  section 3), about the view (`bots::in_sight_cone`, on the chest), for
  every other enemy; the tracked target and the attacker pass it on any
  bearing. A bot in a vehicle seat sees all round: its view is the
  seat's, and with the cone the tank driver of
  `a_bot_fires_rockets_at_an_enemy_tank` (`tests/bot_vehicles.rs`) no
  longer held still for the rocket man behind his hull, and the tank
  survived the rocket.
- `Bot::track`: a target let go of within `forget_ticks` (`Bot::prior`)
  is taken again on a third of the drawn reaction and its old error times
  1.25, between `error_floor` and `error_start`; the attacker of the hit
  on half the reaction and 0.75 of `error_start`; anyone else on the full
  ones, as before.
- `Bot::face_hurt`: with nothing in sight, a hit in the last
  `HURT_LOOK_MS` (1.5 s) turns the view back along the damage direction
  at `turn_deg`, the path's keys rotated to keep the path; the cone
  sweeps over the shooter.
- Hearing has no cone (`bots::loudest`, bot-navigation.md section 4): a
  shot behind the bot pulls it as one ahead does.
- `Bot::aware` off (`AWARE=`, `Server::test_bot_aware`) restores all of
  the before.

### 8.1 Before and after

`AWARE=none` replays master line for line: both sets below print master's
match lines (kills, deaths, damage, ticks, stalls) with it, so it is the
before and supplies the `turned` column master's harness lacks.

VERIFIED (measured, ours, `bot_match`, 2026-10-21): `tdm`, 12 bots,
mp_carentan, mp_harbor, mp_depot and mp_brecourt, seeds 1..=4 (16
matches, all ended); `dm`, 10 bots, mp_carentan, mp_harbor and mp_depot,
seeds 1..=4 (12 matches, none ended in 12000 ticks):

| Run | Kills | K/D | Life s | TTK mean / median ms | Wounds | Dead in 5 s | Turned | Answered | Blindsided | Cover hold | Stalled |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `tdm` before | 2908 | 1.00 | 22.0 | 1270 / 0 | 2160 | 76% | 63% | 17% of 4324 | 12% of 2908 | 483913 | 65863 |
| `tdm` after | 2935 | 1.00 | 22.0 | 1357 / 0 | 2179 | 74% | 92% | 19% of 4237 | 13% of 2935 | 642973 | 127392 |
| `dm` before | 3116 | 1.00 | 20.1 | 1253 / 300 | 2333 | 74% | 92% | 37% of 3726 | 2% of 3116 | 635254 | 45053 |
| `dm` after | 2895 | 1.00 | 21.7 | 1136 / 350 | 2275 | 79% | 96% | 28% of 3406 | 14% of 2895 | 574948 | 30322 |

`Turned` is the share of hits whose attacker the bot targeted within 1 s;
`answered` (section 1) adds the trigger down.

- VERIFIED (measured, the table): in `tdm` the attacker becomes the
  target within 1 s after 92% of hits, from 63%.
- VERIFIED (measured, the table): `answered` barely moves (17% to 19% in
  `tdm`). INFERRED (`Bot::trigger`): the trigger waits until the aim's
  miss falls inside the 10-unit cone, and 0.75 of `error_start` (36
  units) decays under it in about 13 ticks at 360 units standing, longer
  moving; the turn and that wait, not the choice, hold the answer past a
  second.
- VERIFIED (measured, the table): in `dm` the share blindsided goes from
  2% to 14% and `answered` falls from 37% to 28%. INFERRED: before, a
  `dm` bot saw every player all round and was on the nearest; with the
  cone a shot from behind lands before a 180 degree turn (12 ticks at 15
  degrees a tick).
- VERIFIED (measured, the table): `tdm` stalls nearly double, `dm` stalls
  fall a third. On the branch with the parts switched off one at a time
  (mp_carentan, mp_depot, mp_brecourt, `tdm`, 12 bots, seeds 1..=4),
  stalled bot-ticks with no target fell from 6793 to 2903 and those
  holding cover against a target rose from 41k to 80k, all on versus all
  off; on seeds 1..=8 the reacquire and hit-by discounts each raised the
  total alone (124k off, 141k and 143k each alone, 178k both). INFERRED:
  the extra stalls are bots fighting longer from cover, not lost bots.
- VERIFIED (measured, release, stock UO paks): `uo_bots_fight_and_sprint_in_tdm`
  seeds 1 and 19 stalled a bot on mp_cassino's roofs (bot-objectives.md
  10.6) on this branch alone; merged with section 7's noise answering and
  6.5's grenade rules seed 1 passes, and the replay list plays 13 for 19,
  whose game there ends on a bot's own frag.

### 8.2 One-sided

VERIFIED (measured, ours, `bot_match`, 2026-10-21): `AWARE=alt`, `tdm`,
12 bots, the four maps above, seeds 1..=16 (64 matches, all ended):

| Arm | Kills | Deaths | K/D | Life s | Dead in 5 s | Turned | Answered | Blindsided | Wins |
|---|---|---|---|---|---|---|---|---|---|
| aware | 6386 | 4351 | 1.47 | 26.7 | 71% | 91% | 21% | 13% | 62 |
| control | 4344 | 6399 | 0.68 | 18.0 | 75% | 59% | 13% | 14% | 2 |

- VERIFIED (measured, the table): the aware side scores 31.9 +- 1.9 more
  a match and wins 62 of 64, as large as a skill level (6.1's `vc_bot_skill
  2` row, +31.9 +- 1.6); in 6.1's shorter eight-bot matches it is +8.7 +-
  1.6.
- VERIFIED (measured, the table): a blindsided death is no likelier on
  the aware side (13% against 14%) though it sees a cone. INFERRED: it
  turns on its attacker and keeps its target where the control flips to
  the nearest.

## 9. Aim and footwork

`Bot::engage`, `Bot::trigger` and `Bot::footwork` in
`crates/server/src/bots.rs`; measured in 6.6.

Before: the error only shrank with time on target and the trigger waited
for the 10-unit cone, so a bot paused about a second on a fresh target
and then hit nearly every round; target motion added nothing, and a
sighted bot never moved.

### 9.1 The miss

Section 18 replaced this with the hand's aim; `AIM=none` keeps it. The
`error`, `error_range` and `moving` terms carry over; the `track_lag`
term became a trail along the target's motion (18.1). Here, the aimed
point is off the target's chest by

`(error + across * track_lag) * (1 + range / error_range) * moving`

- `error` starts at `error_start` on a fresh target and shrinks by
  `error_decay` a tick to `error_floor`, as before.
- `across` is the target's speed across the line from the bot (units/s,
  `EnemyView::velocity` less its part along the line), and `track_lag`
  (seconds, by skill: 0.09, 0.06, 0.045, 0.03) how far the aim trails it.
  At normal skill a target running across at 190 adds 11.4 units, past
  the cone on its own.
- `moving` is `1 + (error_moving - 1) * (speed / 190)^2`. VERIFIED
  (combat doc 2.1, `game.mp.i386.so` 0x385e8): retail's
  `PM_AdjustAimSpreadScale` adds nothing to a settled sight
  (`pmove::weapon::adjust_aim_spread_scale`), so this penalty is the
  brain's own. INFERRED (the formula, a half-key strafe on the sights at
  about 45 units/s): such a strafe costs 4% at normal skill, where linear
  in speed it cost 18%; the footwork side lost more on the linear build
  (6.6).

### 9.2 The trigger

Inside the cone the trigger goes down as before. Past it, it goes down
with chance `(reach - r) / (reach - 1)` a tick, `r` the miss in cones:
`REACH_AUTO` 4.5 for an automatic, `REACH_SEMI` 3 for a semi-automatic,
`REACH_BOLT` 2 for a bolt rifle (`fireTime` 500 ms or more), whose miss
costs a rechamber. An automatic's burst keeps the trigger down while the
miss stays inside its reach. An automatic inside `AUTO_HIP_RANGE` (700)
also fires while its sights come up; the round takes the hip cone, as
retail's spread is the hip one until `fWeaponPosFrac` reaches 1
(`game::combat::spread_deg`). Everything else still waits for the
sights and a settled eye.

Normal skill's reaction went from 5 + rand(6) ticks to 4 + rand(4), its
error start from 48 to 36 and its decay from 0.88 to 0.85, between easy
and hard as before. VERIFIED (measured, ours, a temporary per-tick
trace of the trigger's gates on mp_harbor `dm`, seed 1, 4000 ticks,
before the reaction change): a fresh target's time before the first
round was about 8.7 ticks of reaction, 5 waiting on the sights and 4 on
the aim. INFERRED: the sights' wait is the weapon's `adsTransInTime`,
so the reaction was what was left to cut.

### 9.3 Footwork

Each strafe spell (8 to 23 ticks) rolls once against `Skill::footwork`
(0.25, 0.45, 0.6, 0.75). A spell that wins, while the bot is on its
sights:

- strafes at half key (`right` 63) instead of standing still; a scope
  still holds still;
- with a bolt rifle inside `BACK_OFF_RANGE` (300) walks backwards;
- past `PRONE_RANGE` (900), on a quarter of the chance, lies down for 2
  to 4 s instead of crouching; inside three quarters of that range the
  body gets up. A held cover spot, a sidestep off a teammate's line and
  a flee from a grenade release the stance.

Off the sights the strafe stays full and the crouch roll stays as it
was. `Bot::footwork` false (`FOOTWORK=` in `bot_match`,
`Server::test_bot_footwork`) skips the roll's effects: the sighted body
stands still, never backs off and never lies down.

### 9.4 What the UO bot gates found

VERIFIED (measured, ours, `tests/bots.rs` ignored tests at a stock UO
install, 2026-10-21). This change routed bots into follower pins, all
since fixed (bot-objectives.md 10.6, bot-navigation.md "Jumps", "A goal
that flickers"):

- `four_shooting_uo_bots_end_a_bas_round` (mp_foy, seed 7) died of its own
  panzerschreck off `Bot::think_armor`: fixed by the muzzle check.
- `uo_tdm_games_that_stalled_or_never_sprinted` seed 25 (and 33): on
  mp_cassino's axis roof pin (4939, -1410, 280) the bot, not engaged,
  keyed forward at 115 toward (4896, -1384, 280) and bounced between
  (4939, -1410) and (4957, -1405), never jumping and never at rest long
  enough for `Follower::rest_jump_fails`. Fixed by a replan on landing
  under a level edge.
- Seed 17: on the steps at (1440, -700..-725) the waypoint flipped each
  time the body crossed a step, between (1440, -725, 13) and (1440, -669,
  56) or (1441, -761, 25). The cause was the goal: an enemy seen for a
  tick or two flipped `Bot::goal` and back, and each replan began at the
  node behind the body. Fixed by a plan parked for a goal that comes
  back.

## 10. Peeks and hunts that vary (2026-10-21)

Before (`WARY=none`): one peek pose per spot, a 3 s peek after the threat
left sight, then a walk onto the lost spot (`Bot::recall`, led by half a
second of his velocity) until arrival or `Skill::memory_ticks`.

Now, with `Bot::wary` on (the default; `WARY=` and
`Server::test_bot_wary` turn it off):

- `cover::Sight::alt` gives a spot a second way to show: the other lean
  side, or from a body hidden crouched a crouched lean (`Peek::LowLean`,
  the 0.5 lean limit of `pmove::lean_scale`). `cover::find` also keeps
  the next nearest spot at least 48 units off (`Spot::next`). The server
  rechecks the spot the bot holds, not the one it handed out
  (`Bot::held_spot`).
- A peek that would repeat the spot and pose of the bot's last peek this
  life takes the spot's other pose (`Bot::cover_keys`).
- Ducking back to reload, a bot moves to the neighbouring spot with a
  chance of 0, 3, 4 and 4 in 8 by skill level (`cover::WARY`).
- The hold with the threat out of sight is drawn per loss: 70-89,
  45-74, 40-69 and 35-64 ticks by level.
- The server hands a hunting bot a vantage (`BotView::hunt`,
  `hunt::vantage`): of the graph nodes 160 to 480 units across from the
  lost enemy's spot, reached along edges with no jump or risky step, the
  one nearest the bot (of the 12 nearest) whose standing eye has a clear
  bullet line to the spot. The bot walks there and its view turns onto
  the spot on the way, as it did onto the spot before. Reaching the
  vantage ends the hunt, as reaching the spot did. Without a vantage it
  walks to the spot.
- A seen enemy's spot is walked to for 70, 70, 60 and 50 ticks after he
  was lost, by level, never past `memory_ticks` (100, 140 and 180 at
  levels 1 to 3) (`hunt::HUNT_TICKS`). The memory itself is unchanged,
  so a grenade at the lost spot and `bel`'s hunted bots read it as
  before.

Rejected cuts, VERIFIED (measured, `WARY=alt`, 32 matches): alternate
hidden and peeking spells (6-15 and 10-19 ticks at level 1) lost the wary
side 159 kills against the control's 3031 (10 wins to 17), 2931 to 3007
with the hunt on too; INFERRED: a hidden bot does not see the threat come
back. A vantage 96 to 320 units off the spot matched the control's kills
(3007 to 2980) but 11.4% of its 3199 arrivals died within 2 s, against
8.3% of the control's 931.

### 10.1 Before and after

`WARY=none` replays master: the match lines of the runs below print
master's kills, deaths, damage, ticks and stalls with it.

VERIFIED (measured, ours, `bot_match`, 2026-10-21): `tdm`, 12 bots,
mp_carentan, mp_harbor, mp_depot and mp_brecourt, seeds 1..=4 (16
matches); `dm`, 10 bots, mp_carentan, mp_harbor and mp_depot, seeds
1..=4 (12 matches, none ended). `Stalled` here is the column before 2026-10-21, holds included (section 1).

| Run | Kills | Life s | Dead in 5 s | Peeks | dead in 2 s | Re-peeks | dead in 2 s | Hunts | dead in 2 s | Frags thrown | Stalled |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `tdm` before | 3026 | 30.8 | 76% | 8784 | 5.9% | 3193 (36%) | 5.1% | 974 | 8.2% | 744 | 76323 |
| `tdm` after | 2942 | 29.7 | 73% | 7884 | 7.3% | 1725 (22%) | 6.9% | 4591 | 8.5% | 489 | 49617 |
| `dm` before | 2947 | 21.4 | 78% | 9246 | 9.7% | 3079 (33%) | 8.7% | 533 | 7.7% | 723 | 29586 |
| `dm` after | 2899 | 21.7 | 79% | 8793 | 9.5% | 1513 (17%) | 10.9% | 3523 | 11.4% | 510 | 34195 |

- VERIFIED (measured, the table): re-peeks fall from about a third of
  peeks to a fifth. The rest repeat from a spot with only one way out.
- VERIFIED (measured, the table): hunts reach their vantage four to
  seven times as often as they reached the spot. In `tdm` the share dead
  within 2 s of the arrival holds (8.2% to 8.5%); in `dm` it rises (7.7%
  to 11.4%). INFERRED: the vantage is reached soon after the loss, while
  the enemy is still near the spot. Before, most hunts ended on memory or
  in a fight before they got there.
- VERIFIED (measured, the table): bots throw a third fewer frags.
  INFERRED: a hunt now ends at the vantage, often inside the window in
  which a lost enemy's spot draws a frag (`LOST_THROW_TICKS`). Frag kills
  were 2 and 3 in `tdm` and 1 and 0 in `dm`.
- VERIFIED (measured, the table): `tdm` stalls fall a third. INFERRED:
  bots spend less time on cover spots (cover hold 621573 to 551272
  bot-ticks).

### 10.2 One-sided

VERIFIED (measured, ours, `bot_match`, 2026-10-21): `WARY=alt`, `tdm`,
12 bots, the four maps above, seeds 1..=8 (32 matches):

| Arm | Kills | Deaths | K/D | Peeks | dead in 2 s | Re-peeks | dead in 2 s | Hunts | dead in 2 s | Wins |
|---|---|---|---|---|---|---|---|---|---|---|
| wary | 2975 | 3001 | 0.99 | 8242 | 6.2% | 1874 | 6.4% | 4547 | 8.9% | 15 |
| control | 3001 | 2976 | 1.01 | 8393 | 6.3% | 3033 | 5.5% | 929 | 8.1% | 15 |

- VERIFIED (measured, the table): neither side wins: 26 kills apart over
  32 matches and 15 wins each. Varying the peek and hunting from a
  vantage makes the bots harder to predict without making them score
  more.
- VERIFIED (measured, release, stock UO paks): the shipped cut passes
  `tests/bots.rs` and `tests/bot_vehicles.rs` at 1.1 and UO. The 96 to
  320 unit vantage once failed `uo_tdm_games_that_stalled_or_never_sprinted`
  seeds 13 and 19 (a bot stalled 12 s on mp_cassino's roof, (4890..4990,
  -1320..-1440, 256..303), routed to the drop edge (4990, -1440, 303) to
  (5040, -1490, 48); seed 13 at (720, -1467, 92), waypoint (-192, -384)).
  Merged with section 9 and the follower's landing replans it passes,
  both halves on and each alone. INFERRED: follower stalls the changed
  draws reached. A lone bot placed at each stall spot with its goal
  reached it within 30 s on the old and new follower, so the pins needed
  the whole game's state.

## 11. The walking view and string-pulling (2026-10-21)

`Bot::walk_turn` in `crates/server/src/bots.rs` and `Follower::pull` in
`crates/server/src/nav.rs`; the follower's side is in
`bot-navigation.md` section 3, "String-pulling".

Before this a walking bot's waypoint was the plan's next node on the
32-48 unit grid, and its view took the yaw to it straight from `atan2`
every tick. Across a diagonal the grid's eight neighbours make a stair
of 45 degree kinks, and at every node the view jumped to the next one.

### 11.1 The rules

- The view turns at most `Skill::walk_turn_deg` a tick toward the
  path's heading (18, 24, 30, 36 by level: 360 to 720 deg/s). The keys
  are the path's keys turned into the view's frame (`rekey`'s turn) and
  scaled so the larger is 127, so the body runs along the path at the
  run's speed while the view comes round.
- Only on a plain run: on the ground, off the rungs, level view, full
  forward key and nothing else held, no leap's or creep's foot, no jump,
  run off, push off or unpin, no creep. Those keep the view the walk
  that proved them had (AGENTS.md, "Bots").
- A fight, a launcher on a vehicle, a bipod's linger, a hurt look, a
  cover peek, a recalled spot and a retreat's look set the view after
  the walk, by their own rules, from the tick they take it. The tick
  after one of them lets go the view takes the path's heading at once
  (`Bot::view_owned`: the last think's view was not the walk's), and the
  limit runs again from the tick after that (11.4).
- The waypoint is pulled one node further along the plan a tick when a
  straight walk from the body reaches it (bot-navigation.md section 3).

### 11.2 Before and after

VERIFIED (measured, release `bot_match`, `COD_DIR` at 1.1, 12 bots
`tdm` on mp_carentan, mp_harbor, mp_depot and mp_brecourt, 10 bots `dm`
on the first three, normal skill; master d3b03cb0 with the section 1
columns, then this change with 11.4's hand-back). Turn is the walking
view's mean yaw change a tick, snaps the walking ticks it turned more
than 30 degrees. Stalled, here and in 11.4, is the column before
2026-10-21, holds included (section 1).

| run | kills | life s | accuracy | walking ticks | turn deg | snaps | stalled |
|---|---|---|---|---|---|---|---|
| tdm seeds 1-4 | 3016 / 2955 | 28.5 / 26.7 | 33.9 / 33.4% | 975 332 / 863 241 | 3.87 / 2.93 | 19 926 (2.04%) / 2 196 (0.25%) | 63 151 / 62 899 |
| tdm seeds 5-8 | 2908 / 2949 | 27.9 / 26.9 | 34.1 / 33.7% | 910 296 / 854 828 | 3.83 / 2.97 | 19 050 (2.09%) / 2 245 (0.26%) | 70 669 / 69 137 |
| dm seeds 1-4 | 3129 / 3124 | 19.9 / 20.0 | 33.9 / 33.3% | 521 357 / 517 329 | 4.53 / 3.75 | 10 780 (2.07%) / 3 328 (0.64%) | 33 834 / 31 890 |

Snaps fall eight times in `tdm` and three in `dm`. Most of those left
are hand-backs: a recalled spot's look or a hurt look ends on a tick the
bot walks on both sides of, and the view takes the path at once. Without
the hand-back (the build before 11.4) they were 818 (0.10%), 889
(0.10%) and 412 (0.08%), at the cost 11.4 measures.

Per map, `tdm` seeds 1-4 then 5-8, stalled before / after: mp_carentan
31 133 / 22 209 and 33 031 / 25 534, mp_harbor 9 858 / 10 730 and
8 894 / 14 347, mp_depot 14 592 / 21 736 and 18 333 / 20 589, mp_brecourt
7 568 / 8 224 and 10 411 / 8 667; mp_depot's accuracy 31.9 / 30.8% and
30.6 / 28.7%. Kills move inside the seed sets' spread; `tdm` life is a
second shorter in both sets, `dm`'s level (not looked into). Without
11.4, mp_depot / mp_harbor seeds 1-4 stalled 14 592 / 9 858 before,
24 480 / 9 230 with the turn limit alone, 17 763 / 11 475 with the pull
alone, 28 947 / 13 428 with both (VERIFIED, measured, temporary switches).

### 11.3 The rocket test

`tests/bot_vehicles.rs`'s `a_bot_fires_rockets_at_an_enemy_tank` failed
with this change at a stock UO install: the rocket went out on the same
tick (21) and struck the same face of the tank's hull as on master, and
the tank lived. VERIFIED (measured, temporary logs in `think_armor`,
`Missile::impact` and `vehicle_damage`): master's struck at (4782.927,
-1938.947, 21.149), this build's at (4782.925, -1938.546, 21.183), the
hull at (4673.791, -1979.002) facing -179.83. `vehicle::damage_scale`
takes the behind scale only within 20 degrees of the hull's back axis;
the point lay about 19.99 degrees off it on master (scale 1, 1000
damage, the hull burns and blows up 214 ticks on) and about 20.17 here
(the side's 0.5, 500 damage of 2900, and the tank's gun killed the
shooter). The shooter was set down 600 units dead behind the hull and
walked about 110 units aside while the launcher came up (`think_armor`
keeps the path's keys while the muzzle is not clear), and where that
walk ended decided the zone.

The test now holds him on the back axis with a script `setOrigin` each
tick until the rocket goes (`Server::test_script_set_origin`;
`place_client` respawns the body and the launcher never comes up). He
fires at tick 91 from (5275.4, -1979.0, -7.75) and the tank blows up 214
ticks on (VERIFIED, measured), the kill assertion kept. Held 500 units
back it fires at 21 and 400 back at 52; 700 back, and 900 to 1450 back
unheld, it never does (no ground, or the line blocked).

### 11.4 The hand-back

Trap: without the hand-back the turn limit doubled mp_depot's `tdm`
stalls. VERIFIED (measured, per-tick dumps, mp_depot `tdm` 12 bots, seeds
3 and 5, master then the build without 11.4): stalled bot-ticks 6 342 to
21 294, of them engaged on a held cover spot 4 459 to 16 567 (view within
10 degrees of the target on 69% and 81%), walking 459 to 617. Engaged
bot-ticks rose 84 685 to 115 317, mostly at 600 to 1000 units. Seed 5
then seed 3: fights 744 to 1 171 and 963 to 1 007; target down 40 to 28%
and 39 to 35%; the same pair again within 10 s 189 to 416 and 226 to 299.
INFERRED: after a peek, recall look or fight let go, the view came back to
the path at the walking rate, so the bot leaving cover saw the same enemy
again, dug back in, and the two traded rounds from cover at range.

With the hand-back (VERIFIED, measured, same dumps): stalled 3 240 and
5 386 (master 2 895 and 3 447); seed 5's target down 33% (master 40%).
The same pair again within 10 s still read 385 and 405 against master's
226 and 189; section 13 measures it on eight seeds, where it does not
rise. Without the hand-back
`bots::tests::the_walking_view_takes_the_path_at_once_after_a_fight`
fails.

## 12. Frags that leave the hand and land where thrown (2026-10-21)

`bot_match` on the seeds of 6.5: 1.1 `tdm` with 12 bots on mp_carentan,
mp_harbor, mp_depot and mp_brecourt, seeds 1..=4; `dm` with 10 on
mp_carentan, mp_harbor and mp_depot, seeds 1..=4; UO `tdm` with 10 on
mp_cassino and mp_kursk, seeds 1..=8 and 9..=16, at stock UO paks.
Before is the branch's base (3135508b), after the branch tip. Over the
`tdm` table before, 297 frags hit 5 players and killed 1.

### 12.1 Why they missed

From temporary `bot_match` dumps of every release, live frag spawn and
`MOD_GRENADE_SPLASH` blast with the bodies around it:

- VERIFIED (measured, ours, mp_harbor `tdm` seed 1): of 31 releases, 1
  frag left the hand. The 1.1 hold ran 6 to 13 ticks from the tick
  `ps.weapon` showed the frag. A release throws only once the raise
  (`raiseTime 0.25`) and the pullback's `holdFireTime 0.6` have run
  (`weapons/mp/fraggrenade_mp` in `pak0.pk3`; `cod11-combat.md` 1.11).
  Released sooner, the throw waits for the pin, and the next tick's cmd
  asked for the rifle, whose switch cancels the pullback with the clip
  unspent (1.14). UO's hold of 24 to 33 ticks outlasts the raise and
  its `holdFireTime 0.5` (`stielhandgranate_mp` in `pakuo04.pk3`), so
  UO frags left, and the pin fix alone leaves UO's `bot_match` lines
  bit for bit.
- VERIFIED (measured, ours, the `tdm` seeds with the pin fixed and the
  old probe, 229 throws): a frag went off a median 494 units from the
  point it was thrown at, a median 1.8 times the range out. A frag
  leaves at `projectileSpeed 960` plus `projectileSpeedUp 120`
  (`fraggrenade_mp`) at any pitch, so the probe's lobs of 5 to 40
  degrees over the line, picked for a clear arc, came down past 1 100
  units on flat ground and rolled on.
- VERIFIED (measured, ours, the same dumps with the landing fixed, the
  `tdm` seeds, 47 throws at an enemy in sight): an enemy facing the
  thrower within 60 degrees and fighting him took 81.5 damage a throw
  (20 throws), one facing him and fighting another 69.5 (11), one
  facing away 26.9 (10). INFERRED: a frag off a high lob comes down
  from above, and an enemy dug in against the thrower stays put, so there
  is no rule on where the enemy looks.

### 12.2 What changed

- The hold keeps the trigger down until the pin is out
  (`BotView::grenade_ready`: `grenadeTimeLeft` set, `weaponDelay` 1 or
  less; `Bot::think_cook`). A pin not out `PIN_WAIT_TICKS` (30) past
  the hold puts the frag back.
- `Server::lob_probe` tries throws from 10 degrees down to 85 up, every
  5 (the view's pitch clamp is now 85, `PITCH_MOST`). Each whose open
  arc comes down within `LOB_REACH_SLACK` (250) of the range is flown
  by `Missiles::predict_flight` to its burst: the full fuse on 1.1 and
  1.5, `COOK_MEAN_MS` (1 250) short of it on UO. The one going off
  nearest the target is taken, within `LOB_MISS` (150).
- The self-burst refusal stays, and also holds for the pitch
  `LOB_SLACK` (2 degrees) either side, the release's slack. VERIFIED
  (measured, ours, UO mp_cassino seed 3, a per-tick dump): a bot let go
  at pitch -34.45 with the probe's lob at -35; the frag went 110 units
  out, rose to z 233, came back and went off 220 behind him, killing
  him and a teammate. Bodies in the early arc are grown 24 units
  (`LOB_BODY_GROW`).
- The teammate check stays while `scr_friendlyfire` is non-zero, and
  now covers each mate where his pace takes him 1 and 2 s on
  (`MATE_LEAD_S`) and every point the frag may go off at under UO's
  spread of cooks. VERIFIED (`maps/mp/gametypes/tdm.gsc` in
  `pak5.pk3`): 1.1's `tdm` sets `scr_friendlyfire` "0" when unset.
- The view goes to the lob less the gun's offset off it, and the release
  waits for the view plus the offset on the lob, as a shot does
  (`BotView::gun_offset`). VERIFIED (measured, ours, UO mp_cassino seed
  13 of `uo_tdm_games_that_stalled_or_never_sprinted`, a per-tick dump):
  a blast at tick 1130 kicked a bot's gun 17.3 degrees up; it let go at
  1137 with its view on the probe's lob at 70 degrees up, the frag left
  at about 87, came down on it and killed it.
- A cook dropped by a thrower killed holding it (`Missile::dropped`) is
  run from by every bot in reach, seen or not. VERIFIED (measured, ours,
  UO mp_cassino seed 7, the kill lines): a thrower died at tick 7785
  and his frag killed a teammate at 7856, 3.55 s on. INFERRED: a frag a
  UO bot lets go has at most 3.05 s left (its spoon is down 19 ticks or
  more), so this one fell from the hand.
- `GRENADE_RANGE` 600 to 800, `COVER_THROW_TICKS` 30 to 20 (1 s in
  sight with no round at him). The 10 s cooldown stays.

VERIFIED (measured, ours, the seeds above, thrown, hits and grenade
kills; before the slack, mate, gun offset and dropped rules):

| Arm | `tdm` | `dm` | UO 1..=8 |
|---|---|---|---|
| pin and probe, range 600, cover 30 | 181 / 188 / 49 | 234 / 73 / 24 | 73 / 51 / 17 |
| range 800 | 445 / 430 / 108 | 684 / 245 / 83 | 196 / 127 / 37 |
| and cover 20 | 509 / 458 / 125 | 767 / 296 / 118 | 243 / 174 / 53 |
| and cooldown 120 | 541 / 503 / 135 | 829 / 312 / 108 | 240 / 165 / 55 |

The 120 cooldown is not taken: little gain, and one own frag in `tdm`.
Tried and dropped (VERIFIED, measured, the same seeds): picking the lob
nearest the view's pitch (`tdm` grenade kills 118 to 83; it takes low
throws that roll), scoring a second on the ground as 100 units of miss
(no clear gain, 2 own and team frags on UO), and holding the release
while the body moves (no change).

### 12.3 Before and after

VERIFIED (measured, ours, `bot_match`, 2026-10-21):

| Run | Thrown | Hits | Grenade kills | Own | Team | Kills | Suicides | Teamkills | Life s |
|---|---|---|---|---|---|---|---|---|---|
| `tdm` before | 297 | 5 | 1 | 0 | 0 | 2977 | 0 | 0 | 27.6 |
| `tdm` after | 492 | 421 | 102 | 0 | 0 | 2963 | 0 | 0 | 27.4 |
| `dm` before | 298 | 0 | 0 | 0 | 0 | 3076 | 0 | 0 | 20.3 |
| `dm` after | 721 | 264 | 99 | 0 | 0 | 3070 | 0 | 0 | 20.4 |
| UO 1..=8 before | 74 | 23 | 3 | 0 | 1 | 1649 | 35 | 18 | 45.4 |
| UO 1..=8 after | 181 | 100 | 34 | 0 | 0 | 1636 | 59 | 12 | 44.6 |
| UO 9..=16 before | 60 | 15 | 1 | 0 | 0 | 1627 | 44 | 13 | 44.4 |
| UO 9..=16 after | 180 | 126 | 34 | 0 | 1 | 1644 | 44 | 8 | 46.8 |
| `tdm`, `scr_friendlyfire 1`, before | 251 | 12 | 0 | 0 | 4 | 2963 | 2 | 34 | 27.1 |
| `tdm`, `scr_friendlyfire 1`, after | 321 | 374 | 90 | 0 | 1 | 2924 | 0 | 24 | 27.9 |

- VERIFIED (measured, the dumps, the first arm of 12.2's table): a
  frag goes off a median 50 units from its target in `tdm`, 47 in `dm`
  and 86 on UO.
- Kills per life hold (every player is a bot: kills match deaths), and
  `tdm` ends on the score limit, so frags take kills from the guns:
  102 of 2963.
- VERIFIED (measured, the kill lines): UO's suicides rise 35 to 59 on
  seeds 1..=8, none a frag: `MOD_SUICIDE` (the bot's `kill` when dry or
  wedged) 21 to 44 over seven mp_cassino seeds, the rest vehicle blasts
  and crushes; on seeds 9..=16 33 against 32. VERIFIED (measured, a dump
  of each `kill` and a per-tick ammo dump): 30 of 33 were the dry rule,
  on bots that spawned with every weapon at 0 rounds, a server bug fixed
  below.

The zero-ammo spawn (fixed 2026-10-21): a vehicle round delivered between
the script threads and the spawn pass applied every slot's queued weapon
ops, the spawning bot's loadout among them, to the old life's
playerstate, and the spawn pass then emptied the ammo arrays
(`cod11-gsc-object-model.md` 20, "The spawn's memset and the weapon op
queue"). INFERRED, off the code: nothing in the path is a bot's, so a
human spawning in such a frame came up dry too. VERIFIED (measured, ours,
`bot_match`, UO `tdm` with 10 bots, seeds 1..=8, base 028799bf with and
without the fix, the kill lines):

| Run | Kills on mp_cassino | Suicides on mp_cassino | `MOD_SUICIDE` mp_cassino / mp_kursk |
|---|---|---|---|
| before | 1402 | 47 | 43 / 2 |
| after | 1331 | 10 | 7 / 2 |

mp_kursk's matches play out bit for bit alike (237 kills, 10
suicides): no vehicle round landed with a spawn queued there.

The `MOD_SUICIDE`s left: VERIFIED (measured, ours, `bot_match` UO `tdm`,
10 bots, mp_cassino seeds 1..=8, master 589516ea, each `kill` logged with
its reason): four, none dry. Three were `Bot::wedged` (seed 3 ticks 1229
and 7330, seed 4 tick 3518): bodies held all solid in a caulk or clip
brush, which a stance rise's late box put them in (`coduo-movement.md`
18, a server bug, fixed 2026-10-21). One was `BotView::stranded` (seed 4
tick 8957, a ledge at (937, -947, 108) off the graph), the wander's
intended out. No script `suicide()`, no cooked frag and no prone body
under a vehicle. With the UO box the same seeds end with 0
`MOD_SUICIDE` (1352 kills, 2 suicides, both a tank's
`MOD_EXPLOSIVE` death blast: one charged to the victim, one to the
world).
- The one team frag left (UO mp_cassino seed 16, tick 2758) went off
  299 units from where the teammate stood at the release; its cause is
  not traced. INFERRED: the mate ran into it, or the frag met a
  vehicle, which `predict_flight` does not fly against.

## 13. Fights broken off and taken up again (2026-10-21)

Follows 11.4's mp_depot reading (the same pair again within 10 s, 226 and
189 to 385 and 405). `bot_match` counts it (section 1's fights row).

### 13.1 Measured

VERIFIED (measured, ours, release `bot_match`, `COD_DIR` at 1.1, master
028799bf, normal skill): `tdm` 12 bots and `dm` 10 bots, seeds 1..=4.

| Run | Fights | Target down | Bot down | Broken | Re-engaged | Share of broken |
|---|---|---|---|---|---|---|
| `tdm` mp_carentan | 5 607 | 30.8% | 19.8% | 49.4% | 1 454 | 52.5% |
| `tdm` mp_harbor | 3 911 | 42.0% | 26.3% | 31.7% | 482 | 38.9% |
| `tdm` mp_depot | 4 020 | 34.3% | 23.4% | 42.3% | 729 | 42.9% |
| `tdm` mp_brecourt | 3 414 | 41.7% | 27.4% | 30.9% | 444 | 42.0% |
| `tdm` total | 16 952 | 36.4% | 23.7% | 39.9% | 3 109 | 46.0% |
| `dm` mp_carentan | 4 642 | 39.4% | 28.5% | 32.1% | 464 | 31.1% |
| `dm` mp_harbor | 3 546 | 40.9% | 30.2% | 28.9% | 258 | 25.1% |
| `dm` mp_depot | 3 629 | 37.3% | 28.0% | 34.7% | 359 | 28.5% |
| `dm` total | 11 817 | 39.2% | 28.8% | 31.9% | 1 081 | 28.6% |

- VERIFIED (measured, the table): mp_carentan, not mp_depot, takes up
  the most fights again. `dm` re-engages about half as often as `tdm`.
- VERIFIED (measured, release `bot_match` with this column on c1faaeec,
  3135508b's first parent, before the walking view, against master;
  mp_depot `tdm` 12 bots seeds 1..=8): 1 885 re-engagements (50.1% of
  broken fights) before, 1 748 (46.9%) on master; target down 34.5% and
  33.9%. Per seed, before then master: 157 / 148, 219 / 175, 208 / 234,
  251 / 172, 209 / 200, 211 / 333, 286 / 196, 344 / 290. INFERRED: the
  walking view did not raise it; 11.4's rise was two seeds' spread on a
  temporary count.

### 13.2 What the bots do

VERIFIED (measured, ours, a temporary per-tick dump of each bot's brain
before its think: target and its lost ticks, the server's enemy, goal,
cover hold, retreat, recall, vantage, hurt; mp_depot `tdm` 12 bots seeds
1..=8 on master, the fights recounted off the dump to the same 1 748):

- A target is dropped for another 5 167 times with the old target alive.
  4 621 (89%) fall on the first tick the old target is out of the bot's
  sight, 4 516 go to someone other than the bot's attacker, and none to
  a new enemy while the old one stayed in sight.
- On the tick before the old target came back into the bot's view, the
  bot was fighting another enemy 880 times (50%), on a cover spot 448
  (26%), walking to a health item 100, roaming 89, walking to a hunt's
  vantage 88 or its spot 23, to a heard noise 87, retreating 30, running
  from a frag 3.
- Within 10 s of the re-engagement the bot died or its target did: from
  another fight 389 and 314, from a cover spot 152 and 215, from a hunt
  37 and 54, from a retreat 18 and 6.

INFERRED (read off `bots::pick_enemy` and the dump): the tracked target
sets the range a new enemy has to beat only while he is in sight. One
who ducks behind his cover for a tick sets none, the nearest enemy in
the cone wins, and the old target goes to `Bot::prior`. Two groups
peeking at each other from cover spots at range hand each bot from one
enemy to the next at every duck, and the first comes back into view when
the next ducks. Hunts and retreats take up a fight broken off 6% and 2%
of the time.

### 13.3 Waiting on a target that ducks (not shipped)

The obvious cut: a tracked target out of sight for under 20 ticks keeps
his range as the bar, and with nothing nearer than `RETARGET_RATIO` of it
in sight the bot has no enemy and waits on him (the attacker still comes
first). Four arms, each a temporary switch: always; only with the target's
health under the bot's; only with no other enemy in sight facing the bot
(his view within 15 degrees of the bot's eye); only while the bot holds a
cover spot.

VERIFIED (measured, ours, release `bot_match`, mp_depot `tdm` 12 bots
seeds 1..=8, both sides on the arm):

| Arm | Re-engaged | Target down | Kills | Accuracy | TTK mean ms | Blindsided | Stalled / held |
|---|---|---|---|---|---|---|---|
| master | 1 748 | 33.9% | 1 431 | 28.5% | 1 626 | 8% | 1 842 / 36 302 |
| always | 826 | 41.3% | 1 443 | 29.6% | 1 650 | 15% | 1 627 / 39 942 |
| target hurt more | 1 434 | 35.3% | 1 485 | 28.1% | 1 402 | 9% | 2 497 / 42 008 |
| no one facing | 1 862 | 33.3% | 1 434 | 26.2% | 1 965 | 10% | 1 672 / 51 727 |
| on cover | 871 | 39.5% | 1 435 | 31.0% | 1 462 | 13% | 2 411 / 41 174 |

VERIFIED (measured, the same build, `tdm` 12 bots on mp_carentan,
mp_harbor, mp_depot and mp_brecourt, seeds 1..=4, the arm on axis in one
run and on allies in the other, 32 matches; master's two sides score
1 448 and 1 515 there):

| Arm | Kills, arm / other side | Re-engaged, arm / other side | Blindsided, arm |
|---|---|---|---|
| always | 2 887 / 2 949 | 1 324 / 3 094 | 17% |
| target hurt more | 2 893 / 2 963 | 2 379 / 3 108 | 12-13% |
| no one facing | 2 924 / 2 976 | 3 034 / 3 442 | 10-12% |
| on cover | 2 945 / 2 928 | 1 804 / 3 206 | 13% |

- VERIFIED (measured, the tables): every arm that cuts re-engagements
  raises the deaths to an enemy the bot never targeted, and no arm wins
  fights: the side on it scores 17 more to 70 fewer kills over 32
  matches, inside the spread section 10.2 read as neither side winning.
- VERIFIED (measured, the first table): every arm raises mp_depot's
  held ticks, by 10% to 42%.
- INFERRED: turning to the enemy in sight when the target ducks is not
  a mistake the yardstick can see: the bot that waits is shot by the one
  it passed over as often as it finishes the one it waited on. The
  re-engagement count stays a column, the brain is unchanged.

The hunts and retreats that take a fight up again are too few to move
anything (13.2), and of them only a retreat's lose more than they win.

## 14. CoD2 bots on the mounted guns (2026-10-22)

Retail has no bots and vcod's CoD1 brain mans no gun, so the rule is
vcod's own (`bots::cod2::gun`): in `dm` and `tdm` a bot that sees an
enemy inside a free gun's yaw arc and more than 250 units from it, with
the spot 40 behind the gun within 320 of its feet, within 40 of its
height and walked to unblocked, goes there (5 s at most) and taps use
once the cursor hint names the gun. On the gun it turns the barrel 12
degrees a tick onto the nearest foe in the arc it has a clear shot at
and holds the trigger within 3 degrees of him; 4 s with none, it taps
use and lets go. `GUNS=0` turns it off.

`GAME=cod2 MAPS=mp_carentan GAMETYPE=tdm BOTS=8 bot_match 1 2 3 4`, 6000
ticks a seed, summed:

| | kills | hits / rounds | held bot-ticks | mounts | seconds on a gun | kills off a gun |
|---|---|---|---|---|---|---|
| `GUNS=0` | 220 | 394 / 1691 | 20 | 0 | 0 | 0 |
| guns | 238 | 441 / 1767 | 356 | 10 | 50 | 10 |

Rounds count the carried weapons' clips only, so a gun's do not show.
Seed 3 meets no enemy in an arc and plays as before. A first build
reset the gunner's idle clock only on a sighting and let go the tick
after the mount: 68 mounts for 78 s on the guns.


## 15. CoD2 bots on CoD1's tick order (2026-10-23)

CoD2's bots run the shared driver (`bots::driver::step`) in CoD1's order
(bot-navigation.md 3): the noises heard ahead of the frame's reads, the
hunt's vantage after the retreat's cover, the lob probed after the drop
probe. The yields no longer count a CoD2 body killed this frame as a
teammate. VERIFIED (measured): with the order move and that fix every
wide and gate line was byte-identical to the base, `DIGEST=250`
included; no bot pass on these seeds fell between a kill and the
victim's end frame.

The two route consts stay off for CoD2 (`crates/server/src/cod2/bots.rs`:
`YIELDS` and `PMOVE_PROBES` false), so the two games share one order but
not one route. Measured with `GAME=cod2 TICKS=12000 bot_match`, seeds
1..=8, mp_toujane, mp_carentan, mp_dawnville, mp_burgundy, a process per
match, summed (32 matches a gametype; "wide"), and the `bm2_tdm` gate set
(seeds 1..=4, 6000 ticks, mp_dawnville, mp_carentan, mp_toujane).
VERIFIED (measured, ours, release):

| | kills | hits / rounds | acc | stalled | held | ladder | mantles | retreats | smokes |
|---|---|---|---|---|---|---|---|---|---|
| tdm wide, base | 4171 | 6047 / 33103 | 0.183 | 1608 | 68918 | 5193 | 1676 | 2876 | 355 |
| tdm wide, `YIELDS` | 4171 | 6240 / 33487 | 0.186 | 2783 | 78426 | 4837 | 1727 | 2917 | 380 |
| tdm wide, `PMOVE_PROBES` | 4231 | 6414 / 33799 | 0.190 | 1700 | 76242 | 3810 | 1683 | 2937 | 356 |
| dm wide, base | 4361 | 6627 / 36433 | 0.182 | 112 | 81737 | 3994 | 1350 | 3058 | 430 |
| `bm2_tdm`, base | 758 | 1091 / 5771 | 0.189 | 169 | 12665 | 1110 | 331 | 506 | 75 |
| `bm2_tdm`, `PMOVE_PROBES` | 740 | 1076 / 5460 | 0.197 | 842 | 12406 | 1079 | 320 | 462 | 79 |

- `YIELDS` (ladder and path yields, `team::crowd`, blocked nodes): tdm
  stalls up 73% for the same kills (169 to 491 on the 6000-tick four-map
  set); dm unchanged.
- `PMOVE_PROBES` (lip, push off the rungs, unpin, stepping CoD1's pmove on
  `bots::cod2::move_ps`'s body under the graph's rules): ladder ticks down
  a quarter in tdm, but stalls rise in every sum; two gate seeds that
  never stalled stall (mp_toujane 3 for 408 ticks, mp_dawnville 4 for
  225), and dm seed 4 on mp_carentan for 128. Where to look first if it
  is tried again.

## 16. The CoD2 graph walked under CoD2's pmove (2026-10-24)

CoD2's graph is flooded and proved under 1.5's rules
(`bots::cod2::GRAPH_RULES`), while its bots move by CoD2's pmove. A seam
in the walks (`nav::WalkStep`, `nav::with_step`, bot-navigation.md 2,
"Other rules") takes a CoD2 step (`bots::cod2::Cod2Walk`): each walk's
body is a CoD2 playerstate stepped by `vcod_cod2::pmove::player_step` over
the level's `Rules`, the cmd through `to_cmd2`, read back into the CoD1
body the walk tests. It is off: `Cod2Bots::nav_tick` hands the build no
step, and `Cod2Walk` stays one argument from on. The seam left every CoD1
and UO gate byte-identical.

Graph sizes, nodes / edges / mantle edges (VERIFIED, measured, ours,
release, the `<map> seed 1: graph` line of `bot_match GAME=cod2
GAMETYPE=tdm`):

| map | 1.5's rules | CoD2's pmove |
|---|---|---|
| mp_toujane | 7386 / 53825 / 352 | 7476 / 54416 / 337 |
| mp_carentan | 10168 / 72645 / 319 | 10124 / 72157 / 342 |
| mp_dawnville | 9700 / 69989 / 369 | 9727 / 70088 / 361 |
| mp_burgundy | 7353 / 52897 / 357 | 7352 / 52675 / 369 |

The build costs the same: a one-tick `bot_match`, build included, took
2154 / 2159 ms (mp_toujane, 1.5's / CoD2's), 1919 / 1780, 1522 / 1526 and
1606 / 1698.

Why off, VERIFIED (measured, ours, release, section 15's wide run; K/D
0.99-1.00 in every arm): the graph under CoD2 alone (`graph`) and with the
follower's walks under it too (`graph + follower`) moved kills within the
seeds' spread (tdm 4171 to 4282 and 4252, dm 4361 to 4278 and 4334) and
raised stalls in lumps on single seeds: `graph + follower` stalled
mp_carentan tdm seed 3 for 815 ticks (12 before), bots pacing round (755,
1800..1890, 8) beside the mantle edges down to the yard at z -40, and
`bm2_tdm` 169 to 966. The two graphs hold the same nodes and mantle edges
there, and `graph` stalls that seed 17 ticks, so the follower's walks
under the new step moved it.

## 17. CoD2's bot pass under the graph's rules (2026-10-24)

Two bugs, fixed:

1. `Cod2Server::step_bots` ran the driver outside any `nav::with_rules`,
   so the pass's walks (`Follower::rest_jump_fails`, the re-proofs)
   stepped under 1.1's rules while the graph was proved under
   `bots::cod2::GRAPH_RULES` (1.5's). The pass now runs under the graph's
   `NavGraph::moving` (rules and step), `GRAPH_RULES` until the graph
   lands, as `engine::tick` runs CoD1's under the server's rules
   (`fix1`).
2. `bots::cod2::cod1_body` never set `jump_penalty`, so `nav::jump_ready`
   never waited out CoD2's penalty and the rest-jump test jumped full
   height. It now copies CoD2's `JUMPING` (`pm_flags` 0x80000) and
   `pm_time` into 1.5's `jump_penalty` and `knockback_ms`, with
   `jump_origin_z`, as `Cod2Walk` maps them (`fix2`, on top of `fix1`).

VERIFIED (measured, ours, release; section 15's runs; `bm2_tdm` 12
matches, `bm2_dm_skills` 8):

| | kills | hits / rounds | acc | stalled | held | ladder | mantles | retreats |
|---|---|---|---|---|---|---|---|---|
| tdm wide, base | 4171 | 6047 / 33103 | 0.183 | 1608 | 68918 | 5193 | 1676 | 2876 |
| tdm wide, fix2 | 4177 | 6205 / 32764 | 0.189 | 2515 | 66370 | 5212 | 1812 | 2888 |
| dm wide, base | 4361 | 6627 / 36433 | 0.182 | 112 | 81737 | 3994 | 1350 | 3058 |
| dm wide, fix2 | 4303 | 6576 / 36512 | 0.180 | 148 | 86512 | 4335 | 1302 | 3003 |
| `bm2_tdm`, base | 758 | 1091 / 5771 | 0.189 | 169 | 12665 | 1110 | 331 | 506 |
| `bm2_tdm`, fix2 | 751 | 1036 / 5181 | 0.200 | 46 | 10054 | 1016 | 351 | 457 |
| `bm2_dm_skills`, base | 593 | 914 / 4857 | 0.188 | 5 | 5881 | 1089 | 202 | 405 |
| `bm2_dm_skills`, fix2 | 570 | 882 / 5082 | 0.174 | 5 | 9264 | 891 | 179 | 399 |

- Both fixes stay: the follower walks and waits as the graph was proved.
  Kills hold. Wide tdm stalls rose (by map, burgundy / carentan /
  dawnville / toujane: 0 / 289 / 740 / 579 to 56 / 352 / 609 / 1498),
  nearly all on mp_toujane seeds 3 and 4; 17.1 took them down.
- On top of both, `PMOVE_PROBES` (tdm wide stalls 3179, `bm2_tdm` 1028,
  kills flat) and `Cod2Walk` (`walk`: wide stalls tdm 1746, dm 20, kills
  +10 and +30, but `bm2_tdm` 46 to 567, one mp_carentan seed for 474,
  ladder ticks +60%) both stay off.

### 17.1 Looks on a mantle edge and the slow onto a stop (2026-10-24)

Three brain fixes on top of section 17's `fix2`:

1. `mantle`: on a CoD2 mantle edge (`BotView::mantle`, off
   `nav::Follower::mantling`) neither the recall look nor `face_hurt`
   turns the view. CoD2's pmove offers the mantle only to a view facing
   the wall (INFERRED: `find_surface`, `vcod_cod2::pmove::mantle`, traces
   along the view's forward), so a bot that turned toward a remembered
   enemy at mp_toujane's yard wall (400-560, 1563, 4) backed off the edge.
   `cover_unseen` is left alone: a held spot walks its own keys straight
   to the spot. CoD1 and UO graphs carry no mantle edge, and their gates
   match the base byte for byte.
2. `stop`: the slow onto a leap's foot or a held node
   (`Follower::holding`) set the forward key along the view; under a
   recall look the view is off the path and the keys were turned to keep
   it, so the slow walked the body away from its stop. Seed 3 of
   `bm2_tdm` ran a bot to node (736, 1881, 8), the recall look turned the
   view to (928, 1523), and the slow's +127 ground it into the corner at
   (761, 1799, 8) for 335 ticks (INFERRED from a per-tick trace: view yaw
   -62, keys 127 / -31, waypoint at yaw 106). The slow now scales both
   turned keys under a recall look. Skipping the recall look while
   slowing instead did as well on CoD2 (tdm wide stalls 113) but raised
   CoD1's (5736 to 12014). VERIFIED (measured).
3. `dead`: an edge the follower gets stuck on again while still avoided
   goes on a dead list that the fallback plan also leaves out until it
   expires (`nav::Follower::avoid_edge`). Before, a plan with no way round
   an avoided edge went through it again however often it failed.

VERIFIED (measured, ours, release; same runs, `base` = `fix2`):

| | kills | hits / rounds | acc | stalled | held | ladder | mantles | retreats |
|---|---|---|---|---|---|---|---|---|
| tdm wide, base | 4177 | 6205 / 32764 | 0.189 | 2515 | 66370 | 5212 | 1812 | 2888 |
| tdm wide, dead | 4270 | 6442 / 33245 | 0.194 | 149 | 75102 | 4883 | 1874 | 3014 |
| dm wide, base | 4303 | 6576 / 36512 | 0.180 | 148 | 86512 | 4335 | 1302 | 3003 |
| dm wide, dead | 4333 | 6545 / 37299 | 0.175 | 50 | 80615 | 4037 | 1429 | 3066 |
| `bm2_tdm`, base | 751 | 1036 / 5181 | 0.200 | 46 | 10054 | 1016 | 351 | 457 |
| `bm2_tdm`, dead | 744 | 1145 / 5421 | 0.211 | 0 | 11041 | 1286 | 427 | 502 |
| `bm2_dm_skills`, base | 570 | 882 / 5082 | 0.174 | 5 | 9264 | 891 | 179 | 399 |
| `bm2_dm_skills`, dead | 564 | 894 / 5057 | 0.177 | 0 | 9164 | 842 | 209 | 409 |

- Steps: `mantle` alone cut tdm wide stalls 2515 to 914 (dawnville to 0)
  but raised `bm2_tdm` to 335 (the carentan corner); `stop` took every
  set to 0 but dm wide's 50, one run at mp_toujane's long mantle (1180,
  1860, 70), an edge longer than the `OFF_PATH_STEPS` spacings that
  `Follower::follow`'s off-path check clears (left as is; exempting it
  grew the roof stall site). `dead` leaves CoD2 where `stop` had it but
  for tdm wide's 149, one run on mp_toujane.
- `stop` and `dead` are in the shared brain and move CoD1 and UO. Gate
  sums, base / stop / dead: CoD1's 12 `bm11*` sets kills 12144 / 12236 /
  12238, stalls 5736 / 4636 / 4892; UO's five `bmuo*` sets kills 2963 /
  2868 / 2844, stalls 969 / 1687 / 2782. Nearly every stall is on mp_ship
  (UO base 819 of 969), a few lumps of one bot held 300 to 3000 ticks;
  17.2's mp_ship runs separate the arms.
- Trap (`stop` without `dead`), VERIFIED (measured, per-tick trace): the
  deck slalom on mp_ship at (3549..3581, -38..135, 352), nodes 2946, 3846,
  5293, 6229, 6697 and 4494, each edge held. Recall-looking bots now slow
  onto each held node and get in. The way out, 6697 (3581, 89) to 4494
  (3552, 135), works only with the graph's running start along the edge;
  from rest on the node, or from (3572, 94), it pins at y 96.9 against
  the wall at y 112 (`walk_with`, UO rules, `Arith::Stored`). The follower
  avoided the edge, found no way round, planned through it again from
  rest, and so on: one bot of UO skills seed 15 for 8624 ticks. INFERRED
  (`Bot::think`'s stall check): each hold sets `BotView::stop`, which
  restarts the stall circle, so no unstick spell broke the loop. With
  `dead` the slalom costs 0 and 2 stall ticks in 17.2's two UO runs.
- What stayed is bots held on or at a ladder with their keys on and
  neither the stall circle nor `STUCK_TICKS` noticing; 17.2 fixes both.

### 17.2 Bots frozen on or at mp_ship's ladders (2026-10-24)

Section 17.1's leftover: bots held on or at a ladder with their keys on,
with neither `Bot::think`'s stall circle nor the follower's `STUCK_TICKS`
firing. Two causes, each read off a per-tick trace of the bot's cmd,
view, stage and follower (VERIFIED, measured, ours, release):

1. `throw`: a grenade throw started on the rungs, or at a ladder's foot
   as the body grabbed it. The rungs hold `ps.weapon` at 0, so
   `think_grenade` asked for the frag every tick and never saw it in
   hand. The stage machine returns before `think_wander`, where the
   stall circle runs, and a throw's `Goal::Hold` resets the follower each
   tick, so its clock and `idle` never moved. UO seed 46's bot at (3808,
   65, 492) sat in `ToGrenade` from tick 7589 with `ps.weapon` 0 and
   `weaponTime` 0; the gates' `bmuo_cover_alt` seed 1 (slot 2 from tick
   4065) and `bm11_cover_alt` seed 2 (slot 5) are the same at the hold
   ladder's foot (3280, -449, -61). Now no throw starts on the rungs, and
   a throw stage whose wanted weapon is not in hand while the weapon
   machine sits idle for more than `SWITCH_IDLE_TICKS` (20) is given up
   (`Bot::switch_idle`). That is also the detector: it catches any switch
   that never lands, not only the ladder's.
2. `rungs`: a plan made halfway up a ladder starts at the nearer end
   (`NavGraph::nearest_ladder_end`), and with the next node the ladder's
   other end the follower sent the body back to the start first. UO
   skills seed 1's bot on the hold ladder at (3370, -380) chased a goal
   that flipped between two enemies: each flip parked one plan and took
   up the other (`Parked`), one with the foot next, one with the head
   next, and the bot went up and down between z 188 and 202 for 361
   ticks, closing on each waypoint in turn so neither check fired. A plan
   starting at a ladder end whose next node is the ladder's other end,
   taken with the body on the rungs between them and within a node
   spacing of the foot, now heads for the other end
   (bot-navigation.md section 3).

mp_ship alone, as in 17.1 (base is 17.1's `dead`, which is 566a8d80's
code):

| mp_ship, seeds | | kills | stalled | seeds with a stall |
|---|---|---|---|---|
| UO tdm, 1-48 | base | 3880 | 5434 | 13 |
| | throw | 3872 | 2416 | 13 |
| | rungs | 3843 | 1100 | 10 |
| UO tdm, skills 0 against 3, 1-28 | base | 2030 | 2965 | 11 |
| | throw | 2072 | 1824 | 11 |
| | rungs | 2091 | 876 | 5 |
| 1.1 tdm, 1-48 | base | 4334 | 3960 | 19 |
| | throw | 4328 | 2767 | 17 |
| | rungs | 4388 | 493 | 11 |

The gate sets, kills / stalls, with
section 17's base (b2d7c818, before 17.1) for reference:

| | b2d7c818 | base | throw | rungs |
|---|---|---|---|---|
| `bmuo_tdm` | 869 / 106 | 881 / 272 | 885 / 0 | 872 / 0 |
| `bmuo_skills` | 667 / 27 | 623 / 371 | 623 / 371 | 666 / 0 |
| `bmuo_cover_alt` | 724 / 722 | 696 / 2106 | 721 / 18 | 713 / 228 |
| `bmuo_retreat_alt` | 703 / 101 | 644 / 33 | 645 / 187 | 667 / 260 |
| UO's five `bmuo*` | 2963 / 969 | 2844 / 2782 | | 2918 / 488 |
| CoD1's 12 `bm11*` | 12144 / 5736 | 12238 / 4892 | | 12298 / 2813 |
| `bm2_tdm` | 751 / 46 | 744 / 0 | | 745 / 0 |
| `bm2_dm_skills` | 570 / 5 | 564 / 0 | | 558 / 0 |

- Both stay. Stalls fall in every set but `bmuo_retreat_alt` (a few
  lumps either way on four seeds); UO's gate stalls end at half
  b2d7c818's, CoD1's at half of 17.1's. UO's gate kills are 45 under
  b2d7c818's, 74 over the base; 17.1's own drop (`bmuo_skills`,
  `bmuo_retreat_alt`) is mostly won back.
- `nav11` and `navuo` match the base; `dg*` move with the play; the
  ignored `bots.rs`, `bot_admin`, `bot_objectives` and `bot_vehicles`
  tests pass at both installs (`bot_vehicles` at UO), `nav_census` at
  1.1.

## 18. Aim that moves like a hand (2026-11-11)

A playtest report: bot aim did not look human. The view turned at its
full rate along a straight line and landed exactly on the aimed point,
the error jumped to a new direction every 4 to 8 ticks, the trail
behind a crossing target pointed anywhere, sway and recoil were taken
back in full on the tick, and every shot went at the chest point.

`crates/server/src/bots/aim.rs` (`Hand`, one per target) holds the new
aim; `Bot::engage` drives it. `Bot::human_aim` turns it off
(`Server::test_bot_human_aim`, `bot_match`'s `AIM=`), and section 9's aim
plays instead. VERIFIED (measured, ours, `bot_match DIGEST=1`, mp_harbor
`tdm`, 12 bots, 3000 ticks, seeds 1 and 2): with `AIM=none` the branch
prints master 313e5a35's digests. The brain still only writes the cmd's
angles and buttons, and the trigger still reads the gun's line off the
true chest point (`Bot::trigger`, section 9.2, unchanged).

### 18.1 The rules

Each tick on the target, after the reaction delay:

- **The target as seen.** The hand keeps the last five ticks of the
  target's origin and velocity. It puts him where he was `2 * track_lag`
  ago (interpolated between ticks), led on by `track_lag` along the
  velocity he had then. A target crossing at a steady speed is trailed
  by his speed times `track_lag`, the figure 9.1 added in a random
  direction; at normal skill and 190 units/s, 11.4 units behind him.
  When he turns back, the aim runs on past the turn by the same amount
  before it follows (`a_crossing_target_is_trailed_and_a_reversal_overshot`).
- **The point.** On acquisition the hand draws a point off the aimed one:
  across the line within 0.75 `error_floor` either way, and from 0.75
  `error_floor` down to 1.5 up (normal: 3 units across, 3 down to 6 up).
- **The drift.** Gaussian noise through two one-pole low-passes, each
  with an 8-tick time constant, scaled so its mean length is 1. The
  miss is that times `error * (1 + range / error_range) * moving` (9.1's
  terms, `track_lag` aside), so its mean equals 9.1's fixed length. It
  moves under 0.45 of the error a tick (`the_drift_moves_smoothly`).
  `error_start`, `error_floor` and `error_decay` are unchanged.
- **Sway and recoil.** The view takes back `recoil_share` of the gun's
  offset (`BotView::gun_offset`) as it was `recoil_ticks` ago, not all of
  this tick's (`aim::pulled`).
- **The hand.** The want is the seen point plus the drift less the pull.
  Off it by more than 1.5 degrees the hand flicks: from where it is to a
  landing 4% short of the want on the mean, spread by `flick_spread` of
  the angle along the move and half that across. The flick takes
  `travel / turn_deg` ticks rounded up, on the minimum-jerk curve
  `10x^3 - 15x^4 + 6x^5`: the speed rises and falls, its peak 1.875
  times the mean. Under `turn_deg` it is one tick. After a flick the hand
  flicks again if still past 1.5 degrees; inside that it closes half the
  gap a tick and follows the want's own motion. A view another rule set
  (a retreat's look, a respawn), off the flick's course by 2 degrees,
  drops the flick (`a_flick_is_bell_shaped_lands_off_and_corrects`).

INFERRED: these are the shapes a mouse hand makes (a ballistic flick, a
correction, a slow drift, pursuit behind a moving target, a late partial
pull against recoil). Nothing here is fitted to a human's input. Whether
the result looks human is pending a hand check.

### 18.2 The parameters

`bots/skill.rs`, VERIFIED (the presets):

| Field | 0 easy | 1 normal | 2 hard | 3 expert |
|---|---|---|---|---|
| `flick_spread`, share of the flick's angle | 0.15 | 0.1 | 0.075 | 0.05 |
| `recoil_share` | 0.5 | 0.7 | 0.8 | 0.9 |
| `recoil_ticks` | 4 | 3 | 2 | 2 |
| `track_lag`, s (trail, and the sight's delay at twice it) | 0.09 | 0.06 | 0.045 | 0.03 |
| `turn_deg`, a flick's mean a tick | 10 | 15 | 22 | 30 |

Fixed for every level (`aim.rs`): the 1.5 degree flick threshold, 4%
short, half the gap a tick inside it, the 8-tick drift, the point's
spread in floors.

### 18.3 New against old

`AIM=alt`: the new aim on axis on odd seeds and allies on even; in `dm`
on the odd slots on odd seeds and the even slots on even. `tdm`:
mp_carentan, mp_harbor, mp_depot, mp_brecourt, `BOTS=12`; `dm`: the
first three, `BOTS=10`; 12000 ticks, normal skill. "Margin" is the new
side's kills less the old side's per match, mean and standard error.
Sight to shot and sight to hit are mean / median ms (section 1); "hit"
is the share of acquisitions that drew a hit.

VERIFIED (measured, ours, release `bot_match`, `COD_DIR` at 1.1, branch
`feat/bot-human-aim` 352cd72d, 2026-11-11):

| Run | Aim | Kills | Deaths | Accuracy | Sight to shot | Sight to hit | Hit | TTK mean ms | Margin |
|---|---|---|---|---|---|---|---|---|---|
| `tdm` seeds 1-8 (32 matches) | new | 2945 | 2921 | 35.3% | 913 / 700 | 1388 / 950 | 34.4% | 1792 | +0.8 +- 3.5 |
| | old | 2921 | 2945 | 32.2% | 955 / 750 | 1459 / 1050 | 33.9% | 1994 | |
| `tdm` seeds 9-16 (32) | new | 2892 | 2912 | 32.8% | 912 / 700 | 1385 / 1000 | 32.9% | 1655 | -0.6 +- 3.9 |
| | old | 2912 | 2892 | 31.9% | 972 / 750 | 1494 / 1050 | 32.6% | 1590 | |
| `dm` seeds 1-8 (24) | new | 3089 | 3158 | 34.8% | 997 / 750 | 1742 / 1150 | 47.0% | 1288 | -2.8 +- 5.7 |
| | old | 3155 | 3087 | 33.3% | 1045 / 750 | 1775 / 1200 | 47.9% | 1362 | |

- VERIFIED (measured, the table): neither aim outscores the other.
  Pooled over the 64 `tdm` matches the margin is +0.1 +- 2.6 kills (5837
  to 5833); `dm` sits inside one standard error. The error floors and
  decay needed no retune.
- VERIFIED (measured, the table, and the runs' shot counts): the new aim
  hits 1 to 3 points more of its rounds and fires 1 to 11% fewer of
  them (`tdm` 22212 to 24633 and 22884 to 23120, `dm` 22268 to 25089);
  its first round goes out 40 to 60 ms sooner on the mean, its first hit
  30 to 110 ms sooner.
  INFERRED: the drift sits near the point for several ticks at a time
  where the old error kept its full length, so the trigger finds the
  cone sooner and a burst stays on longer.

### 18.4 Skill levels

Section 3's mixed `dm`: eight bots, two per level (`GAMETYPE=dm
SKILLS=0,1,2,3,0,1,2,3`), mp_carentan and mp_depot, seeds 1..=8 (16
matches, none ended). VERIFIED (measured, ours, same build; old is
`AIM=none`):

| Level | K/D old | K/D new | Accuracy old / new | Sight to shot old / new | Sight to hit old / new | Hit old / new |
|---|---|---|---|---|---|---|
| 0 | 0.52 | 0.60 | 27.3 / 31.0% | 2065 / 1797 | 2757 / 2450 | 27.9 / 33.0% |
| 1 | 0.85 | 0.93 | 31.3 / 33.5% | 1064 / 1110 | 1886 / 1989 | 42.8 / 44.9% |
| 2 | 1.26 | 1.18 | 33.4 / 34.7% | 957 / 921 | 1685 / 1657 | 52.1 / 52.6% |
| 3 | 1.52 | 1.37 | 38.6 / 38.2% | 789 / 814 | 1707 / 1562 | 55.1 / 55.4% |

- VERIFIED (measured, the table): easy < normal < hard < expert holds on
  K/D, accuracy, sight to shot, sight to hit and the share hit. The spread narrows:
  easy gains (accuracy +3.7 points), expert gives some back.
  INFERRED: the drift's spells near the point help a large error most.

### 18.5 Cuts on the way

All VERIFIED (measured, ours, as 18.3 and 18.4):

| Build | `tdm` margin, seeds 1-8 / 9-16 | `dm` margin | K/D by level 0-3 |
|---|---|---|---|
| sight a fixed 150 ms late, led by 150 ms less `track_lag`; drift 1 root mean square; point fixed (3 across, 3 down to 6 up); every flick two ticks or more | +2.9 +- 2.8 (score) / -4.6 +- 4.4 | +1.9 +- 5.6 | 0.62, 0.90, 1.31, 1.25 |
| sight `2 * track_lag` late, led by `track_lag`; drift mean length 1 | +3.8 +- 3.5 / | | 0.61, 0.96, 1.27, 1.24 |
| point in floors | | | 0.59, 0.95, 1.29, 1.26 |
| one-tick flicks under `turn_deg` (shipped) | +0.8 +- 3.5 / -0.6 +- 3.9 | -2.8 +- 5.7 | 0.60, 0.93, 1.18, 1.37 |

- INFERRED, from the formula: the fixed 150 ms sight put the largest
  error at a strafe's turn on the highest level, since its lead was the
  longest: up to 0.27 s of the target's speed for expert, where 9.1's
  trail was 0.03 s.
- With a two-tick floor on every flick, expert fell level with hard
  three times running. One-tick flicks under `turn_deg` put its sight to
  shot back at 814 ms (old 789) and its K/D over hard's. INFERRED: a
  flick the old aim made in one tick cost every level a tick, and the
  expert's fights are the shortest, so it lost the most.

The UO, 1.5 and CoD2 bots take the same aim. On 1.1 the gun's offset
is the sight's sway and its turn lag; the recoil pull matters most on
1.5, UO and CoD2, whose gun kick lands in it (combat doc 15.7). Not
measured there.
