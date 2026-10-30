# Bot measurements: CoD2 and CoD4

`bot_match` runs moved out of docs/bots.md, which keeps only current behaviour. Each table names its seeds and arms; the harness and what each number means are in [bot-combat.md](bot-combat.md) section 1. Newer CoD2 and CoD4 runs are in [bot-objectives.md](bot-objectives.md) sections 11 and 12 and [bot-combat.md](bot-combat.md) sections 14 to 17.

## 1. CoD2: cover, lean, retreat and smoke (2026-10-22)

Before and after, the
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

## 2. CoD2: first runs (2026-10-22)

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

## 3. CoD2: objectives, frags and mantles

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

## 4. CoD2: weapons and skill (2026-10-23)

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

## 5. CoD4: mounted guns and throw backs (2026-10-26)

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

## 6. CoD4: cars in the sight test (2026-10-26, 2026-10-27)

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

## 7. CoD4: rotation soak (2026-10-25)

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
