# Bot objectives (S&D)

How vcod's debug bots (`--bots`) play stock Search & Destroy. The brain is
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

- Attacker before the plant: one site per life (`pick % sites`, the pick
  redrawn from the seeded generator at each death). Inside the zone (8
  units in) it stops and holds use. Held 20 ticks with no link, or 6 s in
  all, counts as failed: it lets go for 10 ticks and goes to the other site.
- Attacker after the plant: stands 96 to 250 units from the bomb. A body on
  the bomb would block a defender's `isLookingAt`, and the planter starts
  on it.
- Defender before the plant: goes to its site's standing point and holds
  within 250 units of it, looking about.
- Defender after the plant: walks to the bomb, stops within 48 units, turns
  onto the aim point at the normal turn rate, and presses use once the view
  has sat within 1 degree for two ticks (the aim trace is read a frame late,
  `cod11-gsc-object-model.md` 23.1). It holds 10.5 s, or lets go after 20
  ticks with no link and tries again.

VERIFIED (measured, `crates/server/tests/bot_objectives.rs`, 2 bots, shoot
off, seed 7, `mp_carentan`): the attacker plants at tick 654 of the run,
counting the match-start restart; the defender defuses 222 ticks later.
Sweeps over seeds 1-6 on `mp_carentan`, `mp_harbor`, `mp_dawnville` and
`mp_rocket`, and with 4 and 6 bots on the first two, all planted and
defused; the slowest defuse (1109 ticks, `mp_rocket`) was the nav follower
stalled at a drop on the way. `mp_chateau` and `mp_ship` carry no
`bombzone`, `bombtrigger` or S&D spawn entity (VERIFIED, ents lump strings),
so stock S&D does not run there.
