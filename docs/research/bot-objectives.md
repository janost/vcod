# Bot objectives (S&D, retrieval)

How vcod's debug bots (`--bots`) play stock Search & Destroy and Retrieval
(section 4). The brain is
`crates/server/src/bots.rs` (`ObjTarget`, `think_objective`); the server fills
`BotView::sd` and `BotView::linked` in `crates/server/src/server.rs`
(`bot_sd`, `site_stand`) from `ScriptRuntime::sd_objectives`
(`crates/server/src/game/script.rs`). Navigation is in `bot-navigation.md`.

## 1. What stock `sd.gsc` asks of a player

Line numbers are `maps/MP/gametypes/sd.gsc` in `pak5.pk3`.

- VERIFIED (`sd.gsc` `bombzones`, 1797-1814): the sites are the two
  `trigger_multiple` entities with targetname `bombzone_A` and `bombzone_B`;
  `level.planttime` is 5 and `level.defusetime` 10 (seconds). The defuse
  trigger is the entity with targetname `bombtrigger`, sent 10000 units down
  by `_utility::triggerOff` (`maps/MP/_utility.gsc` 134-141).
- VERIFIED (map scripts, `maps/MP/mp_*.gsc`): each map sets
  `game["attackers"]` and `game["defenders"]`. `mp_pavlov` has axis attack,
  `mp_carentan` allies. VERIFIED (grep of `sd.gsc`): the gametype only
  mentions the two assignments in its header comment (51-52) and never
  writes either key, so the sides hold for the whole map.
- INFERRED (`bombzone_think`, 1816-1935, branch conditions): a plant starts
  when a player whose `pers["team"]` is the attackers' touches a zone, is on
  the ground and holds use, while the other zone has no `.planting`. The
  script then `linkTo`s the planter to the zone and counts 0.05 s per frame
  while use stays down; a release or a death before 5 s aborts and unlinks.
