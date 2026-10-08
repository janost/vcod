# Bot navigation

How vcod's debug bots (`--bots`) find their way around a stock map. The code
is `crates/server/src/nav.rs` (graph, A*, path follower) and
`crates/server/src/bots.rs` (the brain's goal and steering).

## 1. Stock MP maps carry no path nodes

VERIFIED (asset census, `cargo run -p vcod-common --example node_census`):
none of the twelve stock MP maps (`mp_brecourt`, `mp_carentan`, `mp_chateau`
in `pak4.pk3`; `mp_dawnville`, `mp_depot`, `mp_harbor`, `mp_hurtgen`,
`mp_pavlov`, `mp_powcamp`, `mp_railyard`, `mp_rocket`, `mp_ship` in
`pak5.pk3`) has a single `node_*` entity in its ents lump. The SP maps in
`pak4.pk3` do: `pavlov` has 793 `node_pathnode` plus cover, concealment and
negotiation nodes, `pathfinder` 877, `pegasusday` 1754. The SP geometry is not
the MP geometry, so none of it can seed an MP graph.

VERIFIED (same census): the spawn classes an MP map does carry are
`mp_deathmatch_spawn`, `mp_teamdeathmatch_spawn`,
`mp_searchanddestroy_spawn_{allied,axis}`, `mp_retrieval_spawn_{allied,axis}`
and one `info_player_start`, 112 to 193 points per map. No `info_player_*`
class other than `info_player_start` appears. These points are what the graph
is flooded from.

## 2. The graph

A node is a feet origin where pmove came to rest; an edge `a -> b` exists only
where a body run from `a` through the shared cmd step
(`vcod_common::pmove::cmd::{chop, player_step}`, one 50 ms usercmd per tick,
standing, no weapon) ends on the ground within 8 units of `b`'s column centre.
Capsule size, step-up, slopes, clip brushes and ladders therefore come from the
same code a player moves with. Edges are directed: a drop of up to 200 units
(under `bg_fallDamageMinHeight`, 256) is an edge with no way back.

### Spacing

The lattice pitch is chosen per map: the spawn points' bounding box cut into
about 20 000 cells, rounded up to a multiple of 16 and clamped to 32..48. The
small maps get 32; `mp_brecourt`, `mp_dawnville`, `mp_hurtgen`, `mp_powcamp`
and `mp_rocket` get 48. VERIFIED (measured, 2026-10-07): at 64, where its box
put it before, `mp_hurtgen`'s Retrieval bunker, the objective's room at
z -198, came out as islands of 6 and 10 nodes its stairs never joined, and
the objective was out of every bot's reach. At 48 it joins the main
component, the graph has 27 406 nodes against 15 365, and the build takes
about 1.5 times as long. 32 is the finest worth having: with a player half
width of 15, a lattice line has to pass within a few units of a door's centre,
and the capsule's slide round a frame's edge covers most of the rest. At 48 and
64 on the small maps the measured spawn connectivity drops (`mp_carentan` 177
and 169 of 185 spawns in one component, against 185 at 32; VERIFIED,
measured).

### The flood

- Breadth-first from every spawn point, dropped to the floor by idle cmds and
  walked onto its own column's centre.
- Cardinal walks first. A level edge (ends within 1 unit in height) already
  walked one way is taken as two-way without walking it back. Diagonals run
  two layers later, once the square's other corners exist: a level square
  whose four sides all run both ways is taken as open across, anything else
  is walked.
- A walk starts at run speed (a following bot never stops at a node), re-aims
  at the target every tick, and gives up after two ticks of moving under 2
  units or at twice the run time plus four ticks.
- A walk still in the air when its budget runs out gets up to 16 ticks more
  to land (a 200-unit drop takes 14), and, landed past its budget, the run
  back to the target from there. VERIFIED (measured, `mp_ship`): the stair
  from the deck at z 168 down to the hull at z 104 by x 3430-3472, y 288
  rises about 48 units in 17, so a run down it leaves the top step and
  falls past the rest; the walk was in the air at z 88 when its 9-tick
  budget ran out, and lands at z 80 on tick 12. Without the extra ticks
  the hull's 126 nodes at z 56-200 and the spawns on them had 2 edges out
  and none in.
- A forward walk pinned on the ground (the two stalled ticks that end it)
  jumps once where a body box raised by the jump's 39 units has room ahead
  (`jump_clear`), and the edge it proves is a jump edge (section "Jumps").
- On a ladder the body looks 45 degrees up, where `ladder_move`'s climb
  saturates, and the budget stretches to 300 ticks. VERIFIED (measured, the
  `mp_ship` hold ladder): a full-rate climb makes 53 units/s, so the 560
  units of that ladder take about 210 ticks and the old 200-tick budget ran
  out short of its top. A forward walk that fell
  and never arrived is retried backing along the line facing the way it came,
  which is how a body grabs a ladder below a ledge.
- A node is refused when it lies under a sheet of ground: terrain and patches
  are surfaces, and a gap at a sheet's edge let the first flood of
  `mp_hurtgen` walk the map's floor at z -447.875 under the whole terrain
  (VERIFIED, point traces from those nodes).
  From there the ray up stops on the underside of a triangle whose
  `(b - a) x (c - a)` faces up, or passes through a one-sided terrain
  triangle that the ray back down then hits. Only terrain (`Prim::Tri`,
  which since the patch grid is terrain alone) counts on the way back down. VERIFIED (measured, point traces): `mp_ship`'s mast ladder foot
  stands 540 units under a spar's top facet (a patch), and a low doorway on
  `mp_pavlov` put a brush ceiling 3.75 units over the head, which the ray
  back down starts against. Counting either refused the node, and the
  doorway kept 540 nodes behind it off the graph. Both rays start a step
  (18 units) over the feet, not at the head: VERIFIED (measured, point
  traces, 2026-10-08) at (389, 575, -46.7) on `mp_carentan`, where a
  flood seeded west of the boundary wall reached the town ("Build times"),
  the terrain sheet is triangle 2968 at z -9, 38 units over the feet, and the rays
  from 72 units up missed it; from 18 the ray back down hits it. The
  stock graphs are unchanged by it but for one node on `mp_rocket` (12 333
  nodes, 90 852 edges); the spawn census is the same.
- No node stands where a body would touch the brushes of a `trigger_hurt`
  or a `trigger_multiple` named `minefield` (`World::hazards`), so no edge
  starts or ends in one. VERIFIED (measured, `mp_hurtgen` `re`, 2 bots,
  seed 7): the axis spawn sits against minefields, and a bot that wandered
  off it died to `MOD_EXPLOSIVE` about every 12 s, which ended each round as
  an allied win. The rule takes 3 900 nodes off `mp_brecourt`, 3 400 off
  `mp_rocket` and 10 400 off `mp_pavlov` (VERIFIED, measured; spawn
  connectivity is unchanged or better on every map). INFERRED: the count
  is the minefields plus ground the flood reached only through them.
- After the flood and the ladders, every one-way edge between two nodes on
  one floor is walked back node to node, and where that walk does not
  arrive, again aimed 8 units to either side of the node. VERIFIED
  (measured, `mp_rocket`, master 2000b76's pmove): the walk west from
  (11858, 4224, 262) pins on a corner at x 11840 aimed at the node at
  (11803, 4226) and arrives aimed 8 units north; without it the corridor
  to the spawn at (12406, 4209) was 54 nodes bots entered and never left. A flood walk aims at a column's
  centre, so a node off its centre is walked out of and never into. VERIFIED
  (measured): the ten axis spawns on `mp_hurtgen` stand against a wall at
  x 6304, whose columns' centres lie 32 units inside it, and each was a
  component of one node until this pass. With it `mp_hurtgen` reaches 188
  of 193 spawns in one component (178 before), `mp_brecourt` 161 (159),
  `mp_dawnville` 176 (174) and `mp_rocket` 141 (136), measured before the
patch grid moved `mp_brecourt` back to 159.
- New nodes stay within 512 units of the spawns' bounding box. Past it lies
  scenery: `mp_hurtgen`'s forest added 10 000 nodes.
- Walks run on every core, a layer at a time, handed out one node at a time;
  the merge is serial in job order, so the graph does not depend on the thread
  count.
- A walk that has been on a ladder keeps its budget while it hangs in the air
  at the top before tipping onto the deck, and a climb's budget carries the
  run still left past the top. Without either, a climb to a node more than a
  column past the face ran out at the lip.

### Ladders

The flood's neighbour walks meet few ladders square on: a climb ends a storey
up and up to 70 units past the face, out of the column the walk aimed at. A
pass after the flood (`NavGraph::link_ladders`) handles them on their own.

- The ladders are the world model's `SURF_LADDER` brushes as axial boxes,
  touching boxes merged (a tall ladder is often several brushes). Each box's
  two broad faces, normal along its thinner horizontal axis, are tried.
- The foot is a body dropped 16 units in front of the face's middle, 16
  units above the box's bottom, facing away from it. Facing it, the airborne
  grab hangs the body on the face and it never lands. Where a body does not
  fit there, the drop is tried 8 and 16 units aside along the face, then
  16 units higher, up to 96. VERIFIED (measured, `mp_ship`): one ladder
  brush runs 23 units below the deck it stands on, and a bulkhead crowds
  another's edge so that only a body 8 units aside fits.
- The climb faces the face, holds forward, looks 45 degrees up while on the
  ladder, and once it stands 48 units above the foot walks on 24 units and
  stops, or stops where something at the lip pins it first. Where it comes
  to rest is the head.
- Up is proved by the bots' own run from foot to head and down by their back
  from head to foot (section 3), each having to arrive on the other end's
  floor. A ladder's head often stands right above its foot, so arriving in
  the right column is not enough. The two ends become nodes, linked as
  proved, and each is walked to and from every node within two columns on
  its floor.
- The back down creeps. The body leaves the lip 15 units past the face and
  can be grabbed only while it is under the ladder's top and the face is
  within 17 units (`check_ladder_move`: the probe box shrunk 6 per side, the
  airborne trace 8 long). At a run, 6.6 units a tick, it is past that before
  it has dropped. Inside 24 units of the foot a back down sends move key 15
  instead of 127, about 16 units/s, so two ticks of travel stay under 2
  units, and it holds the heading the creep began with (`nav::Creep`):
  re-aiming as the body passes over the foot's column turns it round in the
  air, away from the face its grab traces toward.
- VERIFIED (measured, the `mp_ship` ladders at x 3354 and 3793): the back
  down missed the grab at a full run and, without the held heading, at every
  creep speed tried; with it, move keys 15 to 30 caught both.
- The flood creeps too, but only as a third try, after a forward walk and a
  back down at a run have both fallen. VERIFIED (measured, `mp_ship`, each
  build paired with one of the old code on the same loaded machine): with
  every flood back down creeping, the build took 4.6 times the old one's;
  as a third try, 1.7 times. Without the creep in the flood, 4 spawns on
  the decks above z 760 stayed apart.
- The creep is tried only from a node within 96 units across of a ladder
  box that spans its floor down to a drop below it (`nav::ladder_near`).
  Away from a ladder it proved edges no bot follows: a bot backs toward a
  waypoint only more than 64 units below or from a ladder (section 3), and
  the creep found, VERIFIED (measured, `mp_depot`), 20-unit steps down a
  sloped roof at z 384 that a forward run and a back down at a run fell
  off. Without them `mp_depot` loses the 1 600 nodes of its roofs at z 192
  to 300 and no spawn; across the twelve maps the creep was 5 to 25% of a
  build's pmove ticks.
- After the ladder pass the flood goes on from the ladders' ends, so a floor
  reached only by a ladder is flooded like any other. Before, a rung's ends
  were walked to and from the nodes within two columns and no further.
  VERIFIED (measured): with it all 112 of `mp_ship`'s spawns are in one
  component, and `mp_depot` keeps 650 nodes of its upper floors that it
  reached before only by creeping down off its roofs.
- VERIFIED (measured, 2026-10-07): every one of `mp_ship`'s 32 ladder boxes
  gets a rung from one face. Before the budget, foot and `under_ground`
  changes above, six got none: the hold ladder (box x 4320-4344, z 56-615),
  the mast (x 3666-3684, z 902-1145), the crow's nest (x 4975, z 701-1219),
  the two whose foot did not fit (x 6475 and x 3600, y -131) and one at
  x 4229 that a later collision change had already fixed.

### Jumps

`PM_CheckJump` lifts the feet 39 units (vz `sqrt(2 * 39 * 800)` = 249.8,
`vcod_common::pmove::JUMP_HEIGHT`; docs/research/cod11-mantle.md, "Jumps"),
over the 18-unit step. Two kinds of edge use it, kept in
`NavGraph::jumps`, and a bot on one sends the jump key on the walk's cue
(`nav::jump_cue`, `nav::jump_key`):

- A pinned jump: the flood's forward walk above, stalled at a ledge's face
  or a low wall. A bot on the edge jumps on the ground while it closes on
  the waypoint under 2 units a tick (stopped at the face or sliding along
  it), or stands within 16 units flat under a waypoint more than a step
  above. VERIFIED (measured, `mp_depot` `re`, seed 7): from the stair
  landing at z 84 to the step at z 116 by (-1759, -603) the bot came in at
  an angle, slid along the step's face and ran off the stair's side at
  every try on the pinned cue alone; the cue under the waypoint takes it up.
- A leap (`NavGraph::link_leaps`, `NavGraph::leaps`): after the flood,
  ladders and walk-back passes, every node 2 columns off whose floor is
  between a step and a jump higher, that 3 edges do not already reach, is
  run at from a standstill with a jump on the tick the body would lose the
  ground (`nav::lip_ahead`: no floor within a step under where its velocity
  takes it). VERIFIED (measured, `mp_depot`): the Retrieval documents lie
  on a crate top at z 148.125 (x -1964 to -1796, y -576 to -464), across a
  gap from a step at z 116.125 (y -616 to -624) that a run off falls into.
  The flood walks one column, so it never tried the step to the crate. A
  leap from the step at (-1820, -609) lands on the crate; a forward walk
  falls.
- A bot comes to rest on a leap's foot first: the follower holds the foot
  as its waypoint until the body is within 12 units of it flat, on the
  ground, under 2 units a tick, with the jump off its 500 ms cooldown
  (`nav::ready_to_leap`), and the bot slows into it. Then the follower runs
  the leap's walk again from where the body stands, and takes the edge out
  of its plans for 200 ticks when that walk does not arrive. VERIFIED
  (measured, `mp_depot`, seed 7): a bot on the move turned onto the leap at
  the lip with its pace along the step and fell into the gap, one that
  jumped onto the step 5 ticks earlier was refused the jump by the
  cooldown, and one resting 4 units nearer the lip than the node left the
  ground on the first tick of the run, before it had the pace to cross.
- A jump edge's top counts as reached only within 16 units of its height,
  where any other waypoint passes within 48: from the foot of a 32-unit
  ledge the top is in reach flat.
- With both, `mp_depot` `re` (2 bots, seed 7) picks the documents up at
  tick 814 and delivers them 224 ticks later (`tests/bot_objectives.rs`).

### Build times

VERIFIED (measured) with `cargo run --release -p vcod-server --example nav_build` on an
8-core, 16-thread Ryzen 7 5850U laptop. Last column: spawn points in the
strongly connected component holding the most of them.

Before the ladder pass (2026-10-06):

| map | spacing | nodes | edges | ms | spawns in one component |
|---|---|---|---|---|---|
| mp_brecourt | 48 | 19089 | 146089 | 1640 | 159 / 161 |
| mp_carentan | 32 | 9636 | 67600 | 724 | 185 / 185 |
| mp_chateau | 32 | 6497 | 43156 | 908 | 110 / 113 |
| mp_dawnville | 48 | 7245 | 49859 | 1085 | 174 / 185 |
| mp_depot | 32 | 9900 | 66003 | 1427 | 152 / 161 |
| mp_harbor | 32 | 7510 | 52726 | 648 | 156 / 161 |
| mp_hurtgen | 64 | 20561 | 152773 | 2662 | 178 / 193 |
| mp_pavlov | 32 | 25953 | 190118 | 2284 | 161 / 161 |
| mp_powcamp | 48 | 5951 | 40775 | 1046 | 149 / 161 |
| mp_railyard | 32 | 11288 | 79510 | 1331 | 157 / 161 |
| mp_rocket | 48 | 18290 | 135954 | 2071 | 136 / 153 |
| mp_ship | 32 | 9373 | 62377 | 1497 | 87 / 112 |

With the ladder pass (VERIFIED, measured with the same example, each map's
build paired with one of the code before it). The timings came off a machine
other builds were loading, so only their ratio means anything: new over old
ran 0.7 to 1.5 on ten maps, 1.6 on `mp_depot` and 1.7 to 2.0 on `mp_ship`.
The counts below were taken again (2026-10-07) after the BVH rework, the
move of world collision to brushes and patches only, and the ladder budget,
foot and `under_ground` changes, the hazards, the walk back over one-way
edges and the 48-unit pitch cap (section 2, "Spacing", "The flood" and
"Ladders"), then once more after patches moved to retail's facet grid
(`patch.rs`). That move cost `mp_brecourt` two spawns (161 to 159) and gave
`mp_ship` two (105 to 107); neither was looked into.

| map | nodes | edges | spawns in one component |
|---|---|---|---|
| mp_brecourt | 15214 | 116099 | 159 / 161 |
| mp_carentan | 9667 | 68211 | 185 / 185 |
| mp_chateau | 6509 | 43677 | 110 / 113 |
| mp_dawnville | 7280 | 50345 | 176 / 185 |
| mp_depot | 12147 | 83997 | 153 / 161 |
| mp_harbor | 7518 | 53344 | 156 / 161 |
| mp_hurtgen | 27406 | 206844 | 190 / 193 |
| mp_pavlov | 16084 | 114248 | 161 / 161 |
| mp_powcamp | 5978 | 41249 | 149 / 161 |
| mp_railyard | 11348 | 80908 | 159 / 161 |
| mp_rocket | 12331 | 90444 | 141 / 153 |
| mp_ship | 11169 | 75091 | 107 / 112 |

With jumps, leaps, the fall budget, the creep only near ladders, the
flood from the ladders' ends and the sidestep on the walk back (2026-10-07;
section 2, "The flood" and "Jumps"). VERIFIED (measured, same example).

- From here on the census looks each spawn up from where a body dropped at
  it lands (`NavGraph::spawns_in_one_component`), as a bot spawned there
  stands. From the spawn's own origin, often 100 units up, `nearest`
  weighs height four times and can pick a node on a crate beside it.
  VERIFIED (measured, `mp_powcamp`): five spawns at z 104 by (1600-1728,
  4344-4624) land on the floor at z 0.125, whose nodes are in the main
  component, but their origins' nearest nodes were on a platform at
  z 64-94 the jumps added, an island bots leave and never enter. By the
  old lookup `mp_powcamp` read 147 against 149 and `mp_rocket` 140 against
  141; neither was a lost connection on `mp_powcamp`.
- `tests/nav_census.rs` builds all twelve and fails on a map under its
  count in the table. In the test profile it takes about 3 minutes and
  2300 s of CPU.
- The "master" column is the same census on master at 2000b76 (merged into
  this branch), which already reads `mp_rocket` lower: its pmove changes pin
  the walk back at (11840, 4226) that `SIDESTEP` gets past (section 2,
  "The flood").
- Wall times: this branch's binary interleaved with the one before it
  three times under a load average of 10-17 from other builds (the
  quietest the machine got), each map's best of three. They are noisy: the
  same binary's CPU time over the twelve maps ran 102 to 215 s across the
  runs. The pmove ticks the build simulated (counted with a temporary
  counter in `Sim::tick`, not committed) do not depend on load: 6.10
  million before, 6.39 million after (+4.7%), measured before the sidestep,
  which walks only the walk-backs that fail.

| map | nodes | edges | spawns in one component | master | ms before / after | pmove ticks before / after |
|---|---|---|---|---|---|---|
| mp_brecourt | 15237 | 116985 | 161 / 161 | 160 | 1272 / 813 | 725437 / 717483 |
| mp_carentan | 10197 | 72530 | 185 / 185 | 185 | 391 / 452 | 237899 / 294297 |
| mp_chateau | 7148 | 50070 | 112 / 113 | 110 | 336 / 385 | 181627 / 233376 |
| mp_dawnville | 7420 | 51805 | 178 / 185 | 176 | 549 / 547 | 348611 / 373589 |
| mp_depot | 11353 | 79231 | 154 / 161 | 153 | 733 / 652 | 519414 / 435802 |
| mp_harbor | 7715 | 54768 | 160 / 161 | 158 | 247 / 264 | 159361 / 166015 |
| mp_hurtgen | 27395 | 207433 | 190 / 193 | 190 | 2035 / 1638 | 1621316 / 1517702 |
| mp_pavlov | 16256 | 117105 | 161 / 161 | 161 | 979 / 811 | 636434 / 612428 |
| mp_powcamp | 6196 | 42913 | 157 / 161 | 154 | 293 / 321 | 176545 / 212222 |
| mp_railyard | 12037 | 86563 | 161 / 161 | 159 | 576 / 585 | 312426 / 372831 |
| mp_rocket | 12332 | 90843 | 148 / 153 | 145 | 947 / 804 | 721977 / 676427 |
| mp_ship | 14065 | 95533 | 112 / 112 | 107 | 903 / 1347 | 461897 / 775829 |

`mp_ship` costs most: its hull below the deck, reached now down the stair
the fall budget lets a walk land on, is 2 900 more nodes.

- VERIFIED (measured): an early single-threaded build of `mp_carentan`, before
  the diagonal shortcut and the stall cutoff, took 10.6 s. `perf` puts 80% of
  the build in `CollisionWorld::trace_node`.
- VERIFIED (measured): a debug build (the test profile) of `mp_carentan` takes
  about 7 s.
- VERIFIED (measured, same example, the table's builds against the ones
  after the collision BVH moved to a binned surface-area split with the
  static models in a subtree of their own; the two binaries interleaved,
  twice, on the same laptop under a load average of 70-80 from other
  builds): the builds of all twelve maps took 184 s of CPU against 288 s,
  map loads included, and each map's best wall time came down 1.2x
  (`mp_ship`) to 2.0x (`mp_railyard`). `perf` had put 77% of the old build in
  `trace_node`'s own box tests: a build's trace entered 111 BVH nodes on
  average on `mp_rocket` and 284 on `mp_depot`, against 58 and 85 after. Spawn
  connectivity is unchanged on every map; edge counts move by at most 14.
  INFERRED: the moved edges are walks whose trace contacts tie, which the
  new walk order resolves differently.
- The server builds the graph on the first tick with bots, not at map load,
  and keeps every graph for the life of the process (`nav::graph_for`), so a
  map cycle that comes back to a map, and the test gates that start one server
  per test, build it once.
- The binary builds it on a thread of its own (`nav::NavJob`) over a copy
  of the collision taken on that tick, 5-40 ms on the tick thread (VERIFIED,
  measured under the same load), so the server keeps its 20 Hz through a
  build of seconds and the bots wander until the graph lands. A
  `vcod --net-probe` on a `mp_rocket` server with two bots read 20 snapshots
  a second through the whole build (VERIFIED, measured). Tests leave
  `Server::build_nav_in_background` off and wait for the graph on that tick,
  so a run does not depend on how fast the build was.
- A background build's walks leave one core to the tick (`nav::par_map`
  under `SPARE_CORE`). `vcod-server --trace` logs a `tick:` line a second:
  mean and slowest `Server::tick` and the worst lag of a tick's start behind
  its slot. VERIFIED (measured, release, 4 bots, 16 threads, load average
  17-31 from other builds): through `mp_ship`'s 2.8 s build and
  `mp_carentan`'s 1.3 s one the slowest tick was 9-11 ms and the worst lag
  under 8 ms against the 50 ms frame, the same range as after the build. The
  tick that takes the finished graph and plans every bot's first path ran
  33 ms once on `mp_ship`. Built on the tick thread, the same builds would
  have stalled it for their whole length.