- INFERRED (same function, the success arm): a finished plant deletes both
  zones, spawns `level.bombmodel` at `getPlant()` (the planter's feet), moves
  `bombtrigger` to the model's origin and sets `level.bombplanted = true`.
  `bomb_countdown` (1956) runs the 60 s fuse; the explosion deletes
  `bombtrigger` (1956-1990).
- INFERRED (`bomb_think`, 1993-2085, branch conditions): a defuse starts when
  a defender on the ground has `isLookingAt(bombtrigger)`, stands under 64
  units from its origin and holds use. It links the defuser the same way and
  needs use held 10 s; success deletes `bombtrigger`. The progress loop does
  not test `isLookingAt` again, so only the start needs the aim.
- VERIFIED (measured on ours, `mp_rocket`): the moved defuse trigger's bounds
  are centred 8 units above its origin, which is the point the bot aims at.

## 2. What the server tells the brain

- `linked`: `sim.link_to` is set, which is the plant's and defuse's
  `linkTo`. A linked body cannot move, so the brain does not count it stuck.
- `sd`, only while the level's `g_gametype` is `sd`, read fresh every frame
  because a `map_restart` respawns the zones under new handles:
  - `role`: the bot's `sessionteam` against `game["attackers"]` /
    `game["defenders"]`;
  - `sites`: each zone still standing, its absolute bounds and a standing
    point: the nav graph node inside the bounds (16 units in from the sides)
    nearest the middle, from the largest strongly connected component, so a
    bot anywhere on the main graph can path to it. Cached per bounds for the
    life of the graph;
  - `bomb`: once `level.bombplanted` and while `bombtrigger` exists, its
    origin and the middle of its bounds. `None` again after a defuse or the
    explosion.

## 3. The brain

Goal order: a plant or defuse in progress (use held, or linked), a visible
enemy, objective travel, then the rest. Use is pressed only at the
objective, since its rising edge also picks up weapons and mounts turrets.

- Sites are dealt out by rank: a bot's place among its team's bots by slot
  (`SdView::rank`), so a team's bots go A, B, A, ... on both sides.
- Attacker before the plant: its site, moved on by one for every failed
  try in a life. Inside the zone (8 units in) it stops, looks 60 degrees up
  and holds use. Held 20 ticks with no link, or 6 s in all, counts as
  failed: it lets go for 10 ticks and goes to the other site.
- The upward look is for the press's rising edge, which also takes the item
  or turret `G_GetActivateEnt` picks (`cod11-items.md` 2.1). INFERRED (from
  `activate_score`'s 128-unit reach and 0.76 cone, about 40 degrees): an
  item lying on the floor within reach sits more than 30 degrees below
  level, so a view 60 up leaves it outside the cone, and a weapon dropped in
  the zone is no longer swapped for the planter's.
- Attacker after the plant: stands 96 to 250 units from the bomb. A body on
  the bomb would block a defender's `isLookingAt`, and the planter starts
  on it.
- Defender before the plant: goes to its site's standing point and holds
  within 250 units of it, looking about.
- Defender after the plant: only the lead, its team's playing bot nearest
  the bomb (lower slot on a tie, `SdView::lead`), goes for the defuse. The
  rest stand 96 to 250 units off the bomb like the attackers: defenders
  crowding the bomb stood in each other's line from eye to trigger. A dead
  lead hands the defuse to the next nearest on the next frame.
- The lead walks to the bomb, stops within 48 units, turns onto the aim
  point at the normal turn rate, and presses use once the view has sat
  within 1 degree for two ticks (the aim trace is read a frame late,
  `cod11-gsc-object-model.md` 23.1). It holds 10.5 s, or lets go after 20
  ticks with no link and tries again.

VERIFIED (measured, `crates/server/tests/bot_objectives.rs`, 2 bots, shoot
off, seed 7, `mp_carentan`): the attacker plants at tick 733 of the run,
counting the match-start restart; the defender defuses 225 ticks later.
Sweeps over seeds 1, 2 and 7 with shoot off, 4 bots on `mp_carentan` and
`mp_rocket` and 6 on `mp_harbor` and `mp_dawnville`, all planted and
defused, the defuse 221 to 234 ticks after the plant. With 6 shooting bots
on `mp_carentan`, seed 1 planted and defused; seeds 2 and 7 planted and
the round ended with every defender dead. `mp_chateau` and `mp_ship` carry
no `bombzone`, `bombtrigger` or S&D spawn entity (VERIFIED, ents lump
strings), so stock S&D does not run there.

## 4. Retrieval (`re`)

### 4.1 What stock `re.gsc` asks of a player

Line numbers are `maps/MP/gametypes/re.gsc` in `pak5.pk3`.

- VERIFIED (`retrieval`, 1878-1886, and `retrieval_spawn_objective`,
  1922-1979): the objectives are the `script_model` entities with
  targetname `retrieval_objective`, gathered into
  `level.retrieval_objective`. Each one's `trigger` field is the
  `trigger_use` it targets, moved onto one of its `mp_retrieval_objective`
  spots, and its `goal` field the `trigger_multiple` it targets.
- VERIFIED (`retrieval_think`, 1981-2042): the pickup is the `trigger_use`'s
  `trigger` notify, which the use key raises through the aim pick
  (`cod11-gametypes-re-bel.md` 4.2). A player of `game["re_attackers"]`
  takes it, and `other.hasobj[self.objnum]` is set to the objective.
- VERIFIED (`hold_objective`, 2044-2073): a taken objective's trigger goes
  10000 units down (`triggerOff`, 2573-2576) and `re_pickup` is logged.
- INFERRED (`objective_carrier_atgoal_wait`, 2075, branch conditions): the
  delivery is the goal's `trigger` notify with the carrier as `other`; any
  other player's touch is let through and ignored.
- INFERRED (`holduse`, 2434-2534, branch conditions): a carrier holding use
  0.3 s is linked in place, and 2 s drops the objective.
- VERIFIED (measured on ours, `mp_dawnville`): the two goal triggers there
  are 500 by 368 units. A defender standing in one fired it every frame it
  was armed, before the carrier's touch (slot order) could, and the carrier
  stood in the goal for 85 s without delivering. INFERRED (Q3 lineage
  `multi_trigger`): a `trigger_multiple` waiting out its `wait` ignores
  every touch, so the same holds on retail.

### 4.2 The brain

`BotView::re`, filled on an `re` level from `ScriptRuntime::re_objectives`:
the bot's role, each objective's pickup trigger middle while it lies there,
its goal's standing point (`site_stand`, as for a bombzone) with the
distance at which a body is clear of the goal trigger, and who carries it.

- Attacker carrying an objective: walks to its goal and never presses use.
- Attacker otherwise: walks to the nearest objective lying there, stops
  within 48 units, turns onto the trigger's middle, and once the view has
  sat within 1 degree for two ticks taps use for two ticks (under
  `holduse`'s 0.3 s). VERIFIED (measured, `mp_dawnville`): from the side
  the graph led to, the eye's trace to the trigger's middle met the world,
  so each tap that took nothing moves the next try 40 units round the
  objective, eight sides in turn.
- Attacker with every objective carried, and defender with one carried:
  a ring round its goal from just clear of the trigger to 150 units past.
- Defender with nothing carried: guards an objective by rank, within 250
  units of its trigger.

VERIFIED (measured, `crates/server/tests/bot_objectives.rs`, 2 bots, shoot
off, seed 7, `mp_carentan`): the attacker picks up at tick 483 and delivers
377 ticks later. The same run on each stock map for 150 s: 10 of 12 pick
up and deliver. On `mp_depot` the attacker never reaches the objective,
which lies on an upper floor at z 148; on `mp_hurtgen` every round ends in
an allied win within 18 s and nothing is picked up.

VERIFIED (measured, 2026-10-07, same run): `mp_hurtgen` now picks up at
tick 1108 and delivers at tick 2275. Its rounds ended because the lone axis
bot wandered into the minefields round its spawn (`MOD_EXPLOSIVE`, killer
`world`), and it wandered because the objective's bunker was not on the
graph (bot-navigation.md, "Spacing", "The flood" and section 3).
`mp_depot` still picked nothing up then. Its documents lie on a crate top
at z 148, and the nearest the graph got was a step 32 units below at
(-1852, -608, 116), across a gap the flood never jumped. VERIFIED (measured,
2026-10-07, `tests/bot_objectives.rs`, same run): with leaps
(bot-navigation.md, "Jumps") the attacker comes to rest on the step, leaps
onto the crate, picks up at tick 814 and delivers 224 ticks later.

With 6 shooting bots, `mp_carentan` delivered and `mp_harbor` picked up
twice in 150 s and delivered neither.