- VERIFIED (the off-component spawns' heights against the map's
  `SURF_LADDER` brushes): `mp_ship`'s gap was its upper decks, which connect
  by ladders, and the ladder pass closed most of it (section 2, "Ladders").
  VERIFIED (measured): its 7 spawns still apart sat in the hull at z 64 to
  408 (5 after the patch grid), in islands that reach the main component
  neither way, though every ladder has a rung. Walking node to node
  between neighbouring columns on one floor with no edge either way (a gap
  pass after the ladders) added 235 edges on `mp_ship` and joined none of
  them. VERIFIED (measured, 2026-10-07, point traces and walks): the five
  stood in two places. Three were on the hull's deck at z 56-200 (126
  nodes, the spawn at (3534, 61, 64) among them), which a walk left up the
  stair at x 3430-3472, y 288 but never entered: a run down that stair
  falls past its steps and its budget ran out in the air (section 2, "The
  flood"). Two were in a lifeboat at x 3968-4250, y -380 to -225, floor
  z 394-412, walled by a gunwale whose top stands about 40 units over the
  floor, with the deck outside at z 341-351: no walk got over the gunwale
  without a jump. With the fall budget, jumps and the flood from the
  ladders' ends all 112 spawns are in one component; a bot gets into the
  lifeboat by jumping along the deckhouse roof at z 641-664 from the hold
  ladder's head and dropping in from z 648.
- VERIFIED (measured, point traces, 2026-10-07): `mp_carentan` has no
  playable ground at x -1180, y 1900-3300, z -47.875. That floor is the
  map's base brush west of a concrete wall (brush 3164,
  `textures/normandy/walls/concrete@dirtywhite_wallwthrd`, solid at z 2000)
  that runs diagonally from (-1100, 1650) to (-260, 3430), with the town on
  its east side; a flood seeded there reaches the town only under the
  terrain, through (389, 575, -46.7), 37 units under the terrain triangle
  at z -9.27. `under_ground` missed it while its rays started 72 units over
  the feet, above the sheet; since 2026-10-08 they start 18 over ("The
  flood"). The fight-spot test's listener stood there because
  `Server::test_ground_under` traces down outside the wall too; the graph
  is right to leave it out. The other maps' gaps were not investigated.

## 3. Following

The brain stays pure. Each tick the server asks `Bot::goal(&BotView)` for a
`Goal`: `Hold` (dead, spectating, mid-throw), `To(point)` (the enemy it sees,
else a spot it remembers or heard, section 4), `Roam`, or `Away(threat)`
(the hunted side of `bel`, `bot-objectives.md` section 5). A per-bot `nav::Follower` turns that into `BotView::waypoint`:

- A* (Euclidean cost and heuristic) from the node nearest the bot to the node
  nearest the goal. The bots of a tick share a budget of 4000 expanded
  nodes (`BOT_PLAN_BUDGET`). A search is resumable (`nav::Search`): each
  tick a follower's search runs at most 2000 of what is left
  (`PLAN_SLICE`), the bot wanders while it is unfinished, and the search
  resumes next tick where it stopped. A plan searches round the avoided
  edges only when there are some, then without them if that fails.
  VERIFIED (measured, `mp_ship`, 8 bots, background build, release, load
  average 30-40 from other builds): the tick the graph lands ran two
  plans, 2.0 and 1.3 ms, against 13.5 and 11.2 ms when one plan could be
  three full searches.
- Before any search, the component table rules out a goal the start does
  not reach: per strongly connected component, a bitset of the components
  it reaches over the condensation's edges (`NavGraph::reaches`). The stock
  graphs have 1 to 13 components (VERIFIED, measured 2026-10-08: 1 on
  `mp_carentan` and `mp_brecourt`, 3 on `mp_hurtgen`, 13 on `mp_ship`), so
  the table is a few words. VERIFIED (measured, release, loaded machine,
  2026-10-08): on `mp_hurtgen` a search from the main component toward a
  14-node island expanded all 27 394 nodes it reaches in 5.5 ms, one tick
  over a tenth of the frame; with the table the follower's worst tick
  toward the same island was 0.8 ms, and its path came on the second
  tick. With 6 shooting bots on `mp_brecourt` `bel` for 90 s, the bots'
  waypoint pass never took more than 1.6 ms in a tick.
- A `To` point is re-planned only once it moves 128 units off the planned
  destination; at the end of the path the bot heads at the point itself.
  A point the graph does not reach is planned for as far as it goes: the
  path ends at the reachable node nearest it (`NavGraph::path_toward`; the
  component table picks the candidates, so finding it costs no search),
  and the bot heads at the point from there. Heading at the point off the graph
  can drop the body a floor; 48 units below where the path ended, the
  point is planned for again. VERIFIED (measured, `mp_depot` `re`, 2 bots,
  seed 7): before, the attacker climbed to the documents' floor, walked
  off its edge at them, and spent the rest of the round walking at them
  from the floor below.
- A waypoint is passed within 24 units horizontally, or once the bot is nearer
  the next node than the waypoint is, either only within 48 units of its
  height: a ladder's head stands right above its foot. A waypoint four grid
  steps away means the bot left the path: plan again. Forty ticks without
  closing on a waypoint, height counted with the flat distance so a climb
  closes in, drop the path, and the edge the bot was on stays out of its
  plans for 200 ticks (10 s); when no path goes round it, the plan takes it
  anyway. Stuck before the first node, the bot is left 20 ticks to its own
  unstick.
- VERIFIED (measured, `mp_rocket` `sd`, 2 bots, shoot off, seed 1): an
  attacker guarding the planted bomb stood at the foot of the stairs down to
  it, on the graph's edge, and the defender behind him gave up and planned
  the same edge again for about 11 s; the defuse came 1110 ticks after the
  plant. Planning round the edge takes the diagonal past him after one 2 s
  stall, and the defuse comes 886 ticks after the plant.

The brain runs at the waypoint, looks 45 degrees up when it is more than 48
units above or the body is on a ladder (`BotView::on_ladder`, `ps.on_ladder`),
and backs toward it facing away when it is more than 64 below or below at all
while on a ladder (a ladder or ledge below, the way the graph proved it),
creeping inside 24 units with its heading held, as the ladder pass's walks
did. A bot that
has not left a 15-unit circle in ten ticks takes a random heading for 15 ticks
whether it has a waypoint or not. A random heading is never one with a
hazard 64 units along it (`BotView::hazard_ahead`, one flag per 45-degree
octant), and a bot wandering up to one picks again. Engaging an enemy overrides all of it.
In S&D the objective names the point and can hold the bot still
(`bot-objectives.md`); a bot standing at its objective or linked by the
script is never counted as stuck.

## 4. Memory and hearing

Two goals sit between a visible enemy and `Roam`, both kept by the brain
(`Bot::remember`, `Bot::recall_goal`) from what `BotView` carries.

- Memory. Every tick an enemy is visible, its chest point pushed
  `MEMORY_LEAD_S` (0.5 s) along its velocity (`EnemyView::velocity`) is
  remembered. Out of sight the spot stays a goal for `Skill::memory_ticks`
  (100, 5 s) and is dropped once the bot stands within 64 units of it
  horizontally and 96 vertically. The memory outlives the target's identity
  (`forget_ticks`, 1 s): an enemy back in sight after that is a new target
  with a full reaction delay, as before.
- Pre-aim. While a remembered spot within `SHOOT_RANGE` is a goal and the
  waypoint is level, the view turns toward the spot at `turn_deg` per tick
  and the move keys are rotated so the body keeps the path. Ladder and ledge
  waypoints keep the path's own view.
- Hearing. The server records a noise for every `EV_FIRE_WEAPON` shot in the
  per-cmd attack pass (grenade throws and melee excluded), for every blast
  the missile pass sets off, chest high at the shooter or the blast, with the
  client it belongs to, for every round a manned turret fires, at its muzzle,
  the gunner's, and for every script `radiusDamage` (the S&D bomb), chest
  high above its origin, nobody's. `step_bots` takes the list at the top of the next
  tick and hands each bot the loudest one it did not make (`bots::loudest`:
  the smallest distance over range, inside range). No line of sight is
  needed. Teammates' fire counts: a friend shooting means an enemy near him.
- A heard noise is a goal for `Skill::noise_ticks` (200, 10 s, about 2200
  units at run speed) and is replaced by any newer one, but never replaces the
  spot of a seen enemy.

Ranges: `HEAR_GUNFIRE` 2000 and `HEAR_BLAST` 1500 units. VERIFIED
(`soundaliases/iw_sound.csv` in `pak1.pk3`): every rifle, pistol and gun
`weap_*_fire` alias has `dist_max` 7800 (one `airfield` variant of the MP40
has 3000), the grenade throws 350, and `grenade_explode_*` 6000. 7800 covers
most of a stock map and would pull every bot to every fight, so the bot
ranges are a choice that keeps roughly the aliases' 1.3 ratio, not retail data.
