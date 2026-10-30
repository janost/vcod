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
same code a player moves with. Edges are directed: a drop is an edge with no
way back, up to a fall of 250 units from the highest the body rose (under
`bg_fallDamageMinHeight`, 256, section "Other rules").

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
  to land (18 since 2026-10-17; a 250-unit fall takes 16), and, landed
  past its budget, the run
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
- A walk node to node (the walk back, the ladders' links and the leaps)
  arrives only on the target node's floor: within 8 units of its height
  as well as of its spot. The bound was `Z_MERGE`, 40, until 2026-10-09.
  VERIFIED (measured, `mp_ship`): the hull has a diagonal beam, 32 units
  over the floor, from about (2152, 384) to (2232, 520), with nodes on its
  top at z -31.875. Its sides slope down to an edge 16 under the top, and a
  body stands on that edge at z -47.875 (on the brush's axial bevel). The
  walk back from that edge (2208.5, 444.7) to the top node
  (2172.9, 418.2) falls straight on. Aimed 8 units aside, it came to rest
  on the edge at (2183.3, 407.6, -47.87), 16 under the node, and that
  proved the edge. Bots followed it to that spot and stalled there; it is
  the one the 2026-10-08 follow-up reported. The 8-unit bound takes 4
  (`mp_brecourt`) to 38 (`mp_ship`) edges off each stock graph. Node counts
  and the spawn census are unchanged (VERIFIED, measured with the
  `nav_build` example).
- A flood walk that comes to rest in a column whose node stands 8 to 40
  units off its height (inside `Z_MERGE`, the node it merges into) proves
  nothing about that node: the walk is run again node to node, onto the
  node's floor (all three gaits, as the flood's own walk). Where that
  fails too, the rest becomes a node of its own and floods on, and a
  column lookup takes its nearest node in height. Until 2026-10-09 such a
  rest was merged into the column's node and linked. VERIFIED (measured,
  2026-10-09, each case walked with a temporary trace of `walk_as`):
  - `mp_ship`: the walk from the hull floor (2272, 513, -63.875) toward
    the column of the beam-top node (2236.4, 512.0, -31.875) came to rest
    on the beam's edge at -47.875 (the beam is in "A walk node to node"
    below), and the edge it made sent bots onto the slope (seed 1, 16
    bot-ticks).
  - `mp_chateau`: a floor at z 260.125 by (-129, 1520), between nodes at
    236 and 291 in the same columns, never had a node; arrivals on it
    were merged into both. Its own nodes keep the spawn at (-55, 1179)
    in the main component (112 of 113, as before).
  - `mp_depot`: the run from (-28.5, 352.0, 12.125) off a ledge to the
    node at (-64.2, 352.0, 27.125) lands on a step's corner at 3.125,
    right under it, and was merged into it; the spawns at (-125, 494) had
    no other way in. The node-to-node walk jumps there ("Jumps").
  The stock graphs gain 0.1 to 0.9% nodes and 0.2 to 1.0% edges, every
  census count is unchanged (`mp_ship` 14 065 to 14 147 nodes, 95 495 to
  96 389 edges), and a build runs 1.2% (`mp_hurtgen`) to 4.7%
  (`mp_depot`) more instructions (VERIFIED, `nav_build` example, `perf
  stat -e instructions:u`).
- A rest within a unit of a column's node's height but more than a unit
  aside of it, at the foot of a drop deeper than a jump, proves nothing
  about that node either (2026-10-13): the walk is run again node to node,
  and where that fails there is no edge (the rest is not made a node of
  its own). The flood walks at a column's centre, and a node is where the
  first walk to arrive came to rest, up to 8 units off it; on level ground
  that does not matter, at the foot of a drop onto a narrow ledge it
  does. VERIFIED (measured, `mp_ship`, scratch walks): the deck node
  (3647.9, -316.9, 618.5) had an edge to the ledge node
  (3685.5, -354.3, 400.1) over the hold, proved by a back down toward the
  column's centre (3680, -352) that landed; no gait from the one node to
  the other lands, and the bots that took the edge fell past the ledge to
  the hold floor at 92 (119 of master's 249 deaths at x 3700, y -300).
  Of `mp_ship`'s 5 038 drop edges deeper than a jump, 76 had no gait that
  lands node to node, 12 after (the rest are made by passes that walk
  node to node already). The spawn census does not move (`nav_build`).
- A drop deeper than a jump whose walk, in the first gait that lands it
  from its node, falls from 4 units aside of the node is risky
  (`NavGraph::mark_risky`, 2026-10-13), and costs a plan 4 000 units more
  (`RISK_COST`): a bot takes it only where there is no other way.
  VERIFIED (measured, `mp_ship`): the back down from (3647.0, -352.9,
  611.1) to the ledge at 400 lands from the node and from 4 units north,
  east and west, and falls from 4 units south; bots held on the node and
  sent on from rest still fell past the ledge on some seeds (seed 231
  bot 4, below). `mp_ship` has 286 such edges of its 4 955 drops; away
  from ladders `mp_ship` 214 of 4 087, `mp_depot` 174 of 3 320 and
  `mp_hurtgen` 88 of 2 421. Removing those 214 instead of pricing them
  cost `mp_ship` 4 spawns from its main component (112 to 108); with the
  ladders' drops left out of the pricing, bots rerouted onto the drop
  off the mast's platform at (3564, -127, 896), which no walk from aside
  misses, and its next edge below (next bullet in section 3).
  A drop no gait lands node to node at all is risky too (2026-10-13c):
  the build's walk that made it ran from somewhere else. VERIFIED
  (measured, `mp_ship`, scratch walks): the edge from the deck node
  (3515.2, -323.1, 630.6) to the ledge node (3487.9, -357.8, 580.1) falls
  in all three gaits, and every bot that took it ran past the ledge to
  the hold floor at 56 (7 of 19 deaths in seeds 1-767 odd once the mast
  platform's run was fixed, below; `nav::tests::a_drop_no_gait_lands_node_to_node_is_risky`).
  So is a drop deeper than a fall that kills (480,
  `bg_fallDamageMaxHeight`) from a node more than a half-width (15) off
  every ladder (2026-10-13f): it lands only on a grab of the rungs on the
  way down. VERIFIED (measured, the graphs): `mp_ship` has 6 drop edges
  past 480 and the other stock maps none. Four are marked: the two from
  the crow's nest exits (4963.9, 35.5 and 92.5, 1219.1) back off the rim
  into the hatch for the deck node (4929, 68, 692), 26 off the ladder box
  (`nav::tests::a_drop_that_kills_but_for_the_rungs_is_risky`), the mast
  top's (3301, 128, 1030) to the deck at 480, 251 off any ladder, and the
  back down from (4319, -133, 600) to the hold at 56, 55 off. The crow's
  nest head's own way down the rungs (11 off) is not, and the hold
  ladder's (4332, -87, 615), 9 off, was risky already. Effect in section
  3, 2026-10-13f.
- A walk down within a jump (more than 1 unit down, off a jump edge)
  is risky when a point 32 or 48 units from its lower node, in any of
  the eight octants, has no floor within 256 (`bg_fallDamageMinHeight`)
  under it, and the forward walk from 4 units aside of its upper node
  falls (2026-10-15f, `nav::beside_a_fall_that_hurts`,
  `nav::tests::a_walk_down_beside_a_deadly_fall_that_misses_from_aside_is_risky`).
  VERIFIED (measured, the `mp_ship` graph): 280 such edges; without the
  floor test, 297 of the edges down more than a step would be (most in
  the hull at 27 to 56, where a slip lands on the floor at -64). The
  motivating edge is the roof south of the mast platform,
  (3514, -283, 623) to the ridge node (3496, -318, 611), 12 down. Effect
  in section 3, 2026-10-15f.
- The largest strongly connected component with the risky edges left out
  is the graph's safe part (`NavGraph::safe`, 2026-10-16): a roam from
  inside it never ends outside it (section 3). VERIFIED (measured, the
  `mp_ship` graph): 443 of the main component's 13 954 nodes lie outside.
- Nodes with no floor under their middle within 480 are perches
  (`NavGraph::perches`, 2026-10-13f): a roam never picks one (section 3).
  VERIFIED (measured, the graph, point traces down from each node):
  `mp_ship` 74 of 14 143 (the crow's nest rails at 1254, its head over the
  hatch, the yards at 1136 to 1166, the mast top at 1030, platform edges
  at 648 and 701), `mp_depot` 9, `mp_pavlov` 7, none on `mp_hurtgen`,
  `mp_carentan`, `mp_harbor`, `mp_brecourt`, `mp_chateau`, `mp_dawnville`,
  `mp_powcamp`, `mp_railyard` or `mp_rocket`. A point goal (an enemy, an
  item, a remembered spot, an objective point) on a perch is walked to the
  nearest node off the perches within 128 of it instead, and the walked-out
  path heads at that node, not the point (`NavGraph::goal_node`,
  2026-10-18; `nav::tests::a_point_goal_on_a_perch_ends_beside_it`).
  VERIFIED (the graph): both crow's nest rail nodes the test names have such
  a node within 128.
- New nodes stay within 512 units of the spawns' bounding box. Past it lies
  scenery: `mp_hurtgen`'s forest added 10 000 nodes. The box also takes in
  every `script_gameobjectname bas` entity (`World::nav_cover`), which
  leaves the pitch alone. VERIFIED (UO's ents lumps): mp_italy is the one
  stock bas map with such entities past the spawns' box, and its western
  axis bunker's stair and charge room (base at -4556 3760, the box's west
  edge at -4646) had no node (bot-objectives.md 7.2).
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

- The ladders are the `SURF_LADDER` brushes of the world model and of
  every submodel linked where it spawned (`CollisionWorld::model_in_place`)
  as axial boxes, touching boxes merged (a tall ladder is often several
  brushes). Each box's two broad faces, normal along its thinner
  horizontal axis, are tried.
- VERIFIED (the BSPs, UO install): every UO `bas` map keeps its bunkers'
  ladders in the base's own `script_brushmodel`, two a base, and no other
  stock map has a ladder outside model 0: mp_foy `*15`, `*60`, `*63`,
  `*66`, `*69`, `*72`; mp_italy `*2` to `*7`; mp_kursk `*73` to `*98` by
  fives; mp_ponyri `*27` to `*42` by threes; mp_rhinevalley `*28` to
  `*33`. mp_italy's allied base `*4` (targetname `ps5`,
  `script_idnumber`, 73 `clip_nosight_rock` brushes, one `clip_metal`,
  two `common/ladder` with contents 0x28010000) is the whole shell of the
  bunker round (6682, -2624, -714): its five spawns there (58, 62, 63, 64
  and 68) stand in a room it closes on every side, at a prone body's
  height too, and its ladders at x 6720 to 6764, y -2742 to -2730 and x
  6546 to 6588, y -2814 to -2804 (z -716 up to -582 and -546) are the
  way out. `bas.gsc`'s breach (`BaseAssault_base_think`) hides, unlinks
  and deletes every target of the base with a `script_idnumber`, so the
  ladders go with the walls. VERIFIED (measured, release `nav_build`
  `UO=1`, before and after the submodel ladders): mp_italy's spawns that
  reach the main component go 361 to 371 of 371 (the five above and
  245, 246, 247, 303, 304 round (2780, -4360, -72), the western allied
  base `*3`'s), in one component 361 both; mp_foy, mp_kursk, mp_ponyri
  and mp_rhinevalley move by -44 to +504 nodes and keep their counts (324 /
  381, 261 / 282, 219 / 272, 344 / 386). 1.1 has none, so its graphs
  are unchanged. (`nav::tests::italys_allied_bunker_ladders_are_its_bases`.)
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
  floor. The back down creeps from a running start or, since 2026-10-13,
  from rest (section 3, the mast). A ladder's head often stands right above its foot, so arriving in
  the right column is not enough. The two ends become nodes, linked as
  proved, and each is walked to and from every node within two columns on
  its floor.
- The back down creeps. The body leaves the lip 15 units past the face and
  can be grabbed only while it is under the ladder's top (8 under since
  2026-10-12j, section 3) and the face is within 17 units (`check_ladder_move`: the probe box shrunk 6 per side, the
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
- A head can be perched on the ladder's own top: the body stands on the
  ladder brush with no other ground under its middle (`nav::perched`: the
  standing box's ground is a `SURF_LADDER` surface and a point trace a
  step down from the middle finds none other). Such a head links only to
  its foot and to its exits: the first spot 24, 32 or 48 units along the
  face, either way, that a walk from the head reaches with ground under
  its middle (`nav::ladder_exits`), linked back where the walk back
  arrives. It is not flooded from or walked to from the floor around it,
  and the flood takes no node where a walk comes to rest perched.
  VERIFIED (measured, 2026-10-09, `mp_ship`, point traces and walks): the
  mast hatch's plate (x 3711-3712, y 43-69, top 992) stands in a hole in
  the platform at x 3676-3711, y 37-77, against a wall at x 3720; the
  climb's head is (3704.9, 56, 992.1) on the plate's 1-unit top, and the
  flood made a second node at (3705, 62) where a walk toward the column
  centre (3712, 64) pinned on the wall within 8 units. Both crossed the
  hole to the platform by walks that arrived or fell by a few units. The
  exits are (3704.9, 27.5) and (3704.9, 84.5), on the floor south and
  north of the hole, both linked back. The crow's nest head (4963.9, 64,
  1219.1) gets exits at y 35.5 and 92.5. The mast's top (3675, -101,
  1145.1) is perched with no floor along the face and keeps its old
  links (the head of 2026-10-12j's retail ladder is elsewhere, section 3); the hold ladder's head (4332, -87.1, 615.1) stands on a lip, not
  the ladder, and is not perched.
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
`NavGraph::jumps`, and a bot on one sends the jump key where the walk
that proved it jumped (`nav::PinJump`, `nav::leap_cue`, `nav::jump_key`):

- A pinned jump: the flood's forward walk above, stalled at a ledge's face
  or a low wall. A bot on the edge runs the walk's own rule, tick by
  tick: it jumps on the ground after two ticks moved under 2 units with
  its key held at the waypoint, out of a bob (below), or standing within
  16 units flat under a waypoint more than a step above, each only where
  `jump_clear` passes from where it stands (2026-10-20, "The jump cue and
  the re-proof" below). VERIFIED (measured, `mp_depot` `re`, seed 7): from the stair
  landing at z 84 to the step at z 116 by (-1759, -603) the bot came in at
  an angle, slid along the step's face and ran off the stair's side at
  every try on the pinned cue alone; the cue under the waypoint takes it up.
- A bobbing jump: the flood's forward run (`nav::walk_to`) pinned under a
  face too steep to stand on, where the step lifts the body onto the face
  and it slides back, so it is off the ground on most ticks and the
  pinned jump above never fires. A run that stalls sliding jumps on its
  next tick on the ground instead (`nav::BOB`). Only the flood's runs and
  the jump edges' re-proof do, and a bot on a jump edge: the follower's
  walks on from a bot's body never take that jump, so a way proved with
  one is a jump edge or nothing. VERIFIED (measured
  2026-10-17, UO rules, walks): mp_berlin's roof gutter, a trough
  between the roof at z 768 and a steep roof slope south-east of it, with
  a ledge at 633-652 on its south side. The slope node a 149-unit drop off
  the roof lands on, (-673.7, 4131.1, 616.1), runs south into the gutter
  and bobs between z 612 and 630 under the face; a jump from a tick on
  the ground lands on the ledge at (-718.7, 4086.4, 651.6)
  (`nav::tests::a_run_bobbing_under_a_steep_face_jumps_out_of_the_gutter`).
  `jump_clear` allows 2 units over the 39 (`nav::JUMP_SLACK`): that jump
  from z 612.26 stands on the ledge at 652.42, 40.2 up, where a trace to
  exactly 39 is blocked by the ledge's lip.
- A leap (`NavGraph::link_leaps`, `NavGraph::leaps`): after the flood,
  ladders and walk-back passes, every node 2 columns off whose floor is
  between a step and a jump higher, that 3 edges do not already reach, is
  run at from a standstill with a jump on the tick the body would lose the
  ground (`nav::lip_ahead`: the tick's move, run without the jump on a
  copy of the body, leaves the ground; 2026-10-12, below). VERIFIED (measured, `mp_depot`): the Retrieval documents lie
  on a crate top at z 148.125 (x -1964 to -1796, y -576 to -464), across a
  gap from a step at z 116.125 (y -616 to -624) that a run off falls into.
  The flood walks one column, so it never tried the step to the crate. A
  leap from the step at (-1820, -609) lands on the crate; a forward walk
  falls.
- A CoD4 graph keeps no leap (`bots::cod4::LIP`, `NavGraph::build_opts`):
  its bots' route has no pmove probes (`BotWorld::PMOVE_PROBES` off), so
  `BotView::lip` stays false and a bot on a leap never jumps and runs off
  the foot. Each leap there is walked again as a plain jump edge by
  `NavGraph::reprove_jumps` and dropped where that walk falls. CoD2's
  route has no pmove probes either, and its build drops the lip too
  (`bots::cod2::LIP`, bot-objectives.md 11.8). The CoD1, 1.5 and UO
  builds keep their leaps (`build_with` passes the lip), so their graphs
  do not move. INFERRED (bot-objectives.md 12.14).
- A ladder's head is never a leap's foot: a node some edge climbs to more
  than a jump. A bot arrives there off the rungs and comes to rest where
  the climb tips it over, not where the head was measured. VERIFIED
  (measured 2026-10-10, `mp_ship`, walks): the hold ladder's head
  (4332, -87.1, 615.1) stands on the ladder brush's 8-unit lip over the
  shaft, and its leap to the ledge (4322.3, -158.1, 641.1) arrives from
  the head. A climb in seed 149 tipped the bot onto the lip at
  (4333.2, -91.5), and from there the leap falls. From 4 units off the
  head it falls to the south and arrives to the north and west. Of
  `mp_ship`'s 138 leaps this was the only one from a ladder head
  (`nav::tests::a_ladder_head_is_no_leaps_foot`).
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
- A bot waits on a jump edge's foot while its jump cools down
  (`Follower::jump_wait`): `PM_CheckJump` refuses a jump inside
  `JUMP_COOLDOWN_MS` (500) of the last, and the graph's walks set off on
  every edge with one to hand. VERIFIED (measured 2026-10-13d, `mp_depot`
  `dm` seed 9, traced): the crate stack at (-2300 to -2176, 64 to 164)
  is an open crate whose walls top out at 66 round a floor at 23, beside
  a crate at 46 and the floor at 12; nodes on the walls' rims stand 5
  units from nodes inside (graph dump: (-2277, 96, 66) over
  (-2272, 96, 23)). A bot jumped from 12 onto the crate at
  (-2275, 131, 46) and 5 ticks later ran on for the rim at
  (-2277, 96, 66), 20 over it; the jump under the rim was refused and
  the run went off the crate's edge into the open crate, where the rim
  stands 43 over the floor, past any jump. It jumped at the rim for 3 s,
  planned round, came back by the stack's other jump edges the same way
  and hopped there for 12 s. From the foot with a jump to hand the run
  lands on the rim; with 150 ms on the timer it falls in
  (`nav::tests::a_jump_edge_waits_on_its_foot_for_the_jump`). Set off
  from rest on the foot a body moves little on its first tick, so a
  pinned tick counts only with a move key held at the waypoint the tick
  before (`bots::tests::a_bot_set_off_from_rest_is_not_pinned`).
- A body more than 8 units over the foot, under a step, with the top
  still more than a step over it, waits where it stands instead (one on
  the top's own floor beside a top 16 over its foot, as on mp_cassino's
  `ctf` at (5065, -1274, 256), still goes back to the foot) (`nav::tests::a_jump_edge_is_waited_on_from_a_ledge_above_its_foot`).
  VERIFIED (measured, `uo_bots_fight_and_sprint_in_tdm` `SEED=17`, UO
  rules, 2026-10-21): mp_cassino's edge from (3648, -1249, 111.4) to
  (3698, -1200, 128.1) is a jump edge; the graph's walk steps up 17 onto
  a ledge at 128 about 8 units wide, pins on the 164 block beyond, and
  jumps from the ledge. A bot pinned on the step's face jumped there
  instead, landed on the ledge inside the jump penalty, and was sent back
  down to the foot; from the foot, 53 under the block, its jump peaks at
  150 and lands it on the ledge again, penalty and all, for 12 s (ticks
  760 to 999).
- Under 1.5's and UO's jump penalty the wait leaves the body at rest on
  the foot, and an edge the graph's walk proved only with its running
  start can miss from there: mp_cassino's from (4992, -1338, 295) over a
  trench at 256 to (4938, -1393, 303) (bot-objectives.md 10.6). A bot held
  for its jump walks it again from the wait's spot at rest, once per edge,
  and plans round one that misses (`Follower::rest_jump_fails`); one
  already avoided is waited on.
- A body off the graph that no plain walk takes to a node may still jump
  out: `NavGraph::walkable_start` walks a bot's jump from rest to each of
  the nearest few nodes more than a step up, and the plan's first leg is
  then a jump edge (`Follower::jump_start`). mp_cassino's roof dent at
  (4948, -1413, 280), 23 under the roof round it and with no node in it,
  held a bot 12 s before (bot-objectives.md 10.6).
- A level edge (both ends within a step) that is no jump, left off its
  side: a body that lands more than a step under both ends plans again
  round the edge from a node a walk or jump reaches (`Follower::follow`,
  `nav::tests::a_landing_under_a_level_edge_plans_again_round_it`). Every
  stuck replan starts there too. mp_cassino's roof dent took a bot off
  such an edge and held it 4 s (seed 25, bot-objectives.md 10.6).
- A goal that flickers. A point goal that moves past `RETARGET` and comes
  back within `PARK_TICKS` (1 s) takes up the plan it left, node, closest
  approach and idle count included (`Follower`'s `Parked`,
  `nav::tests::a_goal_back_within_a_second_takes_up_its_old_plan`).
  VERIFIED (measured, ours, release, stock UO paks, seed 17 `tdm` on
  `feat/bot-aim-footwork` 6b2ab6d1, a per-tick dump of bot 0's goal and
  follower): on mp_cassino's stair from (1440, -725, 13) to (1440, -669,
  56) the body at (1440, -698, 24) saw an enemy for one or two ticks, so
  `Bot::goal` went from `To(1273, -911, 137)` to `To(1703, -651, 1)` and
  back. Each move dropped the path, and the replan back started at
  `NavGraph::nearest`, the node (1441, -761, 26) behind the body (score 68
  against 68 for (1440, -725, 13) and 160 for the stair's top). The keys
  reversed, the body walked back down to z 8, turned, climbed to the same
  spot and saw the enemy again, ten times over 12 s (ticks 920 to 1128).
  With the parked plan the bot climbs on.
- A pinned walk jumps under a ceiling lower than the jump's 39 units too:
  `jump_clear` looks for room ahead at the height the body rises to, not
  at the full jump's. A floor-bound walk (node to node) also jumps where
  it stands within 16 units flat under its target floor more than a step
  up, the cue a bot on a jump edge jumps on: aimed at a point overhead,
  the run jitters a few units a tick and never counts as pinned.
  VERIFIED (measured, `mp_depot`, 2026-10-09): on the step's corner at
  (-63.9, 352, 3.125) a ceiling stops a jump 2.25 units short of 39, and
  the jump lands on the floor at 27.125 the next tick
  (`nav::tests::a_walk_jumps_onto_a_floor_right_overhead_under_a_ceiling`).
- A jump edge's top counts as reached only within 16 units of its height,
  where any other waypoint passes from up to 48 above or a step below it:
  from the foot of a 32-unit ledge the top is in reach flat. It is never
  cut past (section 3).
- With both, `mp_depot` `re` (2 bots, seed 7) picked the documents up at
  tick 814 and delivered them 224 ticks later (`tests/bot_objectives.rs`).
  VERIFIED (measured 2026-10-11): master at bec4f62d picks them up at
  754 and delivers 260 later; with the follower changes of 2026-10-11
  (section 3) at 983 and 225 later. That run's attacker comes to rest on
  the leap's foot (-1784.1, -609, 116.1) 5.3 units off it at
  (-1779.3, -607.2), runs for the crate and walks off the step's north
  edge on the run's second tick without the lip's jump, twice (ticks 466
  and 874), into the gap at 84; the third run, to (-1856, -576, 148),
  lands.
- The lip is the tick's move, not the pace. VERIFIED (measured
  2026-10-12, the run above with the bot's playerstate and cmd printed
  each tick): the lip cue projected the origin by the velocity alone
  (origin + v * 50 ms) and looked for a floor a step under it. At
  (-1783.1, -603.8) the velocity was (-76, 68), 5.1 units a tick, and the
  floor was there; the tick's move, accelerating, covered 8.9 units, to
  (-1789.7, -597.9), off the step. The bot held a carbine: its first
  tick from rest gained 102 units/s where the walk's body, with no weapon
  (move scale 1), gained 86, so the walk jumped in time and the bot did
  not. `lip_ahead` now runs the tick's move without the jump on a copy of
  the body (with the bot's weapons, toward the waypoint) and reads a lip
  when the copy leaves the ground; the leap walk runs the same tick, keeps
  it when it stays down and runs it again with the jump when it does not
  (`nav::tests::the_lip_reads_on_the_last_tick_on_the_ground`, which fails
  with the projection). Walks the late jump dropped into a gap now land,
  and the builds have more leaps (`examples/nav_build.rs`, master 53bdf043
  then this change): `mp_brecourt` 16 / 21, `mp_carentan` 2 / 13,
  `mp_chateau` 3 / 8, `mp_dawnville` 5 / 7, `mp_depot` 63 / 99,
  `mp_harbor` 2 / 2, `mp_hurtgen` 19 / 448, `mp_pavlov` 27 / 57,
  `mp_powcamp` 3 / 3, `mp_railyard` 14 / 47, `mp_rocket` 16 / 165,
  `mp_ship` 137 / 359. Spawns in one component are unchanged on all
  twelve (`tests/nav_census.rs`). With the follower changes of 2026-10-12
  (section 3), stalled bot-ticks over seeds 1 to 63 odd (`ship_stalls`,
  `dm`, 6 bots, 4000 ticks), master then after: `mp_depot` 7 224 / 2 620,
  `mp_hurtgen` 4 440 / 3 730, `mp_rocket` 0 / 0, `mp_pavlov` 51 / 70.
  Most of `mp_depot`'s, in both builds, are spells around
  (-2400, -1400, 0), not looked into. Seed 7's attacker now
  leaps from (-1779.3, -607.2) to the crate at (-1820, -544, 148) on the
  first try, picks the documents up at tick 483 (983 before) and delivers
  them 272 ticks later.

### Build times

VERIFIED (measured) with `cargo run --release -p vcod-server --example nav_build` on an
8-core laptop. Last column: spawn points in the
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

Cheaper traces (2026-10-08). VERIFIED (measured, same example, the two
binaries interleaved, each map twice, load average 33-41 from other
builds). Instructions are `perf stat -e instructions:u` over the whole
process, map load included, and do not depend on load; ms is the build's
best wall time of the two. Node, edge and spawn counts are the same on
every map: no trace changed its answer.

| map | instructions (G) before / after | CPU s before / after | build ms before / after |
|---|---|---|---|
| mp_brecourt | 96.7 / 44.5 | 21.0 / 10.0 | 4944 / 2521 |
| mp_carentan | 42.9 / 22.0 | 10.5 / 5.2 | 3505 / 1879 |
| mp_chateau | 40.2 / 19.4 | 9.6 / 4.5 | 3411 / 1553 |
| mp_dawnville | 57.0 / 28.0 | 14.3 / 6.5 | 3155 / 1530 |
| mp_depot | 57.9 / 27.2 | 14.8 / 6.2 | 4076 / 2146 |
| mp_harbor | 22.2 / 10.9 | 5.0 / 2.6 | 1621 / 946 |
| mp_hurtgen | 192.7 / 87.1 | 42.8 / 19.5 | 8836 / 3573 |
| mp_pavlov | 82.5 / 42.1 | 22.1 / 9.4 | 3923 / 1731 |
| mp_powcamp | 26.3 / 13.1 | 5.9 / 3.1 | 1457 / 743 |
| mp_railyard | 65.2 / 27.8 | 14.3 / 6.4 | 2490 / 1209 |
| mp_rocket | 87.2 / 41.6 | 19.4 / 9.6 | 3247 / 1770 |
| mp_ship | 108.8 / 50.0 | 23.7 / 11.1 | 4898 / 2270 |

- VERIFIED (`perf record`, a symbolized release build): before, the build
  was `trace_node` 47%, `Terrain::clip_capsule` 35% on `mp_hurtgen`, and
  `trace_node` 31%, `PatchCollide::trace` 26% on `mp_ship`, and the poses'
  `RwLock` took a contended read 3% of `mp_hurtgen`'s samples.
- Four changes, all in `vcod-common`'s collision and each held to the old
  answer by a test (`cod11-mantle.md`, "Terrain is a swept sphere, a patch
  is a facet"): the second pass of a trace replays the leaves the brush
  pass logged instead of walking the tree again (-29% instructions on
  `mp_hurtgen`, -39% on `mp_ship`); a terrain triangle out of the sweep's
  reach skips the clip (`mp_hurtgen` a further -34%); a patch facet whose
  axial bevels keep the sweep out is skipped (`mp_ship` -25%); a world
  with no posed model skips the poses' lock, whose reader count every
  thread wrote on every trace. On `mp_rocket` that last one took the CPU
  time from 15.7 to 10.0 s with 3% fewer instructions.
- Not changed: the flood's layers are still a barrier each. Measured with
  a temporary per-layer timer under a load average of about 57, walks kept
  the 16 threads 66% busy on `mp_ship` (149 layers) and 85-89% on
  `mp_rocket` and `mp_hurtgen` (46-54 layers); under that load the figure
  says little.
- Not changed either: a back down off a ledge away from a ladder. Of the
  1 600 to 8 600 back walks a map runs, about 8% of `mp_hurtgen`'s pmove
  ticks, few arrive, and most of those that do land under 64 units down,
  where a bot runs forward and never backs (section 3). Trying the back
  only near a ladder or after a forward run that fell more than 64 cost
  `mp_ship` 2 spawns and `mp_chateau` 1 (VERIFIED, measured), so it stays.

Perched ladder heads (2026-10-09; section 2, "Ladders"). VERIFIED
(measured, same example, before and after on one machine): spawns in one
component unchanged on all twelve maps (`nav_census` passes at the
floors). Nodes and edges move only where a walk came to rest perched:
`mp_depot` 11451 / 80007 to 11438 / 79752, `mp_powcamp` 6226 / 43140 to
6222 / 43105, `mp_railyard` 12109 / 87257 to 12104 / 87212, `mp_ship`
14147 / 96389 to 14139 / 96255; the other eight are identical. Build
times moved within the load noise (`mp_ship` 3693 to 3320 ms).

No leap from a ladder head (2026-10-10; section 2, "Jumps"). VERIFIED
(measured): `mp_ship` 14139 / 96255 to 14139 / 96254, its leaps 138 to
137; `nav_census` passes at the floors on all twelve maps. I did not
count the other maps' edges.

The leaf walk's cost (2026-10-16). Since the capsule leaf gating
(bsp-ibsp59-format.md, "The leaf walk") a sweep that meets a model-0
patch's box is clipped again in retail's leaf order, which cost
`mp_ship` most: 845 000 of its build's 11.4 million world traces meet a
patch. Two changes, neither moving a trace: each leaf listing carries its
prim's box and one clear of the sweep cut at the fraction is passed over
(it was clipped in full: 14.1 million patch clips on `mp_ship`, 2.0
million after), and the BVH walk after the leaf walk enters only the
subtrees holding a prim outside model 0. VERIFIED (measured, `nav_build`,
release): the graph `digest` (nodes, edges, jumps, leaps, risky edges,
perches) is the same before and after on all twelve 1.1 maps and on UO's
`mp_cassino`, `mp_peaks`, `mp_arnhem` and `mp_uo_carentan`; `nav_census`
and the ignored `bots.rs` tests pass at both installs. Columns: master
before the gating (92c87ada's first parent), master after it (e0e5e9b5),
this change. Instructions as above (`perf_event_open`, whole process,
load-proof); ms is each map's best build of three runs interleaved, load
average 23-25, so only the instructions compare closely. The
gating's own answers moved `mp_carentan`, `mp_dawnville`, `mp_pavlov`,
`mp_powcamp`, `mp_railyard` and `mp_ship` (6 nodes, 36 edges, 2 leaps).

| map | instructions (G) before gating / after / now | build ms before gating / after / now |
|---|---|---|
| mp_brecourt | 54.3 / 54.9 / 55.1 | 269 / 317 / 278 |
| mp_carentan | 32.5 / 39.2 / 34.7 | 187 / 238 / 190 |
| mp_chateau | 29.2 / 30.5 / 29.8 | 156 / 166 / 161 |
| mp_dawnville | 33.6 / 36.7 / 34.6 | 177 / 200 / 181 |
| mp_depot | 68.0 / 74.8 / 68.5 | 438 / 496 / 442 |
| mp_harbor | 16.5 / 16.7 / 16.8 | 128 / 133 / 127 |
| mp_hurtgen | 132.1 / 134.2 / 133.0 | 581 / 650 / 613 |
| mp_pavlov | 65.2 / 72.9 / 66.9 | 330 / 380 / 334 |
| mp_powcamp | 17.6 / 18.6 / 18.0 | 156 / 147 / 154 |
| mp_railyard | 50.3 / 53.7 / 50.3 | 341 / 291 / 273 |
| mp_rocket | 64.6 / 67.3 / 65.9 | 396 / 342 / 327 |
| mp_ship | 105.2 / 142.8 / 110.6 | 698 / 837 / 614 |

UO, after / now (G): `mp_cassino` 52.8 / 52.0, `mp_peaks` 82.8 / 81.3,
`mp_arnhem` 36.1 / 36.1, `mp_uo_carentan` 40.7 / 35.7. Left on
`mp_ship`'s 5.3 G over the build before the gating (VERIFIED, measured
with temporary cycle counters, not committed): the BVH pass a met sweep
throws away and the leaf walk's own tree descent. Ending that pass at the
first met patch and keeping the walk's stack between walks moved
nothing measurable and were dropped.

The thrown-away pass (2026-10-16, later). VERIFIED (measured, `nav_build`
on `mp_ship`, each phase of a met sweep run a second time on a copy of
the trace and the difference in instructions read): of the 845 000 met
sweeps' cost, the BVH pass is 6.5 G (the brush walk 5.65, the partition
replay 0.85), the leaf walk 10.0 G (its patch clips 4.9, its tree descent
2.9, its listings' box tests 1.1, its brush clips about 0.5) and the walk
after it 1.2 G. The pass is what decides which sweeps take the leaf walk,
and each faster way tried moved answers or cost more:

- VERIFIED (measured): testing model 0's patches in the brush walk, at
  the fraction it has reached there, and ending the walk at the first that meets: 950 000
  sweeps take the leaf walk instead of 845 000, and the digest moves on
  `mp_carentan`, `mp_dawnville` and `mp_uo_carentan`, where the BVH's
  order and the leaves' break a tie apart. It took `mp_ship` to 108.6 G
  but put 1.0 G on `mp_brecourt` and 3.0 G on `mp_hurtgen`.
- VERIFIED (measured): a per-node "holds a model-0 patch" flag on the
  BVH, walked before the pass at the entry fraction: the digest held on
  `mp_ship`, but so many more sweeps took the leaf walk that the build
  ran 127.3 G.
- VERIFIED (measured): a per-node box of every listing under it on the
  leaf walk's tree, to
  pass over subtrees: of 36 million nodes the met walks entered, 0.4
  million were passed over (the boxes span most of the map near the
  root), and the build ran 2.6 G more.
- VERIFIED (measured): `trace_node` without recursion (a stack of its own, the sweep's box kept
  between nodes, each child box-tested before it is pushed): `mp_ship`
  went to 103.1 G but its cycles rose from 44-45 G to 49-50 G (three
  runs each, `cycles:u`), so fewer instructions ran slower.

Kept, neither moving a trace (VERIFIED, measured: the digest is unchanged
on all sixteen maps): the
partition replay stops at the first met patch, whose answer goes anyway,
and the leaf walk passes an axial node the sweep stays on one side of
straight to that child without the stack. Instructions (G), master
(65ef830f) / this change; wall times are left out, the load
average ran 30-50 (VERIFIED, measured):

| map | instructions (G) master / now |
|---|---|
| mp_brecourt | 55.1 / 55.2 |
| mp_carentan | 34.7 / 34.4 |
| mp_chateau | 29.8 / 29.7 |
| mp_dawnville | 34.6 / 34.5 |
| mp_depot | 68.5 / 68.5 |
| mp_harbor | 16.8 / 16.9 |
| mp_hurtgen | 133.0 / 133.2 |
| mp_pavlov | 66.9 / 66.6 |
| mp_powcamp | 18.0 / 18.0 |
| mp_railyard | 50.3 / 50.1 |
| mp_rocket | 65.9 / 65.9 |
| mp_ship | 110.6 / 109.3 |

UO, master / now (G): `mp_cassino` 52.0 / 52.0, `mp_peaks` 81.3 / 81.2,
`mp_arnhem` 36.1 / 36.1, `mp_uo_carentan` 35.7 / 35.4 (VERIFIED,
measured). `mp_ship`'s cycles moved within the noise (VERIFIED, measured, three runs each:
`cycles:u` 43.6-45.2 G against 44.0-44.8 G). It stays about 4 G over the build before the gating.

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
- The copy leaves out the brush models marked `script_noteworthy rubble`
  (`World::rubble`): UO's `bas.gsc` lifts each 1000 units at the start and
  drops it back only over a dead base, and every other gametype deletes
  it. Since 2026-10-15 the cache keys on which submodels clip as well,
  since each gametype deletes the others' brushes and one map's graph
  differs by gametype, and the graph follows the script's brush links:
  once `GameHost::brush_links` (bumped by every `delete`, `notSolid` and
  `solid` that changes a brush model's link) has held still 2 s, a new
  build starts over the collision as it stands and replaces the graph when
  it lands, the bots' paths and zone stands dropped (`Server::relink_nav`;
  bot-objectives.md 7). `bas` deletes a breached base's walls, which is
  the only way to its charge.
- The copy keeps the mover poses, the world copies and the vehicles'
  collmap instances where they stand, so a rebuild walks round a parked
  tank. Since 2026-10-21 the cache keys on those too
  (`CollisionWorld::placements`) and keeps the last 24 graphs. Keyed on
  the links alone, a test got the graph whichever test in the same
  process first reached those links had built round its own vehicles:
  VERIFIED (measured, release, UO mp_foy bas, 4 shooting bots, seed 7),
  the graph in use at ticks 6040 and 7040, after a rebuild at the same
  links, digested `04c80a17fac01c91` where a fresh build over the
  collision then digested `12e1ab16edbe74f5`. The same match alone ran
  identical in three processes side by side at load average 40 and in
  two full `bots.rs --ignored` runs. A graph is now a function of its key
  (`nav::tests::the_cache_key_covers_every_placement`).
- A background build's walks leave one core to the tick (`nav::par_map`
  under `SPARE_CORE`). `vcod-server --trace` logs a `tick:` line a second:
  mean and slowest tick and the worst lag of a tick's start behind
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

### Other rules: 1.5 and United Offensive

Until 2026-10-14 every walk ran a body built by `PlayerState::spawn`,
whose `rules` is 1.1's, so a 1.5 or UO server's bots, which move by the
server's own rules (`MoveRules::for_protocol`), followed a graph proven
under 1.1's. Now:

- Every walk's body comes from `nav::body`, which takes the rules
  `nav::with_rules` set on the thread. `NavGraph::build` takes the rules
  and sets them; `par_map` hands them to its workers; `NavJob::start`
  builds under the server's; `engine::tick` runs the bots' pass
  under them, so the follower's re-proofs (`edge_holds`, `flings`,
  `runs_off`, the crease test) walk the same pmove. The graph cache keys
  on the rules. `NavGraph::rules` records them.
- 1.5's and UO's jump penalty (`cod15-movement.md` 2) cuts a jump inside
  1.8 s of the last landing to as little as 15.6 units, where every jump
  edge was proved with the full 39. A bot on a jump edge, or at a leap's
  foot, waits until it has run out as well as the 500 ms cooldown
  (`nav::jump_ready`, `nav::tests::a_jump_edge_waits_out_the_jump_penalty`).
  INFERRED from the formula; not measured on a bot.
- UO's vehicle maps filled the old 60 000-node cap before the flood
  reached their far ends. VERIFIED (measured, `nav_build`, UO rules):
  with the cap at 400 000, `mp_italy` fills 77 171 nodes and its spawns
  in one component go 305 to 361, `mp_rhinevalley` 145 710 and 107 to
  344, `mp_foy` and `mp_kursk` 62 126 and 65 367, `mp_ponyri` 59 089
  (under the old cap already). The cap is 200 000 now, and A* gives up
  past the graph's node count where that is more than 40 000 (no 1.1
  map comes near either).
- The same eleven pakuo maps built under 1.1's rules and UO's (VERIFIED,
  measured, `nav_build` `UO=1 RULES=1.1` against `UO=1`, the old cap):
  node and edge counts within 0.7%, spawns in one component equal on all
  eleven, leaps more under UO's on ten (`mp_berlin` 140 to 262,
  `mp_rhinevalley` 51 to 248, `mp_ponyri` 38 to 61) and equal on
  `mp_cassino` (10 to 11). INFERRED: 1.5's step and ground snap
  (`cod15-movement.md` 5) land more of the leap walks.

CoD2's graph is walked under 1.5's rules too (`bots::cod2::GRAPH_RULES`),
though its bots move by CoD2's pmove. A game can hand the walks a step of
its own: `nav::WalkStep`, set on the thread by `nav::with_step` beside the
rules, handed to `par_map`'s workers, passed to the build by
`NavJob::start_with` and kept in `NavGraph::step`; the cache keys on it,
and `NavGraph::moving` sets both. A walk carries the step's own state
between cmds in `Sim` (`nav::WalkCarry`). CoD1 sets none, and its walks
step as before (bot-combat.md 16, every CoD1 and UO gate byte-identical).
CoD2's step (`bots::cod2::Cod2Walk`) moved its graphs by under 1% and its
bots stalled more, so it is off (bot-combat.md 16). Until 2026-10-24
CoD2's bots' pass ran outside any `with_rules`, so the follower's walks
for a CoD2 bot (`Follower::rest_jump_fails` and the re-proofs) stepped
under 1.1's rules. `Cod2Server::step_bots` now runs the driver under its
graph's `NavGraph::moving` (`GRAPH_RULES` while the graph builds), and
the body the follower reads (`bots::cod2::cod1_body`) carries CoD2's jump
penalty (`pm_flags` 0x80000 with `pm_time`, 1.5's 0x2000 under the
graph's rules), so `nav::jump_ready` waits it out (bot-combat.md 17).

United Offensive's census. VERIFIED (measured 2026-10-14, release
`nav_build` `UO=1`, COD_DIR at the stock 1.5 `main/` with UO's `uo/`,
the machine loaded by other builds, so ms is rough). The first eleven are
`pakuo00`/`pakuo01`, the last eight the map pack (`uomappack00`). "Reach
it" counts the spawns whose node reaches the component holding the most
spawns (`NavGraph::spawn_census`): a UO base's spawns mostly stand behind a
drop out of it (mp_cassino's allied base at (192, 1072, 8), 36 spawns of
three classes, reaches the town and is never reached back), which a bot
spawned there leaves by and never needs again.

| map | spacing | nodes | edges | ms | spawns in one component | reach it | leaps |
|---|---|---|---|---|---|---|---|
| mp_arnhem | 48 | 7640 | 49915 | 511 | 231 / 332 | 287 | 19 |
| mp_berlin | 48 | 17679 | 123026 | 1833 | 202 / 303 | 303 | 262 |
| mp_cassino | 48 | 8558 | 57393 | 707 | 94 / 180 | 168 | 11 |
| mp_foy | 48 | 62126 | 470101 | 7924 | 317 / 391 | 341 | 178 |
| mp_italy | 48 | 77171 | 590070 | 6130 | 361 / 371 | 361 | 192 |
| mp_kharkov | 48 | 40855 | 297995 | 4649 | 187 / 402 | 402 | 98 |
| mp_kursk | 48 | 65367 | 495439 | 5856 | 258 / 282 | 270 | 95 |
| mp_ponyri | 48 | 59089 | 447994 | 3605 | 219 / 273 | 242 (240) | 61 |
| mp_rhinevalley | 48 | 145710 | 1107889 | 14672 | 344 / 386 | 386 | 737 |
| mp_sicily | 48 | 24301 | 177922 | 2227 | 236 / 241 | 238 | 44 |
| mp_uo_stanjel | 48 | 8762 | 62052 | 724 | 366 / 380 | 378 | 61 |
| mp_peaks | 48 | 8570 | 55656 | 1810 | 113 / 128 | 126 | 170 |
| mp_streets | 32 | 8087 | 53523 | 635 | 180 / 226 | 197 | 23 |
| mp_uo_carentan | 32 | 10227 | 72789 | 755 | 285 / 285 | 285 | 11 |
| mp_uo_dawnville | 48 | 7666 | 52863 | 767 | 189 / 249 | 243 | 5 |
| mp_uo_depot | 32 | 15655 | 109243 | 1638 | 275 / 351 | 351 | 184 |
| mp_uo_harbor | 32 | 7842 | 54432 | 442 | 113 / 292 | 292 | 20 |
| mp_uo_hurtgen | 48 | 27484 | 207848 | 2785 | 281 (282) / 286 | 282 | 522 |
| mp_uo_powcamp | 48 | 6440 | 44020 | 333 | 226 / 258 | 254 | 2 |

- `mp_berlin`'s 303 went to 301 with 1.5's and UO's stored velocity tail
  (`cod15-movement.md` 7) and came back with the bobbing jump ("Jumps"
  above). VERIFIED (measured 2026-10-17, release census): the two spawns
  at (-483, 4415, 743) and (-667, 4414, 743) sit on a roof whose one way
  out is a 149-unit drop from (-624, 4180.6, 768) into the roof gutter.
  Under the old tail the slope node landed at (-675.2, 4134.9, 619.3) and
  a run from it happened to reach the ledge at (-723, 4078, 650); under
  the stored one it lands at (-673.7, 4131.1, 616.1) and every run out
  bobs under the gutter's face, so only the edge west along the gutter
  proved. The bobbing jump proves the way onto the ledge from both nodes.
  The same change joins one more spawn on `mp_uo_hurtgen` (281 to 282 in
  one component, in brackets in the table); 1.1's census and the other
  UO maps are unchanged.
- `mp_ponyri`'s 242 is 240 since the UO vehicles merge (a7b99cd6), and
  the census floor with it. VERIFIED (measured 2026-10-15, release, the
  census's own build without the rubble unlink): 59 089 nodes, 447 994
  edges, 242 reach at a7b99cd6^1; 59 128, 448 203 and 240 at a7b99cd6
  and on master. The merge put UO's vehicle-only clip brushes (contents
  0x200, `CONTENTS_VEHICLE_CLIP`) in the collision world; no player
  trace's mask meets them. Their brush rows alone change nothing (242
  with them listed and their bounds in the submodel spans, but kept out
  of the BVH); their prims in the BVH give 240. So the move is the tree's
  shape: which brush a leaf walk meets first decides a tie at a brush
  seam and the `fraction <= 0` early stop, which order a startsolid
  is found in. INFERRED, from those two runs. The player's world is the
  same either way, and the number is not more or less retail than 242,
  whose order is its own BSP's. `nav::ladders` reads brushes without a
  mask, but filtering it to brushes a player clips still gives 240.

- A UO map's spawn points are its gametypes' together: `mp_uo_spawn_*`,
  `mp_gmi_bas_*`, `mp_gmi_ctf_*` beside 1.1's classes (VERIFIED, the
  entity lumps), all matched by `nav::spawn_points`.
- `tests/nav_census.rs`'s `every_uo_maps_spawns_reach_the_fight` holds
  both columns as floors, on the graph a tdm server builds: the other
  gametypes' script brush models unlinked as `_gameobjects` deletes them.
  With every one linked, mp_peaks' dom and ctf one-way spawn walls (26
  `cliponeway` patches each, owned by `*5` and `*6`) cut its component to
  87; with tdm's unlinks `mp_rhinevalley` reaches 383, the `bas` rubble
  out (VERIFIED, measured 2026-10-16). In the test profile it takes 14 minutes and
  8 000 s of CPU (VERIFIED, measured); it returns at once without a UO
  install, so the 1.1 census costs what it did.
- Walled-in spawns (2026-10-17). `nav_build`'s `LOST=1` lists the spawns
  whose node does not reach the census's main component (`GAMETYPE=tdm`
  builds the census's graph). On master c3d1e90b mp_cassino lost 12,
  mp_streets 29 and mp_uo_dawnville none (its floor of 243 was stale).
  Each was a small cluster of nodes no walk left, and each had one of
  two causes:
  - A fall deeper than the walk allowed. VERIFIED (measured, point
    traces and a traced walk): mp_cassino's allied upper floor
    (146..193, 1008..1296, 208), 6 spawns of `mp_uo_spawn_allies`, sd
    and re, leaves only by a window sill at 240.1 on its west wall
    (x 140..162, y 1047..1257); the walk jumps onto the sill and walks
    off it into the street at 0.1, a 240-unit fall, and the walk gave up
    past 200. VERIFIED (the binaries): `bg_fallDamageMinHeight` is 256
    in 1.1d's `gameCvarTable` and in UO's (`game.mp.uo.i386.so`
    `tools/re/dump_cvars.py` rows 103 and 104, 256 and 480, flags
    0x208), and `PM_CrashLand` measures the fall from where the body's
    rise ends (`pmove::crash_land`, the height out of the landing's
    speed). A walk now gives up at a fall of 250 from the highest point
    since it last stood (`nav::MAX_DROP`), measured on the landing tick
    too, where the old test read only the ticks in the air. The
    follower's walks again from a bot's own pace keep the old test, a
    fall past 200 under where the body last stood read in the air
    (`nav::PACED_DROP`): at 250 the back down onto mp_ship's ledge 211
    under the deck node (3647, -352.9, 611.1) lands at the 190 a bot came
    along the deck with, and a bot that did fell past it (section 3),
    so a bot moving onto a drop that deep is still held to take it from
    rest. Gains
    (`nav_build` `UO=1 GAMETYPE=tdm`, reach it): mp_cassino 162 to 174,
    mp_berlin 299 to 303, mp_kharkov 400 to 402; mp_ship gains about
    400 nodes of drops and keeps its 112.
  - A way out through a gap off every lattice line. The flood aims at
    column centres, and a window whose standing room lies between two
    lattice lines is never walked through. VERIFIED (measured, player
    hull sweeps): mp_streets' allied rooms at (-360..-160,
    -7068..-6788, 8), 14 spawns (`mp_teamdeathmatch_spawn`,
    `mp_deathmatch_spawn`, `mp_uo_spawn_allies_secondary`, sd and re),
    leave only by a window in the east wall at x -160 whose sill stands
    at 36; a standing body fits in it with its feet within 2 units of
    38 and 16 units across, about y -6862, between the lattice lines at
    -6848 and -6880. `NavGraph::link_pockets` now runs every node that
    reaches no node of the largest strongly connected component in 32
    headings, 48 and 80 units out; a rest on a floor no node holds that
    the run aimed at the rest itself also reaches becomes a node and
    floods on, and a rest on another node's floor is walked again node
    to node. It repeats up to three times. Gains (reach it, the census):
    mp_streets 197 to 210, mp_arnhem 287 to 332, mp_foy 383 to 391,
    mp_kursk 270 to 282, mp_rhinevalley 383 to 386, mp_sicily 238 to
    241, mp_uo_stanjel 378 to 380, mp_peaks 126 to 128, mp_uo_hurtgen
    282 to 286, mp_uo_powcamp 254 to 258; in one component mp_arnhem
    253 to 265, mp_foy 379 to 386, mp_kursk 270 to 273, mp_rhinevalley
    359 to 362. The 1.1 census does not move (mp_depot's 158 holds with
    both changes switched off; its floor of 154 was stale).
  `bots.rs`'s `uo_bots_leave_their_walled_in_spawns` places a bot in the
  cassino floor and the streets rooms; each leaves within 30 s.
- Crouched jumps (2026-10-17). Three rooms still held 22 spawns after
  the two fixes above, each walled in by a window whose sill is a jump
  up and whose lintel stops a standing body's rise. VERIFIED (measured,
  player hull sweeps, standing 70 and crouched 50):
  - mp_cassino's axis room (5256..5576, -1708..-1272, 128), 6 spawns
    (`mp_uo_spawn_axis`, sd and re): two windows in its south-west
    wall, sills at 160.
  - mp_streets' axis room (290..570, -6900..-6570, 101), 11 spawns:
    the west sills at 127. One window there, (292, -6660), is closed
    by patch 391 against a body inside, which a sweep from outside
    passes through (not looked into).
  - mp_streets' room at (450..710, -3420..-3078, -24), 5 spawns: its
    south window at (536, -3424), sill 0.
  What retail does at the wall under each window. VERIFIED (retail UO
  1.51b, `client-probes/probe_crouchjump` and `--probe-crouchjump`,
  twelve captures in `crates/server/tests/fixtures/playerstate/
  *-crouchjump-*.txt`): run at the wall, jump, and hold crouch from
  the cmd after the jump's for a second, or don't (`-stand`). At 8 ms
  cmds all six runs get onto the sill and out: the standing body's rise
  stops under the lintel (mp_cassino: z 143.8 at ct 21991, `velocity[2]`
  -13), and the step up has it on the sill at 160.125 by the next snapshot.
  At 50 ms cmds, a bot's frame, the three standing runs fall back to
  the floor (mp_cassino: peak 167.1, back at 128.125) and the three
  crouched ones get out. `moves_ab.rs`'s
  `the_predictor_replays_uo_jumps_through_low_windows` replays all
  twelve under UO's rules with no field off.
  - `Gait::CrouchJump`: the forward walk with its jump's room read at
    the crouched height (`jump_clear`), and crouch, with `upmove` -127
    as a retail client sends it, held from the cmd after the jump's
    (`PM_CheckJump` refuses a crouched body) until the body is on the
    ground again. `NavGraph::link_pockets` tries it on every run of a
    cut-off node that arrives no other way, and on the node-to-node
    walk again; an edge it proves goes in `NavGraph::crouch_jumps`
    (and `jumps`). A crouched rest where a standing body does not fit
    is no node (`stands`): every walk on from a node starts standing.
    The rest on each of these sills holds a standing body.
  - A bot on a crouched jump edge (`BotView::crouch_jump`,
    `Follower::crouch_jumping`) jumps on the jump edges' cue and holds
    crouch the same way; `Follower::edge_holds` walks it again in the
    same gait.
  - Gains (release `nav_build` `UO=1 GAMETYPE=tdm`, reach it): mp_cassino
    174 to 180 (4 crouched jump edges), mp_streets 210 to 226 (53). No
    other UO map and no 1.1 map moves. `bots.rs`'s
    `uo_bots_leave_their_walled_in_spawns` places a bot in each of the
    three rooms; each leaves within 30 s, and none does with the crouch
    taken off the bot.
  - The captures also showed the client's predictor dropping a `pm_time`
    that 1.5's penalty left running without its flag
    (`cod15-movement.md` 2).

The sprint. A UO bot holds `+sprint` (`buttons` 0x8) while following its
path on the ground with no enemy in sight, keys straight ahead, and
`Follower::straight_run` at least 400 units: the path's nodes from the
body on, while each stays within 16 of the line from the body to the
last, rises or falls under a stair's 18 a step, and leaves by no jump,
leap or risky edge (40 nodes at most). It starts only with the fatigue
bar at half or more, holds on while the engine keeps the flag up, and
lets go 160 short of the straight's end. The sprint leaves the weapon's
`moveSpeedScale` out (`coduo-movement.md` 3), so it runs at 304 and the
keys are not cut to the graph's pace; the graph's walks ran at 190, so
the sprint is kept off turns and drops it never proved
(`bots::tests::a_uo_bot_sprints_a_long_straight_leg`,
`nav::tests::the_straight_run_stops_at_a_turn_a_rise_and_a_jump`).

VERIFIED (measured, release `ship_stalls` `UO=1`, `tdm`, 6 bots shooting
off, 2400 ticks, seeds 1 to 31 odd): sprinting bot-ticks, stalled
bot-ticks and falls, with the sprint and without it on the UO graph, and
the 1.1 graph without it (what master did):

| map | sprinting | stalled sprint / no sprint / 1.1 graph | falls |
|---|---|---|---|
| mp_cassino | 26 672 | 19 / 14 / 0 | 0 / 0 / 0 |
| mp_foy | 41 518 | 0 / 0 / 0 | 0 / 0 / 0 |
| mp_italy | 41 391 | 0 / 0 / 0 | 0 / 0 / 0 |
| mp_uo_stanjel | 23 678 | 1 840 / 685 / 0 | 1 / 0 / 1 |

Sprinting is 11 to 22% of the bot-ticks, against the 27% a full bar's 3 s
of sprint and 8 s of refill allow. `mp_uo_stanjel`'s stalls are one spot:
the stair down from (720, 244, 254) to (670, 233, 222), whose edge the
flood proved and a floor-bound walk from the node does not (blocked by a
wall west of (711, 235, 246) under both rules). It is neither the UO
graph's doing nor the sprint's: over seeds 33 to 95 odd the 1.1 graph
stalled 1 230 bot-ticks, all but one spell of them there (seed 41), the
UO graph 0 without the sprint and 431 with it (VERIFIED, measured);
which seeds send a bot down that stair is chance.

The stair turns there (VERIFIED, the brushes): the upper flight runs
south along x 696 to 736 down to a landing at z 238 over y 208 to 248,
the lower one west from x 696, and the newel between them is the
brush x 688 to 696, y 248 to 296. A body coming off the upper flight at
(711, 236, 246) presses the newel's corner with the 2 units of its
radius past y 248, and the run at (670, 233, 222), 1 unit south of the
line it slides along, slides it south slower every tick, never the last
unit (VERIFIED, `ship_stalls` `TRACE=4:1000:1060`, seed 13: y 240.7 to
234.2 over 17 ticks, then the unstick spell). `unpin_yaw` would have
turned it 10 degrees south, but it took only a waypoint within a step of
the body's height, and this one is 24 down. It now also takes one off
the body's floor up to a step plus 0.75 a unit across (a stair's 32 in
48 and some), and its runs stay on the ground between a step below the
lower of the two floors and a step above the higher. VERIFIED (measured,
2026-10-15, same runs, seeds 1 to 63 odd): `mp_uo_stanjel` 0 stalled
bot-ticks (seed 13 had 1 561), 2 falls (seeds 43 and 49, elsewhere on
the map, no death).

VERIFIED (measured, release `ship_stalls` `UO=1`, `tdm`, 6 bots shooting
off, 2400 ticks, master 447d904f and 47171373 alike, sprint on): over
seeds 1 to 31 odd, sprinting / stalled / falls `mp_cassino` 27 959 / 6 /
0, `mp_foy` 41 178 / 10 / 0, `mp_italy` 41 201 / 0 / 0, `mp_uo_stanjel`
20 793 / 0 / 1; over 1 to 63 odd, stalled / falls 29 / 2, 768 / 0, 0 / 0
and 13 / 2. `mp_foy`'s 768 are seed 51's one spell (751): a bot in the
hollow at (4635, -2241, -12) jumps for the edge to (4604, -2199, 18)
from (4601, -2213, -14), 14 units short of the top node and 32 under it,
peaks at 25 against the lip with no ground gained, lands back, and
replans to the foot node and back for about 1 000 ticks (`TRACE=2`,
ticks 1040 to 2000). Not looked into further.

The hollow on master ee4c0d0b (VERIFIED, measured, ours, release, UO
rules, `ship_stalls` `UO=1 MAP=mp_foy GAMETYPE=tdm TICKS=2400`): seed 51
stalls 0 bot-ticks and seeds 1 to 63 odd 20 (seeds 13 and 49, elsewhere
on the map); no run puts a bot in the hollow. The graph no longer has
the edge out: `nav_build` (`UO=1 GAMETYPE=tdm`, digest
2a7ea2623d0acc4a) gives the hollow five nodes, (4584, -2236, -17),
(4608, -2214, -14), (4612, -2234, -15), (4648, -2235, -11) and (4652,
-2208, -13), a component of their own that four rim nodes drop into
((4608, -2158, 36), (4656, -2159, 46), (4659, -2258, 25), (4656, -2206,
44)) and that has no edge out; the lowest rim stands 38 over the floor,
a unit under `JUMP_HEIGHT`. A bot placed in it roams only its own five
nodes (`NavGraph::roam_pool`: the largest component it reaches is its
own) and paced there all 600 ticks of three seeds. So the jump from 14
units short is no longer reachable; what is left is the pit. A bot
whose nearest node on its floor reaches no node of the largest
component (`NavGraph::pit`) is now `BotView::stranded`, as on a dead
end, and types `kill` after `STRAND_TICKS` (`tests/bots.rs`
`a_bot_in_mp_foys_hollow_kills_itself`). INFERRED: a room behind a
gametype's walls reads as a pit until the rebuild over their removal
lands (`nav_build` without `GAMETYPE` leaves ten of mp_foy's 391
spawns and ten of mp_italy's 371 reaching no node of the largest
component; with `GAMETYPE=tdm` all of mp_foy's reach it), so the rule
waits for a graph over the script's current brush links
(`Server::nav_settled`). Nor is a component a pit when a spawn that is
cut off too reaches it: a bot spawned there would kill itself over and
over. On mp_italy's `bas` (four shooting bots, seed 5) the allied
bunker spawns round (6682, -2624, -706) reach no node of the largest
component on the settled graph, nor does the charge room at z -898
under them (VERIFIED, measured, ours, `BAS_TRACE`): with the rule
counting them as pits, the allied defender spawned there, walked down
to keep the charge room and typed `kill` three times between ticks 7100
and 7800 (round over at 11679, against 12959 with no pit rule). With the
spawn test the rule never fires on that run, which ends on 12959.
mp_dawnville's spawns 38 and 74 ((-652, -17194, 56) and (-648, -17088,
80)) reach no node of it on 1.1 and UO alike, so they are no pit
either. The allied bunker's way out was its base's two ladders, which
the build read only off model 0 (section 2, "Ladders"); since
2026-10-19 its spawns reach the largest component.

## 3. Following

The tick runs in four passes over the bots in slot order
(`bots::driver::step`, one driver over a game's `BotWorld`): every bot's
server commands first; then every bot's view (enemy refresh, view,
hearing, heal, sidestep, cover, in that order per bot) and the objective
roles over all of them; then per bot the retreat's cover, the hunt's
vantage, the goal, the seat, the follower below, the drop and lob probes
and `think`; then the replies, gamestates and cmds. The follower's rand
draws come off the bot's own generator, so a bot's walk depends on the
plan budget the bots before it in slot order spent that tick, not on
their draws. VERIFIED (`bot_match` and the `bots::` tests byte-identical
before and after the driver moved behind `BotWorld`, 2026-10-23).

CoD2's bots run the same driver in the same order. The route leaves out
what the trait's consts name: no yields, no crowd and no blocked nodes
(`YIELDS`), and none of the edge cues that step CoD1's pmove (lip, push
off, unpin; `PMOVE_PROBES`). Turned on, each made CoD2's bots stall
more (bot-combat.md 15). Between the route and the drop probe
`pre_think` walks a bot to a gun or a dropped gun. VERIFIED (`bot_match
DIGEST=100` over CoD2 tdm on three maps and dm on two, one 1.1 and one
UO run, byte-identical before and after CoD2 moved onto the driver,
2026-10-23). CoD2 used to hear its noises after the frame's objective
and gun reads, look up the hunt's vantage at the end of each view and
probe the lob before the goal; on CoD1's order its `bot_match` gates
(`DIGEST=250` included) stay byte-identical, once the noise pass reads
the level's clock: at the tick's top the frame's pass holds none, and a
smoke cloud aged against 0 was dropped the tick it burst. VERIFIED
(`bot_match` CoD2 tdm and dm gates, 2026-10-23).

The brain stays pure. Each tick the server asks `Bot::goal(&BotView)` for a
`Goal`: `Hold` (dead, spectating, mid-throw), `To(point)` (the enemy it sees,
else a spot it remembers or heard, section 4), `Roam`, or `Away(threat)`
(the hunted side of `bel`, `bot-objectives.md` section 5). A per-bot `nav::Follower` turns that into `BotView::waypoint`:

- A* (Euclidean cost and heuristic) from the node nearest the bot to the node
  nearest the goal. The bots of a tick share a budget of 4000 expanded
  nodes (`bots::driver::PLAN_BUDGET`). A search is resumable (`nav::Search`): each
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
- A `Roam` (and an `Away` flight) picks its destination among the nodes
  of the largest component the start reaches, its own included
  (`NavGraph::roam_pool`). VERIFIED (measured 2026-10-12, `mp_depot`
  `dm`, the detector below, seeds 1-15 odd, traced): two spawns,
  (-2326, -1436, 35) and (-2596, -1437, 32), stand on a 25-node platform
  at z 27 (x -2621 to -2304, y -1497 to -1384) whose only ways off are
  drops to the floor at -32, a component of its own. Picking in the
  start's own component, a bot spawned there ran the platform's length
  back and forth for the whole run: 642 stalled bot-ticks over the 8
  seeds, nearly all of them there, against 37 after. A pick in any
  reached component would let a bot on the main one roam into a pit it
  never leaves; the largest is the main one wherever the start is in it.
  A pick of a perch (section 2) is passed over (2026-10-13f, below:
  `nav::tests::a_roam_never_ends_over_a_fall_that_kills`).
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
  the next node than the waypoint is, either only from 48 units above it to
  a step (18) below it: a ladder's head stands right above its foot, and
  from the rungs under the lip the next node is no way on. A node the next
  edge drops more than a step from is passed only within 8 of its height,
  a jump's top within 16: the drop was proved from the node, not from beside
  or under it. A jump's top is never passed by being nearer the next node:
  a body still in the air beside the top is that too, and falls back to
  the foot. A jump's top, and a node more than a step below the body, are
  passed only on the ground, and on a ladder a node below only from a step
  over its floor (2026-10-11, below). A pass also runs the walk on from
  where the body stands, onto the next node's floor, at the body's own
  velocity, not the graph walk's running start straight at the target
  (`Follower::edge_holds`; not on a climb steeper than a jump, nor from
  more than 8 under the waypoint's height, as on a ladder under its
  head): the graph proved the edge from the node, and a body up to 24
  units off it, or standing on a lip over its floor, or coming onto it at
  another heading, can meet what the node's walk did not. A drop deeper
  than a jump is walked on too when the graph's forward walk ran off it
  (`Follower::runs_off`; a drop that walk misses was backed down, and is
  taken as proved), and the bot then runs off it rather than backing onto
  it (`BotView::run_off`, 2026-10-12, below). Where a miss of such a drop
  can hurt (`nav::a_miss_hurts`: a point 32 to 96 units out from 48 over
  the landing, in the open, with no floor within 256 under the landing)
  the run has to land from 2 units either side of the body across its
  pace too, unless the body stands within 12 of the node and, turned
  round with no key held, would coast off a lip anyway
  (`nav::coasts_off`, 16 ticks) (2026-10-18, below). A run on that is
  neither a jump nor down a step, comes down off the ground under where
  it set off and lands where a miss can hurt holds only if neither walk
  2 units aside falls deeper than a jump; a jump edge whose walk on from
  the body arrives without the jump is run without it (2026-10-20, "The
  crest run and the stringer's jump").
  Where that walk fails the bot heads on at the waypoint, and within 12
  units of it flat takes the edge out of its plans as a stuck one, unless
  the plan took the edge while it was out of its plans (no way round it):
  then it is taken as it is, or the bot would drop it again every tick and
  stand without a waypoint (2026-10-12, below), but from at rest on the
  node: the walk on is not run again there (2026-10-13d, below). A
  waypoint four grid
  steps away means the bot left the path: plan again. Forty ticks without
  closing on a waypoint, height counted with the flat distance so a climb
  closes in, drop the path, and the edge the bot was on stays out of its
  plans for 200 ticks (10 s); when no path goes round it, the plan takes it
  anyway. Stuck before the first node, the bot is left 20 ticks to its own
  unstick. On the ground more than 48 under its waypoint, with no ladder
  leading there, the bot fell off the path: it plans again at once, round
  the edge it fell off. On the ground more than a jump over its waypoint,
  within 24 of it flat and more than a jump under the edge's top, it
  landed partway down a drop and plans again too, the edge kept
  (2026-10-18b, below; a jump, not a step, since 2026-10-18d). On the
  ground off the rungs, it holds still for up to 60 ticks while its step
  toward the waypoint would close within two half-widths and 6 flat of
  a climber rising under it whose top is less than 48 under its feet,
  unless the waypoint is more than a jump down; and of two bodies
  pressed together on the ground (within two half-widths and 2 flat,
  a step in z), both slower than 20 units/s for 10 ticks and within
  48 of the waypoint, the one farther from it steps 24 back from the
  other for 20 ticks (2026-10-18d, below). On the ground on a run-off drop's top, the
  waypoint more than a jump under it and more than 24 off flat, slower
  than 20 units/s, where the forward walk from the body at its pace no
  longer lands, it plans round the edge (2026-10-18c, below). That plan starts not at the node nearest the body
  (`NavGraph::nearest`) but at the first of the 8 nearest by that score a
  walk from the body reaches on its floor without a jump
  (`NavGraph::nearest_walkable`): a body off the graph can stand where the
  nearest node is a wall away (2026-10-11, below). Where none of those
  arrives, the 8 nearest flat among the nodes more than a jump below the
  body get the same walk. Any plan from a body whose nearest node stands
  more than a step (18) off its height starts the same way
  (`NavGraph::off_floor`; 2026-10-12c, below), and so does the plan
  after one the body never closed on the first node of (the follower's
  stuck arm before its first node): VERIFIED (measured,
  `uo_bots_fight_and_sprint_in_tdm` `SEED=1` with `vc_bot_vehicles 0`,
  2026-10-21) an unstick spell left a bot in a slot on mp_cassino at
  (945, -775, 64) whose nearest node, (1008, -767, 64), stands across
  its east wall; every plan started there and the bot pressed at the
  wall from tick 1572 to the 12 s gate. With the walked start it plans
  from (912, -719, 64) and is out at tick 1722. A plan begun on a
  ladder starts at the nearest, by the same score, of the nodes a climb
  of more than a jump leaves from or leads to within two columns
  (`NavGraph::nearest_ladder_end`): the node nearest a body on the rungs
  can be a floor behind the climber's back, which a ladder move does not
  reach (2026-10-12, below). Such a plan's first node over the feet, the
  head, is passed only on the ground, like any climb's (2026-10-13d,
  below). When such a plan, or a parked one taken up again, starts at
  the ladder's foot or head and its next node is the ladder's other
  end, a body on the rungs between them within a node spacing of the
  foot heads straight for the other end: the start is behind it on the
  same ladder (`Follower::follow`; bot-combat.md 17.2). A bot jumps only on a
  jump edge, never toward its first node. VERIFIED (measured, `mp_depot`
  `re`, 2 bots, seed 7): with jumped walks counted, a bot that fell off
  the leap to the documents' crate into the gap at z 84 planned from the
  step node (-1822.1, -608, 116.1) 32 over it and stood under it for the
  rest of the run; for every plan, not only after a fall, the attacker
  never reached the documents in 60 s either.
- A ladder's ends are passed by their own rules (`Follower::rising`,
  `Follower::climbing`). Its head, the node a climb of more than a jump
  leads to, is passed only standing on the ground: from the rungs, or in
  the air over the lip, a body that turns for the next node lets go of the
  ladder. Its foot is passed only within 8 units (the walk-on proof's
  `ARRIVE`), never by being nearer the next node, unless the foot stands
  square in front of the rungs (`NavGraph::face_ahead`: inside the
  face's span along it, within 48 across); such a foot is passed within
  the usual 24. From a square foot the waypoint handed out is the point
  on the face straight ahead of the foot, at the head's height, until the
  body is within a step of the head's floor: the climb that found the
  head ran from the foot square into the face (`rung`), and a run at the
  head itself from a few units aside grabs the rungs at their edge. A foot
  off to the side is a flood walk that met the ladder, and keeps the
  heading at the head its walk proved.
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
did. Off a ladder, a drop the graph's forward walk ran off is run off
(`BotView::run_off`). On the rungs with the waypoint a floor behind the
climber, 8 or more units out from the face and from a jump under to a
step over the body, the bot pushes off: it jumps facing the face, which
`PM_CheckJump` reflects straight out, then holds the forward key facing
the same way until it lands (`nav::push_off`, `BotView::push_off`). Until
2026-10-19 it jumped facing out and braked on the back key, the same
flight, but `bg_ladder_yawcap` turns a view more than 100 degrees off the
rungs back onto the cap on the jump's frame and the jump with it
(cod11-mantle.md, "The ladder yaw cap"). It pushes off only where that flight, run on
a copy of the body, lands on the waypoint's floor within 64 units of it.
A ladder move climbs and strafes along the face but never steps off it,
so the jump is the only way onto such a floor, and the jump alone, run on
at the floor, overshoots it (2026-10-12b, below). On the ground the move
keys are cut by the held weapon's `moveSpeedScale` (`nav::paced_key`),
so a light weapon follows at the 190 units/s the graph's walks ran at,
not its own 215 to 228; the tick a jump goes in, the air and the rungs
keep the whole key, as the wish there takes no weapon scale (pmove
`wish_air`, `cmd_scale`). The probes that run the body's own move
(`lip_ahead`, `unpin_yaw`) hold the same key, and the follower's
`move_scale` is the weapon's cut to 1 (2026-10-13c, below). With no
waypoint while the next plan is searched for, a bot on the ground comes
to rest on the last waypoint it was handed, within 48 units flat and a
step of its height, for up to 40 ticks, as on a leap's foot
(`Follower::rest_point`); a follower resting on purpose (stuck before its
first node) still leaves the bot to wander. On the ground, slower
than 20 units/s and short of a waypoint on its floor (or up or down a
stair from it, since 2026-10-15: a step plus 0.75 a unit across), a bot
whose run straight at the waypoint closes less than 2 units in 4 ticks
takes the heading, of those 10 to 90 degrees either side every 10 and
the compass axes within 90, whose 4-tick run closes most and by 8 or
more, staying on the ground between the two floors and off hazards, and
holds it 4 ticks (`nav::unpin_yaw`, `BotView::unpin`); the stall count
waits for it. A bot that
has not left a 15-unit circle in ten ticks takes a random heading for 15 ticks
whether it has a waypoint or not, unless it closed on its waypoint by 2
units in them (across plus height, since 2026-10-13b); a spell's end
starts the ten ticks afresh. The circle is the farthest the body went in
the ten ticks (since 2026-10-13d; the window's two ends before). On a
ladder it climbs down instead, as it does
with no waypoint: the heading is nothing to a body on a ladder, and the
level view of a wander climbs at a third of the rate. A spell that starts
within 10 ticks of the rungs climbs down on them and holds still off
them, and so does a body in the air within 10 ticks of the rungs with no
waypoint; a body on or within 10 ticks of the rungs, within 24 units
under a waypoint less than a jump over its feet, is not stuck at all: a
climb bobbing under a ladder's top (`Bot::topping`, 2026-10-13d, below). A leap's foot is slowed
onto only within 48 units of its height and off a ladder; one at a ladder's
head is climbed to at the full rate first. VERIFIED (measured, `mp_ship`
`dm`, 6 bots, shoot off, seeds 1-4, 4000 ticks): before, a bot pushed up
under a spar on the deck ladder at x 3696 at z 808.875 for the rest of the
run, and bots took 60 s up the hold ladder at 0.4 units a tick, slowed
onto the leap's foot at its head. With the climb down alone, seed 1 still
had 1 501 bot-ticks within 150 units of where the bot stood 12 s before,
all on the hold ladder; with both, seeds 1-4 had 7 to 59, none on a ladder
(`tests/bots.rs`, `bots_on_mp_ships_ladders_keep_climbing`). The 59 were a
bot at (2180, 407, -45) against `mp_ship`'s hull beam, on the edge an
edge of the graph wrongly led to (section 2, "The flood"). VERIFIED
(measured 2026-10-09, same detector, seeds 1 to 15 odd: the server ORs a
seed with 1, so 2 runs as 3), bot-ticks kept within 150 units of where
the bot stood 12 s before, beam area (x 2080-2280, y 350-560, z under -20)
in brackets:

| seeds 1 3 5 7 9 11 13 15 | total | beam |
|---|---|---|
| before | 58 19 6 7 51 16 0 3 = 160 | 56 + 37 = 93 |
| node floors within 8 | 3 19 35 139 13 11 6 13 = 239 | 0 |
| and the drop pass | 1 5 49 14 0 25 0 59 = 153 | 4 |

A run diverges from the first changed waypoint on, so the totals off the
beam move by chance as much as by cause. Seed 7's 139 with the floors alone
were one bot that passed a node at z 664 from 42 under it, then headed for
the drop beyond, to z 480; the drop pass took that to 14. With both, the rest are seed 15's 59, a
bot at (5832, 561, 56) that passed a node 20 above it and headed for the
stair beyond, seed 5's 49, a bot climbing a ladder and dropping back
near (5350, -100), and seed 11's 4, a bot on the floor heading for the
beam's edge node (2208.5, 444.7, -47.875). Seed 3's 19 before were a bot at
a ladder's foot at (1486, -160, 8) whose stuck count kept restarting.

VERIFIED (measured 2026-10-09, the same 6 bots, `dm`, shoot off, 4000
ticks; a bot-tick counts when every position of the bot over the 12 s
before it lies within 150 units of the first, so a bot that walks off
and comes back is not counted, unlike the table above, whose detector
was not kept): bot-ticks stalled on `mp_ship`, release build (the test
profile gives the same counts).

| | seeds 1 3 5 7 9 11 13 15 | total | seeds 17 to 31 odd |
|---|---|---|---|
| before | 70 31 82 2 41 35 94 20 | 375 | 172 |
| after | 40 0 69 3 2 0 0 19 | 133 | 27 |

The fixes, each from a stall traced tick by tick (a run diverges from
its first changed waypoint, so most turned up in the runs between the
two rows, not in "before"):
- Seed 5 before, (3347, -150, 616), and seed 9 at the same spot: the jump
  from the deck at 616 to a ledge at 652 by (3360, -90). The bot passed
  the top in the air, nearer the next node, and fell back to the deck.
- Seed 15 before, (2214.8, 446.0, -47.9), and two later runs at
  (2229, 480, -47.9) and (2251, -388, -47.9): a bot on a hull beam's edge
  ledge beside the edge node, heading over the beam on the walk the node
  proved, sliding back off the slope. A later run also followed the
  flood's merged edge onto a beam's top (section 2, "The flood").
- A later run on the mast ladder at (3696, 56): bots passed its head at
  992 from 25 under it and headed for the next node across the deck from
  the rungs.

Seed 5's 69 were a bot that tips off the ladder at (3280, -460) onto
the deck at 56, is pinned 10 ticks at x 3288 short of the next node, and
takes the random unstick heading off the deck's lip to the hull floor
at 7 and -64; the heading avoided hazards, not drops. Seed 1's 24
were a bot on the ship beam's slope under its top node (2172.9, 418.2,
-31.875), seed 15's 18 a bot under a stair to z 104 at (2151, -236, 56).

On a ladder with the waypoint more than a step above, the climb faces
square into the face (`nav::climb_yaw`, off `ps.ladder_normal`) and
strafes along it, up to move key 64, toward the climb's line: the foot's
place along the face when the path came up from a square foot
(`Follower::climb_line`), else the middle of the rungs
(`NavGraph::ladder_middle`). Level with the waypoint it heads at it, so a
ledge beside the rungs is stepped onto facing it. Backing down the rungs
it faces square into the face as well and strafes toward the waypoint
along it: on a ladder only the strafe moves a body across the face, and
only one facing it (`ladder_move`, the right vector projected onto the
face's plane), so a body backing away from a waypoint along the face
stays where it grabbed.

VERIFIED (measured 2026-10-09, `mp_ship` `dm`, 6 bots, shoot off, 4000
ticks, the detector below, each traced tick by tick):
- The mast hatch, ladder plate x 3711-3712, y 43-69, z 760-992, in a hole
  in the platform at 992 about 33 units across: bots climbed to 994 on the
  rungs, passed the head (3705, 62, 992) and the next node from there,
  turned west for the platform node (3648, 71, 994), let go of the rungs
  and fell down the shaft to the deck at 760, again and again (seed 19,
  tick 895 on). With the head passed only on the ground, the body goes
  over the plate's top facing the rungs, lands on it at (3701.7, 64.2),
  and walks on along the platform.
- The ladder at x 3792-3793, y 53-77 (304 to 623): a flood foot at
  (3814, 33, 304), 20 units south of the rungs, was passed from 18 off;
  the run at the head from there pinned the body at (3799, 37) beside the
  ladder, under the deck.
- The ladder at x 3353-3354, y 496-520 (56 to 352): the head (3323, 481)
  lies south-west of the foot (3370, 508). A run at the head drifts south
  along the face and grabs at y 495, the ladder's edge, where the climb
  pins at z 281.9 under a spar. The graph's own walk from the foot toward
  the head arrives from y 504 to 516 and falls or blocks from 500 and 495.
- On the mast the same: a body that grabbed at y 68.1 pinned at z 808.9,
  the spar the climb at a third of the rate used to stop under; one at
  y 53.5 climbed past it.
- A climb strafing to the rungs' middle (y 56 on the mast) landed on the
  plate at y 57, where the walk west across the hole falls; the head was
  found from the foot's line, y 62 to 64, where it arrives. Hence the
  foot's line, not the middle, where there is one.

The crossing from the mast's plate to the platform is marginal: a walk
from (3705, 62) or (3701.7, 64.2) to (3680, 71, 994) arrives, one from
(3699.2, 57.2) or (3705, 56) falls, and the edges to (3648, 32, 994) fall
from all four. A bot whose plan starts at a head while it is still on the
rungs passes it there and can still fall back down the shaft; this is
most of what is left below. Since 2026-10-09 the graph no longer crosses
there (section 2, "Ladders": perched heads).

A random heading is never one with a hazard 64 units along it
(`BotView::hazard_ahead`, one flag per 45-degree octant), nor, standing
on the ground off a ladder, one with a drop deeper than a jump (39) within
64 units before a wall (`BotView::drop_ahead`, `nav::drop_ahead`). The
probe sweeps the standing capsule a step up along the octant, then feels
for the floor every 16 units with the capsule, each floor against the one
before, so a stair down is no drop and a body overhanging a lip is held by
it. Since 2026-10-13d it also feels half way between (a point trace, the
capsule only where that finds no floor) and where the sweep stops short
of a wall: mp_ship's crow's nest hatch (below). Since 2026-10-13f the
server runs the probe only on the ticks the brain can read it
(`Bot::reads_drops`: no waypoint, a spell, guard, or a stall window's
last tick with the body still in its circle), under 1 % of bot-ticks
(below). A heading is picked first with no drop in its octant or either
neighbour (a heading into a wall slides along it: on the deck, heading 297
slid the bot south along the wall at x 3288 and off), then with none in
its own octant, then off hazards alone. In the first two tiers a heading
whose octant pins the body on a wall within 4 units comes after every
open one (`BotView::wall_ahead`, `nav::wall_ahead`: the capsule swept a
step up, the wall less than 45 degrees off square): the drop probe reads
such a heading as no drop, though it goes nowhere. A wandering bot picks again when a
drop turns up in its heading's octant or in its velocity's. Two cases take
the drop anyway: a waypoint more than a jump below (the path drops there),
and a bot cornered, its third unstick spell in a row starting within 96
units of the one before. VERIFIED (measured, seed 27 of the table
below): without the second, a bot on the 8-unit rim at (4223, 345, 276) beside the deck
220 below, under the ramps to its waypoint at z 349, paced the rim for
2 248 bot-ticks; the drop is its only way out. Engaging an enemy overrides
all of it.

VERIFIED (measured 2026-10-09, the same detector, 6 bots, `dm`, shoot
off, 4000 ticks, release build; "falls" are unstick spells that start on
the ground off a ladder and end 48 or more below where they started,
stair descents at a run included):

| | seeds 1 3 5 7 9 11 13 15 | seeds 17 to 31 odd | falls, all 16 |
|---|---|---|---|
| before | 43 0 70 3 3 0 0 19 = 138 | 0 0 11 4 0 0 13 0 = 28 | 40 |
| drops never taken | 28 943 0 8 1 286 0 19 0 = 2 284 | 1 129 11 17 3 13 2 248 0 553 = 3 974 | 13 |
| shipped | 0 0 16 8 3 0 0 17 = 44 | 0 17 11 3 17 10 0 0 = 58 | 19 |

A cornered bot still keeps off a drop that hurts: the probe reports the
drop's depth, and one past the default `bg_fallDamageMinHeight` (256)
stays shunned. VERIFIED (measured, seed 5 with the climb changes above):
a bot cornered at the ladder at x 2206-2230 (y -150, z 234-314) took its
heading west along the floor at 216 and ran off its end at x 2010 to the
hull at -64, 280 below. A run down a stair can still leave the ground at
its top and land a storey down: the deck test
(`bots_on_mp_ship_unstick_without_walking_off_the_deck`) counts a fall
only where the graph's way back is longer than twice the flat distance
plus 128. The original deck lip fall, (3285.5, -448.5, 56) to
(3247.4, -390.3, -63.9), has a 167-unit way back and is not counted:
the probe prevents it, but it was never a trap.

VERIFIED (measured 2026-10-09, the detector below, `mp_ship` `dm`, 6
bots, shoot off, 4000 ticks, release build): bot-ticks stalled, before
the ladder changes and after.

| | seeds 1 to 63 odd | seeds 65 to 127 odd |
|---|---|---|
| before | 210 | 274 |
| after | 50 | 265 |

In the second set before, the stall spells were on the ladders at
x 3354 (7), x 3793 (4), the crow's nest at x 4960 (2) and the mast (4),
and in the hull corridor at (3825, -800, -64) (2 seeds). After, the
ladders at x 3354 and 3793 have none in either set and the crow's nest
one, in the first; the mast has 2 in the first set and 7 in the second,
and the hull corridor (seed 95, 50 bot-ticks in both builds) is
untouched. On other maps,
seeds 1 to 11: `mp_depot` 176 to 106, `mp_pavlov` 117 to 47, `mp_harbor`
0 and 0. A run diverges from its first changed waypoint, so a seed's
count moves by chance as well as by cause, and a 32-seed total by tens.

VERIFIED (measured 2026-10-09, same detector and settings, release
build): bot-ticks stalled before and after the perched heads above, the
hull corridor's edge check and the drop probe's slide (below), with the
stall spells that sat at the mast hatch (on the rungs at x 3696 or on the
deck under it), in the hull corridor at (3825, -800, -64) and at the hold
ladder's head (x 4315-4360, y -110 to -80).

| | seeds 1 to 63 odd | 65 to 127 odd | 129 to 191 odd | total | mast / hull / hold spells |
|---|---|---|---|---|---|
| before | 50 | 265 | 199 | 514 | 16 / 6 / 0 |
| after | 81 | 246 | 136 | 463 | 0 / 0 / 4 |

- Hull corridor. VERIFIED (measured, seed 95 traced tick by tick, box
  traces and walks): two hull beam patches (104 and 106)
  run along y -800 at the floor, 22 units high, and the end of patch 106
  stands at x 3840 for y -802 but clears a body at y -805. The node at
  (3837.1, -804.9) walks to (3879.3, -800) without a jump. The bot passed
  that node 3 units north of it at (3822, -802), headed on, and pinned
  on the beam's end at x 3824.9 for every try. From there the walk does
  arrive, but only by jumping, and `Follower::edge_holds` took any
  arrival as proof. Now a walk that needed a jump proves only a jump
  edge, so the bot closes in on the node first
  (`nav::tests::an_edge_that_needs_a_jump_from_aside_does_not_hold`).
- The drop probe's sweep stopped at once against a wall the heading
  only grazes, so it saw no drop where the body slides along the wall
  and off. Against a wall less than 45 degrees off the heading, within
  the first 16 units, it now feels along the slide. VERIFIED (measured,
  seed 5 with the graph change): a bot at (6095.8, -303.9, 97.1) against
  brush 316 (normal (-0.98, 0.21)) took heading south, slid off the ledge
  and fell 41 units to the deck at 56; the probe due south read nothing
  and reads 41 now (`nav::tests::a_drop_ahead_is_felt_along_a_wall_it_slides_on`).
- The hold ladder's head (4332, -87.1, 615.1) is new in this table.
  See below.

VERIFIED (measured 2026-10-10, seed 149 traced tick by tick with the
bot's cmds, walks and box traces): the hold ladder's head stall.

- The head stands on the ladder brush's lip (y -80 to -72, top 615.1)
  over the shaft, which runs down to 56 from y -80 to about -130; the
  floor at 600 lies west, south and east of it. The climb tipped the bot
  onto the lip at (4333.2, -91.5). The next edge was the leap to the
  ledge at (4322.3, -158.1, 641.1). It arrives from the head node but
  falls from there, so the follower took the edge out and planned down
  the ladder. The note of 2026-10-09 had this the wrong way round: the
  leap arrives from the node itself, and the creep down arrives from
  (4335.9, -87.1) too.
- Backing down, the bot met a second bot on the floor at
  (4347.7, -110.5, 600) whose box overlapped its own. Its creep, move key
  15, slid it south-west at 0.35 units a tick instead of over the lip.
  Ten ticks inside 15 units started an unstick spell.
- At (4335.9, -87.1) the octants east, north-east, north and north-west
  pin the body at once on the wall and the ladder plate, south-west to
  south-east drop into the shaft, and west is open for 16 units. The
  spell took a heading into the wall, which the drop probe reads as no
  drop. Ten ticks later, still inside the circle, the stall check picked
  again and set the spell back to 15 ticks, so it never ended. The
  follower meanwhile handed out waypoints a walk away on the floor at
  600, which the bot never steered for: 160 bot-ticks to the end of the
  run.

Three changes: a ladder's head is no leap's foot (section 2, "Jumps");
headings into a wall rank last (above); and a stall inside a spell picks
a new heading without restarting the spell, so the waypoint gets its
turn when the spell ends (`bots::tests::a_pinned_spell_hands_back_to_the_waypoint`).
Each spell also drops the held creep heading, which belongs to where the
creep began. VERIFIED (measured 2026-10-10, same detector and settings,
release build, 192 seeds; "hold" counts spells at x 4315-4360,
y -110 to -80, "x 3808" spells on the ladder at x 3792, y 53-77):

| | 1-63 odd | 65-127 | 129-191 | 193-255 | 257-319 | 321-383 | total | spells | hold | x 3808 |
|---|---|---|---|---|---|---|---|---|---|---|
| before | 81 | 246 | 136 | 112 | 56 | 167 | 798 | 59 | 4 | 2 |
| without the spell change | 285 | 91 | 58 | 57 | 56 | 311 | 858 | 51 | 0 | 2 |
| all three | 132 | 112 | 84 | 75 | 141 | 140 | 684 | 54 | 0 | 9 |

- Without the spell change, seed 55 had 203 bot-ticks at the head of the
  ladder at x 3354: a bot at (3360.1, 496.1, 352.1) spent 175 ticks in
  one chained spell while its waypoint changed under it.
- The ladder at x 3808 (y 59 to 67, z 340 to 556) took 9 spells with all
  three changes, against 2 in each of the other rows. Each is a climb
  pinned on the rungs between z 520 and 560 and backed down by its
  spells. Traced on 2026-10-11 (below): the spell change is the cause, not
  chance.

Four mechanisms behind the follow-ups of 2026-10-10, each traced tick by
tick with `TRACE=` on 2026-10-11:

- The ladder at x 3808, seed 357. VERIFIED (measured, the trace): bot 3 climbing met bot 5 climbing down
  at z 511 (bot 5 was at (3808.1, 67.8, 598) at tick 1300 and stepped off
  over bot 3's head) and pinned; a spell took it down the rungs. The
  climb's rate both ways is 2.67 units a tick. A spell is 15 ticks, and
  the stall check had reset its 15-unit circle 10 ticks into it, so 5
  ticks into the climb back up the circle held 5 ticks down and 5 up,
  read as standing still, and started the next spell (z 511, 487, 459,
  431, 407, 354 ...). INFERRED: before the spell change of 2026-10-10 a
  pin inside a spell restarted it and carried the body further down
  before the check, so the window rarely straddled the turn. Now a
  spell's end starts the window afresh
  (`bots::tests::a_spell_end_starts_the_stall_window_afresh`). The rungs
  themselves are clear. VERIFIED (measured, climbs from (3808.1, y, 306)
  facing the face): from y 56 to 74 every climb runs to the top and
  stands at (3751.1, 616.1) or (3800.1, 623.1) without a pin.
- The ledge at (3347, 250, 616), seed 341. VERIFIED (measured, the
  trace): the path runs up the deck at
  616 by a jump onto the ledge at 652 (node (3366.2, 224, 652.1)), then
  to the floor at 664 (node (3328.2, 252.4, 664.1)). The bot came in
  along the ledge's north face (y 224) at (3367.5, 239.1), jumped, and
  slid west with its pace. At tick 2297, in the air at
  (3349.2, 239.1, 637.1), 14.9 under the top and 22.7 off it flat, it
  passed the top (`JUMP_PASS` bounds only the height), then the node at
  664 from 10 under it, turned north-west for the next and landed on the
  deck at (3347.1, 263.5). There it pushed at the 664 floor's face, 48
  over it, with no jump edge, until the stuck count ran out, and planned
  the same jump again. A jump's top is now passed only on the ground, as
  a ladder's head is (`nav::tests::a_jumps_top_is_passed_only_on_the_ground`).
- The hold ladder's floor at 304, seed 11 (2 292 bot-ticks in one run).
  VERIFIED (measured, the trace, the graph's edges and walks): the graph reaches the node (4318.5, -96, 304.1) only by the back down
  from (4349.5, -93.1, 600.1); a floor at 304 opens west of the shaft,
  and its edge stands 15 units west of a body hanging at x 4328.5. Bot 1
  backed down from the head, passed that node from 42 over it on the
  rungs (the band allowed 48), headed level for (4280.5, -96.1, 304.1)
  and hung at z 303.5, 0.6 under the floor's lip, on the rungs, where a
  ladder move does not step up. Two causes. On a ladder a node below is
  now passed only from a step over its floor
  (`nav::tests::a_node_below_the_rungs_is_passed_only_from_its_floor`).
  And `ladder_move` moves a body across the face only by the strafe,
  projected onto the face's plane: a back down faced away from its
  waypoint, and with the waypoint west along a face whose normal is -y,
  the strafe projects to nothing. The bot had grabbed at x 4328.5 after
  an unstick spell, where the flood's backing walk grabbed at 4322.1 and
  landed on the floor. A back down on the rungs now faces the face and
  strafes toward the waypoint; from x 4328.5 it lands on the floor at
  304.125, without the strafe it falls past
  (`nav::tests::a_back_down_slides_along_the_face_onto_a_floor_beside_the_ladder`).
- The "sunken floor" at (5424, -187, 344), seed 59. VERIFIED (measured,
  the trace, point and box traces, the graph's nodes and walks): no
  floor. A point
  trace down there hits the stair at 272 (brush 790). The body stands on
  a lip of the slope under the shelf at 396 (brush 2194, 316.3 at
  y -176), its box half over the stair, which runs from 352 at x 5396
  down east to 204 at x 5470 along y -197. The flood rightly has no node
  there. Bots land on it off the shelf: the path drops from the floor at
  448 at (5343.4, -155.3) to the shelf node (5383.6, -161.1, 396.1), and
  a bot that comes onto that drop from the north carries its pace south
  over the shelf's edge (missed by 3 units at x 5430). The bot passed
  the shelf node from 48 over it in the air, turned for the next, and
  landed on the lip, 52 under its waypoint, where it pushed at the
  shelf until the stuck count ran out. Then `nearest` started the plan at
  (5472, -126, 352.1), beyond the shelf's side, and a node it did not
  reach; the stair node (5413.7, -193.1, 320.1) it did reach was passed
  from 24 over it, its walk on taken as proved, and the bot bounced on
  the slope. Four changes: a node more than a step below is passed only
  on the ground; from the ground over a node's floor the walk on has to
  arrive (`Follower::edge_holds`); a body on the ground more than 48
  under its waypoint, off a ladder, plans again round the edge it fell
  off (`nav::tests::a_follower_that_fell_off_its_path_plans_again`); and
  that plan starts at the first of the 8 nearest nodes a walk reaches
  (`NavGraph::nearest_walkable`; from the lip the stair node is sixth,
  the five before it blocked or falling;
  `nav::tests::a_plan_starts_at_a_node_the_body_walks_to`). Traced, the
  bot now leaves the lip down the stair 3 ticks after it lands.

VERIFIED (measured 2026-10-11, same detector and settings, release
build; the master column is bec4f62d; spells counted by where they
start: x 3808 as in the table above, the ledge at x 3330-3400,
y 200-300, z 600-680, the shelf area at x 5390-5530, y -210 to -150,
z 280-460):

| | seeds | stalled bot-ticks | spells | falls | x 3808 | ledge | shelf area |
|---|---|---|---|---|---|---|---|
| master | 1-383 odd | 684 | 54 | 376 | 9 | 7 | 3 |
| after | 1-383 odd | 620 | 45 | 338 | 2 | 0 | 6 |
| master | 385-767 odd | 493 | 43 | 342 | 1 | 4 | 2 |
| after | 385-767 odd | 567 | 42 | 264 | 0 | 0 | 4 |

"Falls" counts unstick spells that end 48 or more under where they
started, stair descents included. Over the 384 seeds the totals move
from 1 177 to 1 187 bot-ticks, 97 to 87 spells and 718 to 602 falls.
The detector is deterministic (the graph is built on the tick, not in
the background), but a run diverges from its first changed tick, and
two builds here that differed in one rule moved a 192-seed total by 100
to 300: only the per-place columns say anything. The two x 3808 spells
left are bots climbing down to the foot at 304, at z 570 with another
bot 61 units off (seed 1) and at z 515 with none within 400 (seed 91);
neither is traced. One new place: seed 643 held a bot 100 bot-ticks on
the ladder at x 3354 under a floor at (3427, 546, 128) behind its back,
which a body on the rungs leaves only by the push-off jump. The shelf area
keeps two traps that are not the lip: the drop from the shelf node
(5402.6, -160.2, 396.1) to (5404.9, -192, 204.1), which falls through a
slot between the stair's top and the shelf (a body dropped at
(5402, -192) from 430 lands at 204) and which a bot backing off the
shelf misses onto the stair beside it, then walks the stair over a node
140 under it; and the corner at (5375.1, -193.1, 352.1), where the edge
from (5377.1, -193.1, 352.1) to (5344.4, -156.1, 352.1) pins a bot that
the walk from the same spot with a running start gets past. Neither is
traced to the end.

Three of those, traced tick by tick with `TRACE=` on 2026-10-12, and one
found on the way:

- The shelf slot. VERIFIED (measured, seed 47 bot 4, the trace and
  walks): the bot came west along the shelf at 396 at (-218, -2) units/s
  and passed the shelf node (5402.6, -160.2, 396.1) at (5407.5, -165.5).
  The slot node lay 192 below, so it backed toward it facing away,
  creeping, drifted west along the shelf and pinned on its south edge at
  (5389.4, -192.3); spells took it off the shelf and down the stair. The
  graph's forward walk to the slot node arrives from every start within
  8 units of the shelf node and from most within 12 (a running start
  straight at the target, re-aimed every tick); the backward and creep
  walks from the node fall. Now the pass re-proves the drop at the
  body's velocity and holds the node while that fails, and a drop the
  forward walk ran off is run off
  (`nav::tests::a_drop_is_proved_again_at_the_pace_the_bot_comes_onto_it`,
  which fails with the drop taken as proved). Traced, the bot holds the
  node, reaches it at (5406.2, -160.8) still at pace, drops the edge
  there and plans round by the floor at 352 to the north.
- The corner at (5375.1, -193.1, 352.1). VERIFIED (measured, seed 139
  bot 2, the trace and box traces): the bot came up the stair from the
  east, passed the node (5377.1, -193.1, 352.1) and stopped dead at
  x 5375.1 against brush 2194 (normal (0, -1, 0), the shelf's slope).
  Run from rest there toward the next node (5344.4, -156.1, 352.1) the
  body moves 0.003 units a tick; from rest on the node itself it slides
  west along the face and arrives. The walk on at the bot's pace from
  (5389.7, -196.4) is blocked where the running start arrives, so the
  node is held and, on it, the edge avoided. That edge has no way round:
  the plan took it again, the pass dropped it again, every tick, and the
  bot stood without a waypoint until an unstick spell ran it down the
  stair. An edge planned while it is avoided is now taken as it is.
- The ladder at x 3354. VERIFIED (measured, seed 643 on master, the
  trace and the graph): the bot on the rungs at (3369.1, 495.1, 154.7)
  planned from its nearest node, (3426.9, 545.6, 127.7), a floor behind
  its back, and climbed up and down the rungs under it. Higher up the
  same ladder the nearest is (3399.1, 508.5, 320.1), also behind it. The
  ladder at (3369, -379) has the same shape: with the lip change of
  2026-10-12 seed 275 held a bot 803 bot-ticks on it, its nearest node
  (3399.1, -380.5, 320.1). A plan from the rungs now starts at a ladder's
  end (`nav::tests::a_plan_from_the_rungs_starts_at_a_ladders_end`).
- Tried and backed out. VERIFIED (measured, the 192 seeds 1-383 odd):
  running the walk on with the bot's own weapon move scale, running a
  drop's walk from 2 units either side of the body as well, and slowing
  onto a node whose walk fails at pace (as onto a leap's foot) took the
  stalled bot-ticks from 559 to 2 907, nearly all on the ladder at
  (2150, -214): more bots, held off the run off its head at 216, took the
  back down to the floor at 104 by (2145, -230) and hung on the rungs at
  z 102.8 to 104.6, 16 units in front of that floor. A push-off jump
  there (the one move that leaves a ladder backwards) carried the body 60
  units back, over the floor's edge, into the hull at -64. INFERRED: that
  back down is a trap of its own (master: 12 stall spells there over the
  384 seeds); it is open.

VERIFIED (measured 2026-10-12, same detector and settings, release
build; master is 53bdf043; the shelf area as above, the corner at
x 5355-5395, y -210 to -180, z 340-360, the ladder at x 3354 at
x 3355-3380, y 485-525; each place's spells, then falls that start
there, which at the corner are unstick spells run down the stair):

| | seeds | stalled bot-ticks | spells | falls | shelf area | corner | x 3354 | x 3808 |
|---|---|---|---|---|---|---|---|---|
| master | 1-383 odd | 620 | 45 | 338 | 6 / 23 | 1 / 33 | 0 / 5 | 2 / 34 |
| after | 1-383 odd | 544 | 34 | 268 | 0 / 0 | 0 / 15 | 0 / 2 | 0 / 37 |
| master | 385-767 odd | 567 | 42 | 264 | 4 / 17 | 1 / 20 | 2 / 1 | 0 / 27 |
| after | 385-767 odd | 460 | 33 | 308 | 0 / 0 | 0 / 24 | 1 / 5 | 1 / 31 |

Over the 384 seeds the totals move from 1 187 to 1 004 bot-ticks, 87
to 67 spells and 602 to 576 falls; as before, only the per-place columns
say anything. The corner still starts spells that run down the stair:
a bot that reaches x 5375.1 at all stops there.

Three more, traced tick by tick with `TRACE=` on 2026-10-12 (the second
round that day, "2026-10-12b"):

- The ladder at (2150, -214). VERIFIED (measured, seed 189 bot 5, the
  trace, the graph's edges and walks): the rungs face north (the climber
  stands at y -214.1), the foot is (2160, -215, 56) and the head
  (2160, -156.1, 216). Edges back down from the head's floor at 216 lead
  to the floor at 104 south of the ladder, nodes (2144.7, -229.9, 104.1),
  (2112.8, -223.1, 104.1) and (2144.2, -248.9, 104.1). The bot stepped
  off the head, grabbed the rungs at (2146.5, -214.1, 214.6), backed down
  and hung at z 102.8, 1.3 under that floor and 16 units in front of it,
  until a spell took it down the rungs. A body hung there, faced out from
  the face or into it, with any move key, climbs or descends but never
  leaves the rungs. Jumping off facing out (yaw 270) from z 101 to 130 and
  then holding the back key (measured before the yaw cap was modelled;
  the same flight is now the jump and the forward key facing the rungs),
  it lands on the floor at 104 at
  y -255.9 to -258.2; holding nothing or the forward key it lands in the
  hull at -64, 120 to 170 units out; faced 45 degrees off, braked, at
  x 2116.6 or 2176.4, y -244 (`nav::tests::a_push_off_lands_on_the_floor_behind_the_rungs`).
  The point traces at x 2146.5 find 56 under y -230 to -250 and -64 from
  y -260 on: the floor at 104 holds the capsule by an edge, not the
  point.
- The corners at x 5375.1. VERIFIED (measured, seeds 65 bot 4 and 139
  bot 2, the traces and walks): the body stops dead at (5375.1, 321.1)
  or (5375.1, -193.1) against the shelf's face (brush 2100, normal
  (0, 1, 0), on the north side). From rest there, every heading from 195
  to 240 degrees (north) or 90 to 170 (south) moves it under 0.02 units
  in 4 ticks; due west moves it 30. With 150 units/s of pace west the
  run at the node north-west slides through, and from the node
  (5373.6, 326.1) it arrives. The follower had passed the node within
  reach and handed the next one out, and the walk on from the body,
  proved at the body's pace, did not catch it every time (seed 65: the
  plan began a tick after the body turned back east at 50 units/s). An
  unstick spell then ran the bot east down the stair to 204-244, the
  "falls" at the corner. Now the pinned bot runs aside
  (`nav::tests::a_body_pinned_in_a_crease_runs_aside`), the south side's
  way out being the west axis, 50 degrees off the way to the node; the
  10-degree steps alone missed it (180 pins at 179.7).
  VERIFIED (retail, `client-probes/probe_shipcrease` on `cod_lnxded`
  1.1d, `probe_fall` client with `--probe-fall-walk`, 2026-10-18): a
  body respawned at rest in the north corner runs free at yaw 200 and
  225 to (5343.12, 227.13) and at 140 to (5343.12, 332.87); in the south
  one it stays at x 5375.1 at 140 and runs at 200 and 225 to
  (5343.12, -204.87). So the north corner held ours only through the
  BVH's order: the capsule stops at fraction 0 on brushes 2100 and 2101
  at once, and since the leaf walk names 2101 (bsp-ibsp59-format.md,
  "The leaf walk") ours matches all six runs; the run at the north node
  needs no unpin. The south crease is retail's.
- The drop probe's slope. VERIFIED (measured, seed 5 with the two changes
  above, the trace): bot 2's unstick spell from (3762.4, 338.9, 352.1)
  ran north down the hull's slope, 49 units in 75, no 16-unit sample a
  jump under the one before, into a wedge at (3752.1, 413.7, 303.4) off
  the graph, and stood there (`tests/bots.rs`,
  `bots_on_mp_ship_unstick_without_walking_off_the_deck`, failed at tick
  1339). A floor more than a jump under the body anywhere within the look
  now reads as a drop of that depth
  (`nav::tests::a_drop_ahead_is_felt_down_a_steep_slope`).

VERIFIED (measured 2026-10-12b, same detector and settings, release
build; master is cb5f98d0; places as in the table above, plus the ladder
at x 2125-2180, y -260 to -185, z 50-220; "corner falls" counts falls
starting at x 5350-5399 on either side):

| | seeds | stalled bot-ticks | spells | falls | ladder (2150, -214) | corner falls | x 3354 | x 3808 |
|---|---|---|---|---|---|---|---|---|
| master | 1-383 odd | 544 | 34 | 268 | 3 / 0 | 46 | 0 / 2 | 0 / 37 |
| push-off | 1-383 odd | 685 | 56 | 255 | 0 / 0 | 50 | 1 / 1 | 0 / 38 |
| and unpin | 1-383 odd | 236 | 25 | 225 | 0 / 0 | 2 | 0 / 1 | 0 / 45 |
| and slope | 1-383 odd | 244 | 19 | 231 | 0 / 0 | 0 | 1 / 2 | 1 / 46 |
| master | 385-767 odd | 460 | 33 | 308 | 3 / 0 | 70 | 1 / 5 | 1 / 31 |
| push-off | 385-767 odd | 482 | 39 | 267 | 0 / 0 | 59 | 2 / 2 | 2 / 29 |
| and unpin | 385-767 odd | 281 | 22 | 241 | 0 / 0 | 2 | 2 / 4 | 1 / 38 |
| and slope | 385-767 odd | 165 | 21 | 221 | 0 / 0 | 1 | 2 / 5 | 0 / 34 |

Over the 384 seeds the corner falls go from 116 to 4 with the unpin (24
with its first version, 10-degree steps without the axes) and 1 with all
three; the totals from 1 004 to 409 bot-ticks, 67 to 40 spells and 576
to 452 falls. The push-off row's totals rose by chance: seed 297's hull
trap at (5150-5350, -440 to -570, 28) took 249 bot-ticks there against
143 on master, and is open. The x 3808 falls are stair descents at a run
from that ladder's floor.

Other maps, seeds 1-63 odd, master (botmaps' numbers, cb5f98d0) and with
all four changes (the roam pool above included): `mp_depot` 2 620 to 62
stalled bot-ticks, `mp_hurtgen` 3 730 and 3 730. The `mp_hurtgen` count
is one run, seed 43: bot 5 hangs off the ground, not moving, at
(436.8, 2824.3, -97.2) on the terrain slope down to its waypoint
(480, 2833, -123) from tick 78 to the end (VERIFIED, the trace); open.

Three more, traced tick by tick with `TRACE=` in the 2026-10-12c round:

- mp_hurtgen's crease. VERIFIED (measured, seed 43 bot 5, the trace and
  box traces): the bot slid off the ground at (379.3, 2806.7, -33.5) on
  the terrain down to the node (480, 2833, -123) and came to rest at
  (436.8, 2824.3, -97.2), in the air (ground normal z 0.684, under
  `MIN_WALK_NORMAL`). A capsule there touches terrain triangle 25398
  0.05 under its feet, terrain partitions 1005 and 1007 4 units to
  either side and brush 1747 (`clip_nosight`) 4 units north. From rest,
  ours holds it within 0.05 units for 40 ticks under the forward key at
  every 45 degrees, the jump key, crouch, prone and both leans. VERIFIED
  (retail, `client-probes/probe_wedge` on `cod_lnxded` 1.1d,
  `probe_fall` client, 2026-10-12): a player respawned at rest at
  (436.8, 2824.3, -97.2) settles at (436.77, 2824.30, -97.26), ours'
  (436.769, 2824.3, -97.261), and stays there 2 s standing still and 2 s
  holding forward at yaw 45; respawned 17 units above it, or at
  (430, 2810.4, -87.5) or (416.7, 2799, -60) on the slope, it slides into
  the same spot and stays. With a running start (a `setOrigin` keeps the
  walk's velocity) it leaves at yaw 45, toward (513, 2830, -133). So the
  crease is retail's, not a pmove gap, and nothing a still body presses
  gets it out; the terrain position test that master gained for
  zero-length traces (movers doc 20) does not enter into it. A player
  there has `kill` left. A bot held off the ground, off the rungs and
  unlinked within a unit of one spot for 100 ticks (5 s) now sends `kill`
  as a client command (`Bot::wedged`, driven through `client_command`
  like a menu reply), once per 100 ticks while it stays, but not on an
  `sd` or `re` level, where a death sits out the round
  (`bot-objectives.md` section 3;
  `bots::tests::a_body_held_in_the_air_asks_for_a_kill`,
  `tests/bots.rs` `a_bot_held_in_mp_hurtgens_crease_kills_itself`).
- mp_ship's boat. VERIFIED (measured, seed 297 on master cb5f98d0, the
  trace, point traces and walks): an unstick spell ran bot 4 off the deck
  at (5209, -352, 375) at tick 912, and it landed at (5131, -505, 6) in
  a dish of patches 122 and 124 (floor z 5 to 29, sloping to the south)
  between x 4995 and 5350, y -440 and -590. The deck at 56 north of it
  is walled off from it: a run from the boat at yaw 90 slides west along
  the wall, with or without a jump, and a run south from the deck nodes
  at (5280, -409) or (5344, -447) stops at the wall too. Its way out is
  over the south rim to the floor at -64 under it. The flood never placed
  a node in it. The bot's plans started at the node nearest by
  `NavGraph::nearest`'s score, every one a deck node 28 up beyond the
  wall (a node 90 down scores 360 and up), so it walked into the wall,
  took spells and planned again for 143 bot-ticks before a spell took it
  over the rim. `nearest_walkable` walked the 8 nearest, all deck nodes,
  none arriving, and fell back to the nearest; and it ran only for the
  first plan after a fall, while the bot's next plans came from
  `nearest`. Walked to the nearest nodes below, flat, the run from seven
  spots in the boat reaches the floor at -64 from every one
  (`nav::tests::a_body_in_a_pit_off_the_graph_plans_from_the_floor_over_its_rim`,
  `tests/bots.rs` `a_bot_in_mp_ships_boat_drops_out_over_its_rim`, which
  holds the bot in the boat at (5340, -520) without `off_floor`). On
  master c7ca821a no seed in the 384 enters the boat.
- The ladder at (3369, -379). VERIFIED (measured, seed 313 with the two
  changes above, the traces of bots 4 and 5): bot 4 stood at the head,
  (3346.4, -387.9, 352.1), with its waypoint the foot (3391, -420, 56);
  bot 5 climbed to the head on the rungs at x 3369 and hung at
  (3370, -368, 354), 30 units from bot 4, from tick 3287. Each held the
  other's way: spells took bot 5 down the rungs and bot 4 along the
  deck, and they cleared at tick 3478 (17 stalled bot-ticks). Tried and
  backed out: a bot off the rungs on an edge that rises or falls more
  than a jump, with another player within 96 units flat on the rungs of
  it, climbing toward the bot's end, backs away to 64 units and waits
  (60 ticks at most, then not again for 200). Over the 384 seeds the
  stalled bot-ticks went from 466 to 1 043 (VERIFIED, measured, table
  below); the ladder's own spells from 4 to 4. INFERRED: the wait holds
  bots on mast and hold platforms where a climber sits on a ladder next
  to their path, and those places outweigh the head-on pin. Open.

VERIFIED (measured 2026-10-12c, same detector and settings, release
build; master is c7ca821a; places: the ladder at (3369, -379) at
x 3320-3420, y -430 to -330; the boat at x 4950-5400, y -620 to -420,
z -10 to 40; the hold ladder at x 4310-4350, y -110 to -80; x 3354 as
above; each place's stall spells):

| | seeds | stalled bot-ticks | spells | falls | ladder (3369, -379) | boat | hold ladder | x 3354 |
|---|---|---|---|---|---|---|---|---|
| master | 1-383 odd | 244 | 19 | 231 | 0 | 0 | 4 | 1 |
| kill and pit | 1-383 odd | 298 | 16 | 233 | 1 | 0 | 4 | 1 |
| and yield | 1-383 odd | 519 | 32 | 181 | 1 | 0 | 5 | 5 |
| master | 385-767 odd | 165 | 21 | 221 | 2 | 0 | 3 | 2 |
| kill and pit | 385-767 odd | 168 | 18 | 220 | 3 | 0 | 4 | 1 |
| and yield | 385-767 odd | 524 | 33 | 199 | 3 | 0 | 5 | 1 |

The "kill and pit" rows are what the branch keeps; the 57 bot-ticks over
the 384 seeds are within the runs' divergence, with no place moving.
`mp_hurtgen`, seeds 1-63 odd: 3 730 stalled bot-ticks on master (all of
seed 43), 0 with the kill alone and 0 with both. The "and yield" rows are
its last version, with the 200-tick rest; without the rest the first half
read the same 519.

Two more in the 2026-10-12d round. Bots are vcod's, so neither has a
retail measurement; the crease itself is retail's (the 10-12c entry
above).

- mp_hurtgen's crease, the way in. VERIFIED (measured, seed 43 bot 5 on
  master 7de7be23, the trace with the follower's waypoints): from the node
  (337.7, 2830.1, -24.7) the bot turned for (381.7, 2790.8, -53.6), left
  the ground at (379.3, 2806.7, -33.5) on terrain too steep to stand on
  and never landed again. The follower passed (381.7, 2790.8) in the air
  at (392.1, 2797.3, -55.1), 12 units off, and (428.2, 2783.2, -60.3) at
  (412, 2797.7, -64.6), 22 off, and the body slid on into the crease. A
  body dropped on either node stands there 20 ticks (VERIFIED, measured).
  The edge (428.2, 2783.2) to (480.1, 2833.2, -123.2) that the 10-12c
  report blamed is not at fault: its walk arrives from the node with the
  running start, from rest, from 8 units off in all eight directions and
  from 16 or 24 off in all but three of sixteen rest starts, and from rest
  at each tick of its own slide (VERIFIED, measured). From where the bot
  passed the first node, at rest or at 50 units/s, the body slides into the
  crease under the forward key. Two changes:
  - A node is passed only on the ground or on the rungs, never in the air
    (`nav::tests::a_node_is_not_passed_sliding_past_it_in_the_air`).
    Seed 43's bot holds (381.7, 2790.8), lands on it and walks on.
  - A walk that proves an edge and slides (the body off the ground against
    a plane too steep to stand on, `PlayerState::sliding`) arrives only if
    no spot it slid over drains into a crease: a body left there at rest
    for 40 ticks never lands and then stays within a unit for 10. VERIFIED
    (measured): (436.8, 2824.3, -97.2), (416.7, 2799, -60) and
    (392.1, 2797.3, -55.1) drain, the patch at (428.2, 2783.2, -59) does
    not; the walk from (385.2, 2831.8, -22.2) to (385.6, 2785.7, -52.6)
    arrives over spots that drain and proves no edge
    (`nav::tests::a_slide_over_ground_that_drains_into_a_crease_does_not_hold`).
    The rule changes 1 to 75 edges on nine of the twelve stock maps and
    none of the spawn census (`nav_build`: `mp_hurtgen` 207 996 to
    207 921 edges, 190 / 193 spawns either way). Tried first: a slide proved again with
    its running start turned 45 (then also 90) degrees either way, failing
    on a turned run that ends held in the air. It rearranged the slope's
    nodes and sent bot 5 into the crease off its spawn route on five seeds
    and bot 3 on one (6 deaths there over 96 seeds, against 1 on master);
    its first version, failing on any turned run that missed,
    cut 2 of `mp_chateau`'s spawns off the main component.
  VERIFIED (measured, `ship_stalls`, `mp_hurtgen` dm, 6 bots, 4000 ticks,
  seeds 1-191 odd, release): bots that died in the crease, master 1 (seed
  43, off its path), with the pass rule alone 1 (seed 39, its pace carried
  it past the held node), with both 1 (seed 67, an unstick spell's heading
  from (390.6, 2778.7, -52.2)). No seed stalls. A spell's heading down
  that slope is shunned since 10-12e (below).
- The ladder at (3369, -379), head-on. VERIFIED (measured, `tests/bots.rs`
  `bots_meeting_head_on_at_a_ladder_take_turns`: a climber placed at the
  foot (3375, -380, 56) with its goal the deck at (3268, -352, 352); once
  it is on the rungs above 330, a second bot placed where seed 313's stood,
  (3346.4, -387.9, 352.2), with its goal the floor at (3391, -420, 56)): on
  master the climber hung at (3369.2, -368.1, 352.2), fell back to the
  floor by tick 104 and stood on the deck at tick 353. Now a bot off the
  rungs, within 64 units flat of the ladder whose edge it is on or takes
  next and at its end's height, while another player on those rungs is
  within 64 units of that end's height and moving toward it or still,
  walks to the neighbour of its end farthest from the ladder (at least 32
  off) and comes to rest there; at most 200 ticks, 40 of them with the
  climber still, then not again for 200 (`Follower::ladder_yield`,
  `nav::tests::a_bot_at_a_ladders_end_waits_aside_for_a_climber`). The
  climber stands on the deck at tick 67 and the other reaches the floor at
  190. Tried: the same wait for any climber coming toward the end, however
  far down: the stalled bot-ticks over the 384 seeds went from 439 to
  1 670 and the spells at the ladder at x 3808 from 0 to 20. Bots on the
  platform at 616 waited a whole climb, then another, on mp_ship's busy
  ladders. Since 2026-10-13d the wait takes the ladder box nearest both
  ends (it took the first within 96 of them, at the crow's nest the
  ladder under the deck at its foot), at the head of a ladder taller
  than a fall that kills (480, `bg_fallDamageMaxHeight`) waits for a
  climber from two bodies' height (140) under it, and where no
  neighbour stands 32 clear of the rungs walks to the node two steps
  off on its floor farthest from them; and a bot backing down such a
  ladder with a climber coming up within 140 under it climbs back to
  the head (`Follower::head_on_yield`). Since 2026-10-13f, at such a
  head, a wait under way lasts while another body off the rungs stands or
  bobs between the spot and the rungs (nearer them, flat, than the spot,
  within a step of the head's height), a climber within a jump (39) under
  the head counts whichever way it moves (the bob under the plate), and
  the spot stands a body's width (30) clear of every neighbour of the
  head, three steps off where two are not. Since 2026-10-18 a bot already
  past the end toward the rungs (flat, by more than a half-width, 15:
  backing down over the lip) walks back onto the end before it heads for
  the spot
  (`nav::tests::a_yield_from_over_the_lip_steps_back_onto_the_head_first`).
  Since 2026-10-18c, at a head whose fall does not kill, a body pressed
  against the waiter (flat within two half-widths and 2) holds no wait:
  it stepped off and is on its way out through the spot
  (`nav::tests::a_climber_pressed_on_the_waiter_ends_the_wait`). And a
  creep off the head of a ladder whose fall kills faces the rungs square
  and strafes to their middle instead of backing at the foot
  (`nav::Follower::back_face`, `BotView::back_face`).
  Below.

VERIFIED (measured 2026-10-12d, same detector, `mp_ship` dm, 6 bots, 4000
ticks, release; master 7de7be23; `stalled` and `spells` over seeds 1-767
odd; the ladder (3369, -379) as above; `Server::test_seed_rng` stores
`seed | 1`, so seeds 2k and 2k+1 replay the same run, and a rerun of one
binary read the same totals):

| | stalled bot-ticks | spells | falls | ladder (3369, -379) |
|---|---|---|---|---|
| master | 466 | 34 | 453 | 4 |
| pass rule | 439 | 35 | 420 | 5 |
| and wait, any climber | 1 670 | 85 | 393 | 9 |
| and wait, near the end | 733 | 56 | 419 | 7 |
| and drain rule | 733 | 56 | 419 | 7 |

The kept row is the last. The 294 bot-ticks it adds over the pass rule
come from 10 seeds whose runs part after a wait of 10 to 29 ticks
(VERIFIED, measured with the waits logged); none of their stalls has a
bot within 100 units (seed 239's 90 at (2293, 588, -44), 1 259 from the
nearest; seed 647's on the ladder (3369, -379) with none within 450). No
stall at that ladder on the kept row has another bot within 100 units,
against seed 313's head-on pair (54 units) on master. `mp_depot`,
seeds 1-191 odd: 516 stalled bot-ticks on master, 947 with all three, in
42 and 43 spells, 35 and 33 of them at the corner trap at
(-433, 1129, -26) off the edge to (-484, 1051, -32), which holds a bot
longer on some runs (seed 187's last 229 ticks); fixed in 10-12e (below).

The 2026-10-12e round, with the same detector. Bisected first with each
of the three 10-12d changes switched off in turn (VERIFIED, measured,
release, `mp_depot` seeds 1-191 odd, `mp_ship` 1-767 odd):

| | `mp_depot` stalled | spells | `mp_ship` stalled | spells |
|---|---|---|---|---|
| all three (master e2228f97) | 947 | 43 | 733 | 56 |
| without the ladder wait | 943 | 42 | 439 | 35 |
| without the pass rule | 498 | 26 | | |
| without the drain rule | 1 047 | 47 | | |
| none of them | 516 | 42 | | |

So the pass rule raised `mp_depot` (longer holds at the corner trap) and
the ladder wait raised `mp_ship` (all of it on ten seeds, every one 0
without the wait; VERIFIED, the table and the per-seed totals). Four
fixes followed, each traced tick by tick:

- mp_depot's corner trap at (-433, 1129, -26). VERIFIED (measured, seed 3
  bot 3, and a scratch dump of the brushes): the node (-417, 1130, -32) is
  the foot of the ladder at (-427..-406, 1111..1114, 1..135) up to the
  roof at 133, and the graph held edges from it to (-484, 1051, -32) and
  (-347, 1058, -32), both on its own floor south of the ladder. South is
  the bracket (-459..-448, 1106..1127, 12..23) under the slab
  (-464..-368, 741..1108, 13..27), too low to stand under. The walk that
  proved the edge pressed into the rungs, climbed them to 135, crossed the
  roof and dropped 142 back to the floor beside the target, and arrived.
  A bot heading there touches the rungs for a tick, steps onto the rail at
  (-449..-445, z -26) and pins against the bracket. Now a walk to a floor
  that holds the rungs more than a jump over the higher of its start and
  that floor proves nothing (`walk_with`'s `rung_top`,
  `nav::tests::a_level_walk_over_a_ladders_top_proves_no_edge`). On
  `nav_build` the rule drops 2 edges on `mp_depot` (79 777 to 79 775) and
  2 on `mp_ship` (96 475 to 96 473), none elsewhere, and the spawn census
  does not move. With it the trap's spells over the 96 seeds went from 35
  to 0.
- The drain test's cmd splits. VERIFIED (retail, `client-probes/probe_wedge`
  on `cod_lnxded` 1.1d with a `--probe-fall` client, its trial spots
  set by hand to the three spots the walk from (385.2, 2831.8, -22.2)
  slides over under each of vcod's same-plane tests, 2026-10-18): a body
  left at rest at (385.42, 2813.30, -29.90) comes to rest in the air at
  (412.85, 2797.97, -65.12) and stays there; one at (385.52, 2813.39,
  -29.87) slides into the crease at (436.76, 2824.27, -97.24); both are
  held off the ground. Ones at (383.21, 2802.26, -42.12) and (383.39,
  2802.37, -42.10) rest at (391.9, 2794.5, -58.0), and ones near (382.8,
  2795.3, -50.6) at (385.1, 2792.8, -55.4). VERIFIED (measured): ours under
  one 50 ms cmd a tick lets go of the first spot after `PM_SlideMove`'s
  0.999 (cod11-mantle.md, "The slide's same-plane test") and of the
  second before it, and holds each under one of a client's 16/17 ms
  splits; a few tenths of a unit or a cmd split decide it on that slope.
  The drain test (`nav::drains`) asks the bot's 50 ms cmds and the three
  phases of a 60 Hz client's and takes a hold under any. It refuses 18
  more of mp_hurtgen's 209 877 edges (`nav_build`, 1.1), moves at most 12
  on four other stock maps, and no spawn census.
- mp_hurtgen's crease, an unstick heading. VERIFIED (measured, seed 67
  bot 5): a spell picked off the ground at (340, 2756, -78) ran the bot
  north-east over (390.6, 2778.7, -52.2) and (411.7, 2786, -57.7) onto the
  slope and into the crease at (436.8, 2824.3, -97.2). The drop probe felt
  floors 16 units apart, each within a jump of the last: the slope falls
  36 in the 64-unit look, on planes with a normal z of 0.46 to 0.68. Now a
  floor sample on a plane too steep to stand on (normal z under 0.7) whose
  spot drains into a crease (the drain rule's 40-tick settle, 10 held)
  reads as a drop that hurts (`nav::drop_ahead`). The settle costs 1.7 ms
  where it drains and 9 us where it lands, so the verdict is kept per
  8-unit cell for the level (`nav::Creases`, on `Server`). VERIFIED
  (`nav::tests::a_slope_draining_into_a_crease_reads_as_a_drop_that_hurts`):
  north-east from (411.7, 2786, -57.7) reads infinite, west reads none.
- mp_ship seed 693, the head (3298, -380, 352) cut past. VERIFIED
  (measured, bot 3): at (3365.4, -324.2, 352.1) the bot was 57 flat from
  the ladder's foot (3370, -380, 56), nearer than the head the path backs
  down from (72), so `past` skipped the head and the bot pinned on the wall
  at y -333 over the foot until a spell (53 stalled bot-ticks). The pass
  no longer cuts past a node whose next node lies more than a jump below
  (`nav::tests::a_drops_top_is_not_cut_past`). Seed 693 stalls 0.
- mp_ship's bow gutters. VERIFIED (measured, seed 577 bot 0, once the cut
  above changed its run): a bot that slid off the roof at
  (2150, 268, 323) landed in the gutter at y 270-283, z 312, between the
  roof's clip (normal z 0.6) and the ship's side. Its nodes
  (1920, 281, 312) and (1955, 281, 312) have no edge out, a component of
  one each; the drop north is 376 to the dock, and the bot, which keeps
  off a fatal fall, bounced on the roof's foot for 373 ticks. Bodies
  pushed every way from (1955, 281, 312) for 200 ticks either fall off the
  ship or stay in the gutter (VERIFIED, measured, 16 headings with and
  without jumps). `mp_ship` has five nodes with no edge out: the two
  gutters at y -152 and 281 and (4058, -195, 800). A bot whose nearest
  node has no edge out for 200 ticks asks for a kill, as in a crease, but
  not in a one-life round (`NavGraph::dead_end`, `BotView::stranded`,
  `bots::tests::a_body_stranded_off_the_graph_asks_for_a_kill`).

VERIFIED (measured 2026-10-12e, same detector and settings, release):

| | seeds | stalled bot-ticks | spells | deaths |
|---|---|---|---|---|
| `mp_depot` master e2228f97 | 1-191 odd | 947 | 43 | 8 |
| `mp_depot` with the four | 1-191 odd | 158 | 9 | 9 |
| `mp_ship` master e2228f97 | 1-767 odd | 733 | 56 | 332 |
| `mp_ship` with the four | 1-767 odd | 304 | 27 | 254 |
| `mp_hurtgen` master e2228f97 | 1-191 odd | 0 | 0 | 6 |
| `mp_hurtgen` with the four | 1-191 odd | 0 | 0 | 4 |

The pre-10-12d numbers were 516 (`mp_depot`) and 466 (`mp_ship`).
`mp_ship`'s spells at the ladder (3369, -379) and the head west of it
(x 3200-3500, y -420 to -300) went from 11 to 4. `mp_hurtgen`'s deaths
left are all at (5750-6030, -190 to 74, 128) and on master too; the crease
death of seed 67 and the off-path one of seed 159 at (3742, -317, -51)
are gone. `mp_depot`'s deaths sit at x -2470 to -2910, y -920 to -1450,
on master as well (7 there before 10-12d).

The 2026-10-12h round took the three spots 10-12e left, each traced tick
by tick with the same detector:

- `mp_hurtgen`'s deaths at (5750-6030, -190 to 74, 128): no fall. VERIFIED
  (measured, seed 57 bot 2): the path's edge (5856, -93) to (5904, -46)
  clips the corner (5888, -64) of the minefield `*2`, an L of axial
  brushes; the touch pass's brush contact fires `_minefields.gsc`'s click,
  and its `self istouching(trigger)` half a second later read the
  submodel's box, which the bot was still inside, 196 units on. Retail's
  `isTouching` runs `trap_EntityContact` against the brushes like the touch
  pass (`cod11-gsc-object-model.md` 22.2), and vcod's does too now
  (`trigger::entity_contact`). All four deaths of the 96 seeds go.
- `mp_depot`'s deaths at x -2470 to -2910, y -920 to -1450: falls off the
  roof at z 508 to the yard at -24, eight of the nine at one ladder.
  VERIFIED (measured, seeds 75, 85, 97, 119 and the scratch dumps): the
  ladder at x -2750 (y -1229 to -1205, z 336 to 504) is a 1-unit plate,
  its head (-2720, -1217, 524) on the roof east of it and its foot
  (-2766, -1217, 326) west. A back down from the head creeps over the
  plate's top at 0.7 units a tick, and ten ticks of that read as stuck
  inside the 15-unit stall circle; the unstick spell's heading ran off
  the roof, because a waypoint more than a jump below licensed any drop.
  Now a creeping body is stuck only inside 2 units in ten ticks
  (`bots::tests::a_creep_is_not_stuck_until_it_stops`), and a path that
  drops licenses no drop that hurts (past `FALL_HURTS`) deeper than its
  own plus a jump (`bots::tests::a_path_down_licenses_no_deeper_fall`).
  The creep then grabs the rungs at tick 24 and climbs down. The second
  rule is most of the drop in `mp_ship`'s falls below.
- `mp_ship`'s ladder stall at (4334, -93): the hold ladder (plate y -78 to
  -77, z 56 to 615), its head (4332, -87, 615) on a lip across the face.
  VERIFIED (measured, seed 309 bot 1, five-start forcing runs): on the
  rungs past 615 the head's height reads below, and the bot backed off the
  face the way it backs down, which moved the body 0.6 units away; it left
  the rungs at y -93.7, short of the lip (whose ground reaches y -92),
  fell back onto the rungs, and the stall circle sent it on a spell. Now a
  head a step below, across the face by less than a body's half-width, is
  climbed over (`bots::tests::a_head_across_the_face_is_climbed_over`;
  `tests/bots.rs` `a_bot_climbs_over_mp_ships_hold_ladder_top`: from five
  starts along the foot master stands on the deck at tick 311 or 312, now
  at 214 to 216). Tried first: any head across the face. Two bots side by side
  over the top of the ladder at x 3792, whose head is 25 across, stood
  pinned there on seed 193 (195 bot-ticks), where the back off had kept
  them apart.

Tried and dropped (VERIFIED, measured over the 384 `mp_ship` seeds):
re-proving a backed drop at the body's pace by a creep, from where it
passes the node and from a tick on. It caught the turn onto `mp_depot`'s
creep-only ledge drop at (-2750, -1244, 508) to (-2749, -1281, 326) on
seed 183, but rejected ladder back downs on `mp_ship` approached at a
run, and the replans doubled the spells (18 seeds to 37). Left: that
drop and its mirror (-2745, -1186, 508) to (-2755, -1150, 326) land only
creeping, and a bot that comes onto them off a turn is still faster at
the lip than the walk that proved them (VERIFIED, seed 123: 155 units/s
backing at -127 to the walk's 133 after its first tick, 81 at the lip to
its 14).

VERIFIED (measured 2026-10-12h, same detector and settings, release;
master ce5fdb33):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_depot` master | 1-191 odd | 158 | 9 | 18 | 9 |
| `mp_depot` with the three | 1-191 odd | 161 | 10 | 4 | 1 |
| `mp_ship` master | 1-767 odd | 304 | 27 | 414 | 254 |
| `mp_ship` with the three | 1-767 odd | 296 | 33 | 74 | 235 |
| `mp_hurtgen` master | 1-191 odd | 0 | 0 | 12 | 4 |
| `mp_hurtgen` with the three | 1-191 odd | 0 | 0 | 11 | 0 |

`mp_depot`'s extra 3 bot-ticks are one spell on seed 69 at
(-2232, 238, -24), in the corner by the documents' step at
(-2250 to -2325, 74 to 240) that holds every other `mp_depot` spell on
both rows; that seed's run parts from master's at an earlier creep and
reads 0 with the stall circle at 15. Its one death left is seed 183's,
the ledge drop above. `mp_ship`'s spells at the hold ladder went from 4
to 1 (seed 467, a climb up and back down after a replan).

The 2026-10-12i round took the four left after 10-12h:

- `mp_depot`'s ledge drop at (-2750, -1244, 508) to (-2749, -1281, 326)
  and its mirror (-2745, -1186, 508) to (-2755, -1150, 326). VERIFIED
  (measured, seed 183 bot 1, a scratch walk from the node at each pace):
  the forward run and the back down from the node fall, the creep
  arrives from the graph's running start and from rest, and falls with
  the body moving 150 units/s across the way (west, off a turn). The bot
  passed the node at (-2742.7, -1246.2) running west at 148 and backed off
  the lip at 6 to 8 units a tick. The walk's first tick (155 to 133) was
  not the difference: the bot backs at -127 while its run across decays
  only by friction, so its speed tops the back key's 133. Now a creep
  that misses off a turn, at `nav::TURN_PACE` (150) either side, and
  arrives from rest, holds its top while the body is faster than
  `nav::CREEP_TOP_PACE` (100) (`Follower::creeps_off`,
  `nav::tests::a_creeps_top_is_passed_only_slow`). Tried first, each
  measured: re-proving the creep at the body's pace every tick (seed 45
  still died there: the walk read as arriving by a floor 3 under the
  ledge at (-2766, -1320, 323), which the bot's fall, 8 units further
  out, missed); holding the top of every creep that misses off a turn
  until the body was under 100 (`mp_ship`'s stalled bot-ticks over seeds
  1-767 odd went from 296 to 493 with the crest hold below and 496
  without it: bots slowed onto the lip node (5402, 321, 344) from 8
  above it and bounced off the stair below), and every creep top to rest
  was worse (814 over the first 97 seeds); and
  dropping such creeps from the graph build (mp_ship lost 29 nodes and
  two spawns from its main component, the flood's own creeps among them;
  with the flood's left alone `mp_depot`'s two edges stayed, which the
  flood made).
- `mp_depot`'s roof peak (-2660, -1216, 552). VERIFIED (measured, seed 45
  on master ce5fdb33, bot 4, and scratch runs): the peak is flat from x
  -2679 to -2643, and a run east off it flies 67 units at 190 units/s
  before it lands at 508 on the flat inside the roof's edge at x -2570;
  the bot came over at 220 units/s (a `moveSpeedScale` of 1.16) and
  flew past the edge to the yard. The graph's walks and `edge_holds` run
  at 1, where the run lands. Now a body faster than the run (190) on a
  walk down off a crest, more than a step and no more than a jump, takes
  the walk at its pace and its weapon's scale (`Follower::move_scale`,
  `Sim::scaled`); if that falls the node is held, passed only from rest,
  and avoided if the same walk from rest falls too (`Follower::flings`,
  `nav::tests::a_run_over_a_crest_too_fast_for_the_weapon_is_held_at_the_top`;
  from a standstill 6 short of the peak a 1.16 weapon is back at 220 by
  the crest). Tried first, each measured over `mp_ship` seeds 1-383 odd
  beside the round's other changes of the time: the weapon's scale in
  `edge_holds` for every miss (deaths 117 to 164), and holding any edge
  that missed at a pace over the run (stalled bot-ticks 130 to 418); a
  hold on the level or uphill also rerouted
  `mp_hurtgen`'s bots past (292, 2785, -52) onto (338, 2830, -25) to
  (382, 2791, -54), whose walk arrives at a slow pace while the bot slides
  into the crease (seed 19); and a hold on a drop run off sent bots off
  `mp_ship`'s (3648, -317, 618) for the ledge at 400 from rest, and they
  fell to the hold.
- Two bodies at the top of `mp_ship`'s ladder at x 3792 (y 53-77, rungs on
  its east face to 623). VERIFIED (measured, seed 193 on master 416a1522
  with any head across the face climbed over, both bots' cmds, and a
  capsule trace west from x 3800.125 at z 623.125 every unit of y): the
  hatch over the ladder's top passes a body west only with its centre at
  y 58 to 72; anywhere else the trace stops at x 3800.125 on a wall at
  3785. The two bots climbed abreast at y 46.2 and 76.3, 30.1 apart, came
  off the rungs onto the plate's top at x 3800.1, z 623.1, and each
  steered for a waypoint across the other (y 64 and y 35): bot 0 had to
  move 11.7 north into the lane and bot 1 4.4 south, and each was pressed
  by its key into the other's capsule, 0.1 units off, with velocity 0
  from then on. Not a pmove fault: the lane holds one body. Master has a
  two-body stall there too (seed 435, 52 bot-ticks: one bot at
  (3807, 65, 623) bound down the rungs, another 30 away). Now of two
  climbers abreast on the rungs (within two
  half-widths flat and a body's height in z, the other still climbing),
  the lower, or of two level the later in (y, x), climbs down until the
  other is off (`Follower::abreast_yield`,
  `nav::tests::of_two_climbers_abreast_one_gives_way`). The hold ladder's
  climb-over limit of 10-12h stays.
- `isTouching`'s capsule variant: a client's side now goes against a
  trigger's brushes as its capsule (`cod11-gsc-object-model.md` 22.2).

VERIFIED (measured 2026-10-12i, same detector and settings, release;
master 266d4c97, whose ladder change leaves all three maps' totals as on
ce5fdb33's branch above):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_depot` master | 1-191 odd | 161 | 10 | 4 | 1 |
| `mp_depot` with the four | 1-191 odd | 158 | 9 | 4 | 0 |
| `mp_ship` master | 1-767 odd | 296 | 33 | 74 | 235 |
| `mp_ship` with the four | 1-767 odd | 270 | 17 | 62 | 241 |
| `mp_hurtgen` master | 1-191 odd | 0 | 0 | 11 | 0 |
| `mp_hurtgen` with the four | 1-191 odd | 0 | 0 | 10 | 0 |

`mp_ship`'s 6 more deaths are in the hold under (3700, -390, 92), where
master has 126 of its 235 and the branch 136: bots that run off
(3648, -317, 618) for the ledge at 400 and land past it, on master too.
No stall on the branch is at the hatch over the ladder at x 3792.

The 2026-10-12j round made `pmove::ladder_move` retail's
(`cod11-mantle.md`, "Ladders"): the climb went from 32 units/s at a level
view to 59.4 and from about 53 to 95 at the bots' 45 degrees up, sideways
motion on the rungs bleeds only without a strafe key, and the probe box
starts 8 above the feet, so the rungs let go 8 under a ladder's top and a
body gets over only by the step-up. VERIFIED (measured, same detector and
settings, release, master d0fceeb6 against the branch at each stage):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_ship` master | 1-767 odd | 270 | 17 | 62 | 241 |
| `mp_ship` retail ladder, bots as before | 1-767 odd | 1 178 | 77 | 97 | 270 |
| `mp_ship` with the three below | 1-767 odd | 570 | 24 | 56 | 249 |
| `mp_depot` master | 1-191 odd | 158 | 9 | 4 | 0 |
| `mp_depot` retail ladder, bots as before | 1-191 odd | 35 | 3 | 4 | 1 |
| `mp_depot` with the three below | 1-191 odd | 84 | 2 | 5 | 3 |
| `mp_hurtgen`, all three builds | 1-191 odd | 0 | 0 | 10 | 0 |

- A climb in 50 ms frames bobs under a lip. VERIFIED (measured, `mp_ship`
  seed 49 bot 0, and the climb replayed alone frame by frame): at the mast
  hatch at x 3711 the rungs let go at z 989 under the plate at 992; the
  step-up from 975.9 reached the platform, but its slide at the wall
  glue's 500 units/s ran 25 units into the wall 9 past the lip and came
  back at -0.5, and `PM_StepSlideMove` reverted it. The bot fell back onto
  the rungs, climbed, fell again, and after 15 ticks backed off the hatch
  and fell to 823 (17 such falls there and at (1543, -160, 8), none on
  master). At 16.7 ms frames the same climb steps over; at the hold
  ladder's lip (6 deep) 11 ms frames do and 16.7 ms ones do not, and at the crow's nest
  no fixed frame length tried (8.3, 10, 11.6, 16.7 ms) gets over, where
  frames that vary get over within one to three tries, as the retail
  capture did on its second. Now a body on the rungs off the ground sends
  five, four, three or six cmds a tick by turns, and the graph's walks step
  the same way (`nav::tick_times`); a body on the ground, at a ladder's foot
  among them, still sends one. Tried first: three cmds a tick always on a
  ladder, without the strafe stop below (`mp_ship` 407 stalled bot-ticks;
  the hold ladder lost its rung), and with it five always on a ladder
  (460; 7 falls off `mp_depot`'s ladder foot at (-442, 1133)) and five off
  the ground only (200 on `mp_ship`, but the crow's nest lost its rung).
- The strafe toward the middle of the rungs stops 36 under the waypoint
  (`bots::STRAFE_STOP`). VERIFIED (measured, `mp_depot` seed 47 bot 5): a
  climber still sliding along the face reaches the step-up with its
  velocity along the face; the stepped and the plain slide get as far along
  it and the revert keeps the plain one, and it hung under the ladder at
  (-417, 1112)'s top, 12 under the floor at 130, until it fell.
- The graph's ladder heads moved with the climb: `mp_ship`'s mast top is
  now (3675, -95.1, 1147.1) on the spar beside the plate, from which the
  creep back down at a run is 40 units out before it has dropped the 8 the
  rungs need, so that ladder was up only until 2026-10-13 (below;
  `nav::tests::ships_ladders_are_climbed_both_ways`);
  the head at x 3354 moved to (3358.6, -384.2, 352.1).

Left: the branch's spells are mostly in the hold, on crates at x 2100 to
2500 and y -650 to -350 or 350 to 650 (10 spells, 2 on master) and at
(5400, -100, 300) (4), none of them on a ladder (2 spells start on the
rungs, 10 on master); routes change with the climb's pace and the hold's
crate jumps, a trap on master too, see more traffic. `mp_depot`'s two
falls off the ladder foot at (-442, 1133, 23) and three deaths in the
yard under the roof at (-2700, -1400) are the 10-12i ledge drop's.

The 2026-10-13 round took those, each traced tick by tick with the same
detector and walked again in scratch tests. VERIFIED (measured, same
detector and settings, release; master 5e8d7fe2, which reads as the
10-12j branch above, against this branch merged with it):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_ship` master | 1-767 odd | 570 | 24 | 56 | 249 |
| `mp_ship` with the seven below | 1-767 odd | 121 | 9 | 35 | 61 |
| `mp_depot` master | 1-191 odd | 84 | 2 | 5 | 3 |
| `mp_depot` with the seven | 1-191 odd | 33 | 1 | 3 | 0 |
| `mp_hurtgen` master | 1-191 odd | 0 | 0 | 10 | 0 |
| `mp_hurtgen` with the seven | 1-191 odd | 0 | 0 | 9 | 0 |

By place, master then the branch: spells on `mp_ship`'s hold crates
(x 2100 to 2500, y beyond ±350) 10 / 0; deaths in the hold under
(3700, -390, 92) 177 / 0 and at the hold ladder's foot (4352, -94, 56)
21 / 0; `mp_depot`'s falls off the ladder foot at (-442, 1133) 2 / 0 and
deaths in the yard under the roof at x -2400 to -2900 2 / 0.

- The hold's pit, the graph. VERIFIED (measured, seed 199 bot 0 and
  scratch walks): the bots came along the deck from the north, passed the
  node (3647.9, -316.9, 618.5) and backed off for the ledge node
  (3685.5, -354.3, 400.1). That edge was proved toward the ledge column's
  centre, not the node, and no gait lands node to node (section 2, "The
  flood"); the flood now walks such a rest to the node and drops the edge
  when that fails. The way down left, from (3647, -352.9, 611.1), lands
  backing from the node and from 4 units north, east and west, and falls
  from 4 units south: it is risky and planned round (section 2, "The
  flood", `RISK_COST`).
- The hold's pit, the follower. A drop deeper than a jump that a run off
  misses is re-proved when the bot passes its node, by the bot's own back
  down (`Gait::Creep`, which is what `Bot::steer` sends: -127, the creep
  key inside 24 units, the creep's held heading) from where the body
  stands at its pace and weapon scale; if that falls the node is held and
  passed from rest, and if the walk from rest falls too the edge is
  planned round (`Follower::backs_short`,
  `nav::tests::a_backed_drop_is_proved_again_at_the_pace_the_bot_comes_onto_it`).
  Ladders included: VERIFIED (measured, seed 229 bot 2), the 21 deaths at
  the hold ladder's foot were bots that passed the deck node
  (4319, -133, 600) at a run, backed for the ladder's foot at 56 and went
  over the lip with their back to the rungs. Tried first: the Backward
  gait away from ladders only (`mp_ship` deaths 249 to 114, the ladder
  foot's 21 to 36); holding every such node until rest away from ladders
  (stalled bot-ticks 121 to 240, deaths 61 to 56); and holding from 48
  units out where the walk at the pace fell (662 stalled bot-ticks, bots
  held on the crow's nest's perched head). Bot and walk part on such
  lips by a few units a tick (seed 231 bot 4 passed (3647, -353, 611)
  nearly at rest, its walk from there landed, and the bot left the lip at
  106 units/s and flew past the ledge), so only the risk cost settles the
  pit.
- The hold crates. VERIFIED (measured, seed 709 bot 4): the hull floor
  node (2247.1, 511.2, -47.9) leads to the beam top (2207.9, 511.5,
  -31.9) by a walk with a running start; from rest, and from where the
  bot comes onto it, the body bobs in a crease at (2234, 494, -42). The
  bot's 10-tick stall circle sent it on a spell, the spell ran it more
  than 4 grid steps off the path, and the follower dropped the path
  without avoiding anything, so the next plan took the same edge: 12 s
  and more of crease, spell, crease. Now an unstick spell plans round the
  edge it began on (`Follower::stuck`, `Bot::take_stuck`,
  `nav::tests::a_spell_plans_round_the_edge_it_began_on`). With it, the
  strafe change below and the pit's first two `mp_ship` read 8 stalled
  bot-ticks;
  `mp_depot` went 73 to 114 then, all of it the corner by the documents'
  step and a roof trough at (-1535, 2100, 226) (seed 31), both reached
  by other routes, and back to 33 with the rest of the round.
- A jump's foot is not cut past. VERIFIED (measured, seed 203 bot 3, with
  the drops planned round): a bot dropped from the mast's platform at
  (3564, -127, 896) for (3520, -162, 760), landed 38 above it on the
  ledge at (3468, -210, 798), and the pass cut to the node after, the top
  of a jump from 760 back up to (3492, -192, 798); the bot jumped where
  it stood and off the ledge to 352. A node whose edge on is a jump is
  now never cut to (the pass's `past`); 76 deaths at x 3400, y -200,
  z 300 to 500 on that build, 15 after.
- `mp_depot`'s ladder foot. VERIFIED (measured, seed 81 bot 4, and the
  climb replayed alone): the bot grabbed the rungs of the ladder at
  (-417, 1112) running west at about 40 units/s and drifted west at
  31 units/s all the way up, off the rungs' edge at x -433, under the
  lip, and fell onto the bracket at (-442, 1128, 23), where it stood
  pinned until a spell walked it off the bracket's north edge (the
  "fall"). The strafe toward the rungs' middle was held the whole climb:
  under a full-rate climb (95 up) `ladder_move`'s accelerate adds next to
  nothing across, and the bleed runs only with no strafe key (retail's,
  `cod11-mantle.md` "Ladders"), so the key kept the grab's pace. From
  rest the same strafe moves the body 8 units/s toward the middle. Now
  the key comes up while the body slides away from the middle
  (`bots::tests::a_climber_sliding_away_from_the_middle_lets_the_strafe_go`):
  the replay holds x -415.6 to the top.
- `mp_depot`'s yard deaths: none on the branch; seeds 47 and 97 run other
  routes now, and no change here touched the roof's ledge drops.
- The mast. VERIFIED (measured, scratch walks): from the head
  (3675, -95.1, 1147.1) the creep from a running start misses the rungs,
  the creep from rest lands at the foot (3675, -103, 896.1), and so does
  one at up to 100 units/s along the spar (±x); at 150 it misses. `rung` takes
  either creep as down, the follower holds the head of a down edge that
  only the creep from rest proved until the body is under 100 units/s
  (`Follower::creeps_off`), and a bot sent from the head to the platform
  climbs down (`nav::tests::ships_ladders_are_climbed_both_ways`,
  `nav::tests::a_creep_down_proved_only_from_rest_is_passed_only_slow`,
  `tests/bots.rs` `a_bot_climbs_down_mp_ships_mast_from_the_spar`). The
  pace rule changes nothing in pmove's climb.

Left after 2026-10-13: `mp_ship`'s crow's nest. Deaths at x 4900, y 0
to 100 went from 6 on master to 21: bots that back from the platform's
exits (4963.9, 35.5 or 92.5, 1219.1) through the hatch for the deck at
692 slide along the hatch's rim slower than the walk that proved the
edge, and the stall circle sends them on a spell as they leave the lip,
facing away from the rungs (VERIFIED, seed 291 bot 4). A stall circle of
2 units while backing to a waypoint far below took that to 18 and the
stalled bot-ticks to 145 (not kept). Also left: 15 deaths at x 3400,
y -200, z 400 to 500 (bots off the mast platform's ledges), and the
spells at (5350 to 5460, -160 to 0, 352) and around (7232, 293), as on
master.

The 2026-10-13b round took the crow's nest. VERIFIED (measured, same
detector and settings, release; master 76073d5c against the branch):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_ship` master | 1-767 odd | 121 | 9 | 35 | 61 |
| `mp_ship` with the two below | 1-767 odd | 0 | 0 | 17 | 34 |
| `mp_depot` master | 1-191 odd | 33 | 1 | 3 | 0 |
| `mp_depot` with the two | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_hurtgen` master | 1-191 odd | 0 | 0 | 9 | 0 |
| `mp_hurtgen` with the two | 1-191 odd | 0 | 0 | 8 | 0 |

Deaths at x 4850 to 5050, y -20 to 140 (the crow's nest and the deck
under its hatch) went from 20 to 3. The geometry, VERIFIED (measured,
the graph's nodes and scratch walks): the head (4963.9, 64, 1219.1)
stands on the ladder's plate in the hatch, with exits at y 35.5 and
92.5 on the rim at 1219 and the top floor at 1254 west of it; a body
on the rim west of x 4954.1 pins on the top's face, and one at y 57.9
to 72.9 there drops through the hatch.

- The back down of the brief. VERIFIED (measured, seed 291 bot 4): the
  bot came to the exit at y 92.5 along the rim from the east, backed
  west for the deck node (4929, 68, 692), pinned on the top's face at
  x 4954.1 and slid south along it at 2.5 falling to 0.5 units a tick;
  the graph's walk from the exit node, its running start aimed at the
  target, slid at 4. The bot left the rim at y 72.9 and the spell fired
  3 ticks into the fall. A first change made a back down to a waypoint
  more than 64 below not stuck while it closed on the waypoint; the
  bot then slid to y 70.5, fell and grabbed the rungs at 1204. That
  alone took `mp_ship`'s deaths from 61 to 58, the crow's nest's from
  20 to 18, and the stalled bot-ticks from 121 to 138 (seed 321).
- The 20 crow's nest deaths left with that change, VERIFIED (measured,
  each traced over the 60 ticks before it):
  - A climb that bobbed under the head's lip (6; seeds 331 bot 3, 415,
    509, 629, 661, 703): the rungs let go at 1214, the body fell back
    onto them at 1199 to 1206 and stepped onto the head 10 ticks later;
    the spell fired on the head or in the bob, and from the plate every
    heading is the hatch. On 415 and 661 the spell left a plan from the
    head to the foot that the bot backed toward from the rim beside the
    plate, facing along the rim, not the rungs, and fell past them.
  - A turn onto the head off the north exit (3; seeds 125, 417, 545):
    the bot slowed to 1 to 3 units a tick at the turn, and the spell's
    heading took it down the hatch.
  - A plan that began a floor up (8; seeds 247, 383, 399, 451, 521, 551,
    599, 697): the bot reached the top's node (4928.5, 60.8, 1254.1),
    its roam ended, and while the next search ran over several ticks
    (no waypoint) it wandered east off the top and dropped 35 onto the
    rim. The plan had started at the top's node, so its first waypoint
    stood 35 over the feet with no edge up from the rim; the bot steered
    for it, pinned on the top's face at x 4954.1 and slid along it into
    the hatch.
  - Three more: a back down toward the foot from the rim beside the
    plate (seed 331 bot 4), a spell that began on the top floor west of
    the hatch at (4841, 70, 1203) and ran east into it (seed 251), and a
    bot that climbed the rungs and fell back down them (seed 389).
- Now a body that closed on its waypoint by 2 units in the ten ticks,
  across plus height, the measure the follower's own give-up runs on,
  is not stuck, whichever way it goes (`Bot::closes_in`,
  `bots::tests::a_back_down_sliding_along_a_rim_is_not_stuck`); and a
  plan whose first node is more than a step over the feet, its start
  not proved by a walk (`NavGraph::nearest`, not `nearest_walkable` or
  a ladder's end), is planned again from a node a walk reaches
  (`Follower::proved_start`,
  `nav::tests::a_plan_that_starts_a_floor_up_is_planned_again`). A
  stall window restarted when a climber steps off the rungs was written
  and dropped unmeasured: the general rule covers the bob.

`mp_ship`'s mast platform ledges (x 3400, y -200, z 400 to 500) keep
their 15 deaths on both builds; the rest moved with the routes. The
stalls master had (`mp_ship` seeds 61, 321, 517, 639, 729; `mp_depot`
seed 93, at the documents' step) are gone, and no new one appears.

The 2026-10-13c round took the mast platform's ledges. VERIFIED
(measured, same detector and settings, release; master bcf7d489 against
the branch; `mp_ship` also over a second set of seeds):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_ship` master | 1-767 odd | 0 | 0 | 17 | 34 |
| `mp_ship` with the three below | 1-767 odd | 3 | 2 | 9 | 16 |
| `mp_ship` master | 769-1535 odd | 0 | 0 | 15 | 38 |
| `mp_ship` with the three | 769-1535 odd | 0 | 0 | 10 | 5 |
| `mp_depot` master | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_depot` with the three | 1-191 odd | 30 | 2 | 1 | 0 |
| `mp_hurtgen` master | 1-191 odd | 0 | 0 | 8 | 0 |
| `mp_hurtgen` with the three | 1-191 odd | 0 | 0 | 3 | 0 |

Deaths at x 3350 to 3500, y -450 to -190 (the mast platform's ledges at
480 and 520 and the hold floor under them) went from 19 to 1 in seeds
1-767 and from 23 to 1 in seeds 769-1535.

- The 15 ledge deaths, VERIFIED (measured, each traced over the 70 ticks
  before it, seeds 23, 127, 203, 219, 235, 351, 441, 443, 513, 515, 623,
  639, 669, 741, 767): every one is the same edge. The bot climbed down
  the mast to its foot (3647, -96, 896), ran west for the platform's
  drop node (3564, -127, 896), passed it and ran off at 210 to 228
  units/s for the narrow ledge node (3520, -162, 760), flew over it and
  over the ledge at (3468, -210, 798), and landed on the ledge at 480 or
  520 (fall damage) or the floor at 56. The graph's forward walk from
  the node lands on the ledge (3525.5, -160.3, 760.1) at 190; the same
  run at 1.1, 1.16 and 1.2 times the pace falls from the node and from
  every pass point traced, where the back down
  (`Gait::Creep`) lands up to 1.16
  (`nav::tests::the_run_off_the_mast_platform_lands_only_at_the_graphs_pace`).
  The pass re-proved the run at the body's pace but at the graph's
  scale, so it held. Three bots died at (3400, -430, 56) by other ways.
- Two dropped variants, measured: re-proving a run off at the weapon's
  scale once per edge and backing down where only the creep landed
  took the ledge deaths to 1 and the total to 17, with 28 stalled
  bot-ticks; doing it in the pass, holding the body on the node when
  neither landed, moved 8 deaths onto the roof slopes south of the
  platform, where runs at 215 to 228 fly off the turns between nodes at
  (3514, -283, 623), (3496, -318, 611) and (3456, -289, 520). Pacing the
  keys fixes both places, and every other proof the bot runs after the
  graph: the graph's walks never ran faster than 190.
- The no-gait drop above, priced as risky: with the pace alone the 7 deaths
  at (3420, -400, 56) were all that edge (seeds 101, 125, 127, 317, 393,
  453, 613).
- With the pace and the price, `mp_ship` read 105 stalled bot-ticks over
  seeds 769-1535 and 43 over 1-767: seeds 997 and 1211 ran the same route, and their bot
  reached the shelf node (5397, -161, 396), finished its roam and, over
  three ticks without a waypoint, ran 17 units west at 162 units/s; the
  next plan began at the shelf's node 22 behind it, passed it, and backed
  the body toward the deck at 256 from there, up a slope it slid on for
  20 s (VERIFIED, seed 997 bot 2, traced). The wait on the last waypoint
  took that set to 9 stalled bot-ticks and its deaths from 20 to 7, and
  seeds 1-767 from 43 to 13 (seed 357's crow's nest stall, a spell that
  left the top in a plan's three-tick gap, is gone).
- A jump's tick keeps the whole key: pmove jumps and then runs the air
  move, whose wish takes no weapon scale, so a cut key shortened the jump
  against the graph's. With it, 13 to 3 and 9 to 0 stalled bot-ticks
  (the table's rows).
- The 30 `mp_depot` bot-ticks are one bot in seed 9 hopping a crate stack
  at (-2256, 149, 46) to (-2235, 164, 66), its jump to the top crate
  falling back for 12 s. The run parts from master's at tick 161, when
  the wait first holds the bot a tick, and master's run never goes there;
  pacing and the price alone (without the wait) also leave seed 9 at 0.
  Taken in 2026-10-13d (below): the jump's cooldown.
- Left on `mp_ship`, seeds 1-767: the crow's nest (deaths at x 4850 to
  5000, z 400 to 692: 7 against master's 4, spells off the head and back
  downs along the rim, the mechanisms of 2026-10-13b) and (4781, 87,
  400) (4, as on master).

The 2026-10-13d round took the follow-ups of 10-13b and 10-13c: the
`mp_depot` crate stack, the crow's nest and (4781, 87, 400). VERIFIED
(measured, same detector and settings, release; master e4ff0318 against
the branch):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_ship` master | 1-767 odd | 3 | 2 | 9 | 16 |
| `mp_ship` branch | 1-767 odd | 11 | 1 | 3 | 4 |
| `mp_ship` master | 769-1535 odd | 0 | 0 | 10 | 5 |
| `mp_ship` branch | 769-1535 odd | 35 | 3 | 1 | 2 |
| `mp_depot` master | 1-191 odd | 30 | 2 | 1 | 0 |
| `mp_depot` branch | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_hurtgen` master | 1-191 odd | 0 | 0 | 3 | 0 |
| `mp_hurtgen` branch | 1-191 odd | 0 | 0 | 1 | 0 |

Deaths at x 4700 to 5100 (the crow's nest, its spar and the decks under
them) went from 11 to 2 in seeds 1-767 and stayed at 2 in 769-1535. Each
change below comes from a death or stall traced tick by tick
(`TRACE=`, whose lines now end with the nearest other bot within 100
units):

- `mp_depot` seed 9: the jump's cooldown (section 2, "Jumps").
- (4781, 87, 400), seeds 245, 287 and 359: bots that ran off the crow's
  nest's top for the spar west of it. The drop from (4928.5, 60.8, 1254)
  to (4888.3, 64.6, 1211.6) had been taken out of the plans (the walk on
  failed at the body's pace, seed 245 at tick 298), the next plan had no
  way round it and took it, and a planned-round edge was passed with no
  check: from (4940.6, 48.3) at (-118, 120) units/s the run falls, at
  (-170, 0) or from rest it lands (scratch walks). Such an edge is now
  held: the body comes to rest on the node and takes it from there
  (`nav::tests::an_edge_with_no_way_round_is_taken_from_rest`).
- A climb that bobs under the crow's nest plate (seeds 553, 659): the
  rungs let go at 1214, the body rose to 1216.7 and fell back onto them
  at 1200, and the stall circle fired in the bob or on the plate; the
  spell's heading ran it down the hatch. Seed 81 bot 2 bobbed the same
  way under a spar at y 43.9, z 1145. A spell that starts within 10
  ticks of the rungs now climbs down on them and holds still off them,
  as does a body in the air just off them with no waypoint (seed 557: a
  plan's gap with the body over the plate)
  (`bots::tests::a_spell_just_off_the_rungs_holds_still`). Held still,
  bots went round the bob again and again (seed 143, 19 stalled
  bot-ticks), so a body on or just off the rungs within 24 units under a
  waypoint less than a jump over it is not stuck
  (`bots::tests::a_climb_bobbing_under_the_top_is_not_stuck`).
- The drop probe from the plate (4963.9, 64, 1219.1): west the sweep
  stops at the top's face 11 units off, before the first sample, and
  north-west the samples stand on the hatch's rim; it read no drop in any
  octant. The hatch is about 40 by 46 units (point traces down from 1300,
  x 4940-4980, y 41-86). With the samples half way between and at the
  sweep's end it reads 135, 180 and 225 as drops; north and south along
  the rim stay open (`nav::tests::the_drop_probe_feels_the_crows_nest_hatch`).
  A point trace under a capsule held by a rim too steep to stand on was
  tried and dropped: the test reads the same without it. Cost, measured
  over 2 000 `mp_ship` nodes, 8 octants, release, loaded machine: 61 us
  a body against 34 before (noisy); the drop reads rose from 1 868 to
  1 974 of 16 000 (with that check in).
- Bots that backed down the crow's nest ladder onto a climber (seeds 37,
  81 bot 4, 683; the climber 64 to 77 under them, `near` in the trace):
  the descender stood on the climber's head and slid off the rungs. And
  bots on the plate that started down with a climber 70 to 100 under them
  (seeds 545, 551 on an earlier build). The ladder's wait never fired
  there: it took the first ladder box within 96 units of both ends, the
  one under the deck at (4998-5022, 118-119, 576-692), which no climber was
  on; and the head's two exits stand 26 from the rungs, short of the 32
  the wait needs. Now the wait takes the nearest box, and where no
  neighbour is clear, the node two steps off on the end's floor farthest
  from the rungs, here the rim at (4992.6, 35.1) or (4992.6, 92.9), 29
  off (`nav::tests::the_crows_nest_head_has_a_spot_to_wait_at`). At the
  head of a ladder taller than a fall that kills (480,
  `bg_fallDamageMaxHeight`; on `mp_ship` only the crow's nest's 527) the
  wait looks two bodies' height (140) down, not 64
  (`nav::tests::a_bot_at_a_ladders_head_waits_for_a_climber_two_bodies_down`),
  and a body backing down such a ladder with a climber within 140 under
  it climbs back to the head (`Follower::head_on_yield`,
  `nav::tests::a_bot_backing_down_onto_a_climber_climbs_back_up`). On
  every ladder, both rules held two bots at the hold ladder (3369, -379)
  for 44 stalled bot-ticks (seed 513): off a ladder that short a body
  slides off a climber's head and lives. A bot sent aside after it had
  passed the head then headed for the foot from the rim and backed off
  it beside the plate (seeds 81, 1019, 1273); it goes back onto the head
  first now.
- A plan begun on the rungs under the plate starts at the head
  (`nearest_ladder_end`), and with no node before it the head was passed
  from the rungs 8 under it; the bot turned for the rim's exit and fell
  (seed 81 bot 4 on an earlier build). Such a first node over the feet is
  passed only on the ground
  (`nav::tests::a_plan_from_the_rungs_passes_its_head_only_on_the_ground`).
- The spar west of the top (seeds 421, 521): a bot that landed past the
  node (4888.3, 64.6, 1211.6) walked back up to it and on west, ended the
  ten ticks 4 units from where they began, and the spell's heading ran it
  off the spar's side. The circle is now the farthest the body went
  (`bots::tests::out_and_back_is_no_stall`).
- The perched head passed from 24 units (10-13b): not changed. A scratch
  sweep of bodies on the rim at 4 positions across and 7 along each side,
  at 0 to 180 units/s toward the head, passed the head only from
  x 4963.9 within 14 units of it, where the back down from the spot
  arrives; everywhere else `backs_short` holds the body.
- The stalls the branch adds are the waits: seed 861 (32 bot-ticks, two
  bots that turned back and waited by turns at the crow's nest ladder),
  seed 81 (11, a wait on the rim) and seed 1319 (3, a turn back and a
  wait). None ends in a death.
- Left: 4 deaths at the crow's nest over the 768 seeds. Three (seeds 81
  bot 2, 383 bot 1, 1273 bot 2) are climbers that stepped onto the plate
  with a waiting bot heading back from the rim's exit, and went west off
  the plate into the hatch: the wait ends when the climber leaves the
  rungs, not the plate (the climbers list holds bodies on a ladder only).
  One (seed 1019) is a back down from the rim at (4956.6, 44.4) after the
  climber left. The spar falls (4824, 64, 1200) to (4696, 98, 1140) are
  gone; the rail at y 125 to 130 north of the top (seed 743 on master)
  was not looked into. Taken in 2026-10-13f (below).

The 2026-10-13f round took the follow-ups of 10-13d: the crow's nest
deaths and turn-taking stalls, the rail north of its top and the drop
probe's cost. VERIFIED (measured, same detector and settings, release;
master dfe1d2da against the branch, each row adding to the one above;
master's rows match 10-13d's branch rows, its tip):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_ship` master | 1-767 odd | 11 | 1 | 3 | 4 |
| `mp_ship` wait to the head's end | 1-767 odd | 18 | 2 | 3 | 2 |
| `mp_ship` and the hatch drops priced | 1-767 odd | 0 | 0 | 2 | 1 |
| `mp_ship` and no roam onto a perch | 1-767 odd | 0 | 0 | 2 | 2 |
| `mp_ship` master | 769-1535 odd | 35 | 3 | 1 | 2 |
| `mp_ship` wait to the head's end | 769-1535 odd | 4 | 1 | 1 | 2 |
| `mp_ship` and the hatch drops priced | 769-1535 odd | 1 | 1 | 1 | 0 |
| `mp_ship` and no roam onto a perch | 769-1535 odd | 1 | 1 | 2 | 0 |
| `mp_depot` master | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_depot` all | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_hurtgen` master | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_hurtgen` all | 1-191 odd | 0 | 0 | 1 | 0 |

Master's 6 `mp_ship` deaths were 4 at the crow's nest (seeds 81, 383,
1273, 1019) and 2 off the mast platform at (3421, -419, 137) (249, 457);
the branch's 2 are both the mast platform's (379, 457), whose mechanism
this round did not touch. Every stalled bot-tick on master was at the
crow's nest; the one left is seed 861's bot 4 on its ladder. Each change
below comes from a run traced tick by tick:

- The wait at the head (seeds 81 bot 2, 383, 1273): the climber stepped
  off the rungs onto the plate (4963.9, 64 to 72, 1219.1), the waiting
  bot's wait ended that tick and it walked back from the spot
  (4986, 35) for the south exit (4963.9, 35.5); the climber, bound for
  the same exit, met its capsule (17 east and 25 south of it at seed
  81's t488) and slid west off the plate into the hatch. The plate
  holds a body only with its middle east of about x 4961 (VERIFIED,
  point traces 4 units apart: the capsule over the plate's x 4976 to
  4980). The wait now lasts while a body off the rungs stands between
  the spot and the rungs, so the climber gets to the exit and up onto
  the rail at 1254 first
  (`nav::tests::a_wait_at_the_crows_nest_lasts_until_the_climber_is_off_the_head`).
  With that alone two more ways in showed: seed 861's climber bobbed
  under the plate (the rungs let go at 1216.9, it fell back onto them
  moving down at 1206.9, neither on a ladder nor on the ground in
  between) and the wait, fed only bodies on rungs moving toward the
  head, ended in the bob; the bot walked back and stood on the exit
  while the climber stood on the plate, 30 ticks, until the climber's
  spell sent it back down. And the spot at (4992.6, 35.1) is 28.7 from
  the south exit: a body there touches one on the exit. Bodies in the
  air count now, a climber within a jump under such a head counts
  whichever way it moves, and the spot keeps 30 clear of the head's
  neighbours: the farthest from the rungs three steps off on the rim,
  (5021.2, 35.1) or (5021.2, 92.9), 51 off the ladder box and 57 off
  the near exit (`nav::tests::the_crows_nest_head_has_a_spot_to_wait_at`).
  That is the table's second row. Master's seed 1019 death was neither
  (VERIFIED, traced): a bot walking from the south exit for the head
  pinned on the top's face at x 4954.1, slid north along it off the rim
  and fell through the hatch at y 61.4, west of where the plate holds a
  body. It does not recur on any row, by the runs parting, not by a
  change aimed at it.
- The hatch drops: the edges from the exits back off the rim into the
  hatch for (4929, 68, 692) land only where the falling body grabs the
  rungs. Seed 383 on the second row: a bot that backed off the north
  exit grabbed them at y 62 and slid along them to y 49.6, off their
  side; on an intermediate build (seed 81) one pinned on the top's face
  at x 4954.1 left the rim at y 56.9, missed the rungs and fell to the
  deck at 503. Bots whose roam ended on the rail took those drops to come
  down, waited, backed out and tried again, which is where the stalls
  came from (INFERRED from the traces of seeds 81, 383 and 861). Priced
  as risky (section 2), plans come down the head and the rungs: the
  third row, with no crow's nest death or stall left over 768 seeds but
  seed 861's one bot-tick.
- The rail (seed 743 on master e4ff0318, traced on that build): a roam
  whose destination was the rail's corner (5023.8, 123.7, 1254.1) ran
  north onto it at 100 units/s; the roam ended there, the next plan came
  two ticks later from (5020.8, 119.2), turned west along the rail with
  the run still carrying north, and the body left the rail at y 128.6.
  The rails stand a body's middle 2 to 3 units in from the edge (the
  rail top's floor ends at y 110 and the node is at 123.7). Seed 1319
  on the second row ended a roam on the west rail (4928.5, 60.8) and ran
  off it south-west the same way. No roam picks a perch now (section 2,
  `nav::tests::a_roam_never_ends_over_a_fall_that_kills`): seed 1319 no
  longer falls, master's tip does not take seed 743 there, and no seed
  on the last row falls off a rail. The second death on its first set
  is the mast platform's (seed 379), where the runs parted.
- The probe (below): on master the server felt for drops in all eight
  octants for every bot standing on the ground every tick, and the brain
  reads them only to pick or keep a wander heading.
- Tried and dropped: back up the rungs on the foot's line
  (`ladder_middle`) when `head_on_yield` turns a bot back; the strafe
  stops 36 under the head, where seed 1273's bob slid the body off the
  rungs' side (y 59 to 55) on the second row, and the run read the same.
- Left: the mast platform's deaths at (3421, -419, 137); seed 1273's
  head-on climb back that bobbed under the plate and slid off the rungs'
  south side (gone once the hatch drops were priced, not fixed); and
  seed 1019's pass of the head from the top's face.

The 2026-10-15f round rechecked the open places of 10-12b to 10-13f
(the hold pit, the crow's nest ledges, the boat, the ladders at
(3369, -379) and (2150, -214), `mp_hurtgen`'s steep edge and seed 43,
`mp_depot`'s (-2400, -1400)). VERIFIED (measured, same detector and
settings, release; master 32eb3a4e against the branch):

| | seeds | stalled bot-ticks | spells | falls | deaths |
|---|---|---|---|---|---|
| `mp_ship` master | 1-767 odd | 0 | 0 | 2 | 2 |
| `mp_ship` branch | 1-767 odd | 0 | 0 | 1 | 0 |
| `mp_ship` master | 769-1535 odd | 1 | 1 | 2 | 0 |
| `mp_ship` branch | 769-1535 odd | 36 | 4 | 3 | 0 |
| `mp_ship` master | 1537-2303 odd | 5 | 1 | 1 | 2 |
| `mp_ship` branch | 1537-2303 odd | 12 | 2 | 2 | 0 |
| `mp_depot` master | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_depot` branch | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_hurtgen` master | 1-191 odd | 0 | 0 | 1 | 0 |
| `mp_hurtgen` branch | 1-191 odd | 0 | 0 | 2 | 0 |

On master none of those places stalls or kills over the 1 152 `mp_ship`
seeds: the hold pit, the crow's nest, the boat (5150-5350, -440 to
-570), both ladders, `mp_hurtgen` seed 43 and `mp_depot`'s yard all read
0. `mp_hurtgen`'s edge (428, 2783, -60) to (480, 2833, -123) stays in the
graph: 10-12d showed it holds from every start tried, and the crease is
kept out by the pass and drain rules. The deaths left on master:

- The mast platform's roof, 2 of 4 (seeds 379 bot 5, 457 bot 0, traced):
  both came off the mast's foot east along the deck at 664, dropped onto
  the roof at (3587, -260, 634), passed (3514, -283, 623) 24 units short
  at (3529.6, -265.1, 626.8) on the ground at 158 units/s, 9 up the
  slope, ran for the ridge node (3496, -318, 611), flew over it and fell
  to (3421, -419, 137). From the node the walk arrives; from 4 units
  aside, and from 43 of 45 starts within a unit of where the bot passed
  at the bot's own flat pace, it falls (scratch walks); `edge_holds`
  caught neither because the bot's own start arrived by a unit. Seeds
  1583 (the crow's nest, (4933, -47, 535)) and 1601 ((3445, -397, 56))
  were not traced.
- Tried first: the walk on from the body taking its whole velocity, the
  slope's 9 units/s rise included (with it the walk from seed 379's
  pass falls, flat it arrives). The edge was then avoided and the replan
  went (3515, -323, 631) to the ridge, where bots flew off far more often:
  deaths over seeds 1-1535 went from 2 to 44, 41 of them at
  (3454, -419, 137). Backed out.
- Kept: such walks priced risky (section 2). Plans go round the roof;
  the branch's stalls are seed 1337's three bots clearing the head of
  the ladder at (3369, -379) by turns (35 bot-ticks, two of them 30
  apart), seed 1713's one spell on the hull crates at (2325, 637, -47),
  and seed 861's crow's nest bot-tick and seed 2171's climb at the mast
  hatch, both on master too. No new death place.

The 2026-10-15h round took seed 1337's ladder and seed 2171's hatch.
VERIFIED (measured, same detector and settings, release; master
7567c4e3 against the branch, ours only):

| | seeds | stalled bot-ticks | falls | deaths |
|---|---|---|---|---|
| `mp_ship` master | 1-2303 odd | 48 | 6 | 0 |
| `mp_ship` branch | 1-2303 odd | 24 | 6 | 0 |
| `mp_depot` master / branch | 1-191 odd | 0 / 0 | 1 / 1 | 0 / 0 |
| `mp_hurtgen` master / branch | 1-191 odd | 0 / 0 | 2 / 2 | 0 / 0 |

- The hold ladder (3369, -379), seed 1337. VERIFIED (traces of bots 0,
  1 and 5): the head node (3315, -380, 352) stands west of a doorway
  (y -388 to -372 at x 3300-3330, free-space grid), so a climber leaves
  the rungs through it. Bot 0 waited aside at (3268, -353, 352), more
  than 64 from the rungs, and the wait let go there on its first tick;
  then, with no climber left on the rungs, it let go of the climber
  walking out, since only a deadly head watched those. Bots 0 and 1 went
  for the doorway as bot 5 came out of it, and the three pinned each
  other for 12 s. Now a wait under way holds at its spot however far it
  stands, and at any head lasts while a body off the rungs stands
  between the spot and the rungs (`Follower::ladder_yield`,
  `nav::tests::a_wait_aside_lasts_until_the_climber_is_past_the_spot`).
  Seed 1337: 35 stalled bot-ticks to 0. This generalises `ladder_yield`
  to any number of bots: each waits while a climber or a walker out of
  the doorway is coming.
- The mast hatch (3705, 56, 992), seed 2171. VERIFIED (trace of bot 2
  and scratch walks): the bot left the rungs, passed the side node
  (3705, 85, 992) and headed west for (3648, 71, 994) holding a weapon
  at 0.9. From where it stood the walk at 1 arrives and the one at 0.9
  falls back down the hatch (3687-3693, 48-63 is open to the deck);
  the bot climbed the ladder four times. Now `edge_holds` fails a walk
  on, within two edges of a climb, that falls at the weapon's scale
  under 1 (`nav::tests::a_walk_on_that_falls_at_the_weapons_scale_is_not_taken`).
  Tried first: the walk at the weapon's scale for every walk on,
  arrive or not (stalled bot-ticks 48 to 102, two new deaths in the
  hold), and its fall off any ladder head's floor (held seed 471 on the
  shelf at (5384, -161, 396)). Seed 2171: 6 to 0.
- The hold ladder's head, seed 1327. VERIFIED (trace of bot 3): with
  the two changes above a bot backed off the head (3368, -385, 352) as a
  climber came up 70-90 under it, met its head, slid off the rungs and
  fell the 296 units to (3383, -513, 56) with its health already down.
  The head-on rules took a fall off a ladder under `DEADLY_LADDER` (480,
  the fall a whole body never lives through) as survivable. Now the
  follower's deadly height is the lowest fall `PM_CrashLand`'s damage
  at the bot's current health kills (`nav::fall_that_kills`: the
  percent linear from `bg_fallDamageMinHeight` to 100 at the max, its
  f32 share of max health truncated, 110 past 99; `pmove::fall_damage`
  and the server's `fall_damage`), set each tick from the bot's vitals
  (`Follower::fall_kills`). A hurt bot at such a head waits for a
  climber from 140 under it, and one backing down climbs back to the
  head for one (`nav::tests::a_hurt_bot_waits_for_a_climber_further_down`); seed
  1327's bot waits at (3273, -324, 352) and lives. The graph's own
  perches and risky drops keep 480.
- Left: seed 861's one bot-tick is a roam that climbed the crow's nest and
  turned back down, and seed 471's 17 the shelf drop at (5343, -155,
  448), a spell place since 10-12e. The mast's down edge from the spar
  (3675, -86.5) has been in the graph since 10-13
  (`nav::tests::ships_ladders_are_climbed_both_ways`), and `mp_depot`'s
  ladder foot no longer drops a bot (its census falls are at (-791, 90)
  and (-1853, -699), elsewhere).

The 2026-10-15i round took seed 471's shelf. VERIFIED (measured, same
detector and settings, release; master 39838b03 against the branch,
ours only):

| | seeds | stalled bot-ticks | falls | deaths |
|---|---|---|---|---|
| `mp_ship` master | 1-2303 odd | 24 | 6 | 0 |
| `mp_ship` branch | 1-2303 odd | 4 | 7 | 1 |
| `mp_ship` master | 2305-4607 odd | 246 | 6 | 6 |
| `mp_ship` branch | 2305-4607 odd | 201 | 5 | 6 |
| `mp_depot` master / branch | 1-191 odd | 0 / 0 | 1 / 1 | 0 / 0 |
| `mp_hurtgen` master / branch | 1-191 odd | 0 / 0 | 2 / 2 | 0 / 0 |

- The shelf, seed 471. VERIFIED (trace of bot 2, the path and the
  graph): the bot came down from the floor at 448 onto the shelf node
  (5384, -161, 396), landed 55 units east of it, walked back west and
  passed the slot node (5403, -160, 396), whose drop to (5405, -192, 204)
  it then avoided from on the node. The new plan's first edge, from the
  same node to the stair at (5446, -197, 256), is a jump edge 140 down:
  the graph's walk ran off the shelf's lip to the floor at 204, turned
  back and jumped a stair step on the way up to 256. The bot still
  slowed from its walk west at 1.9 units a tick, which `jump_cue` read as
  pinned (closing under `PINNED`), so it jumped straight up off the
  shelf, came down on it, jumped again and flew north onto the floor at
  352, where it hopped against the shelf's face under a waypoint 96 below
  it for 80 ticks before the plan gave up, then climbed the ladder and
  came down the same drop again: 17 stalled bot-ticks. Every rule on the
  shelf was right; the cue was not. Now, under a top more than a jump
  below, the body reads pinned only on its second cmd in a row with the
  key held at that waypoint and closing under `PINNED`, as the walk's
  stall counts two ticks (`Bot::pinned_ticks`,
  `bots::tests::a_bot_turning_for_a_top_below_is_not_pinned`, which fails
  without it). Traced, the bot turns, runs south-east off the lip and
  goes on down the stair. The graph has 212 such jump edges on `mp_ship`
  (of 2 727 jumps), most of them a rail and the deck below it. Seed
  471: 17 to 0.
- Tried first and backed out: the cue for every jump edge with the key
  held at the waypoint (203 stalled bot-ticks and 3 deaths over seeds
  1-2303), and a body running from its waypoint read as not pinned
  (`closing.abs() < PINNED`: 31 and 5 deaths, at the crow's nest's rail
  drops to 535).
- Seed 981's death (bot 4, traced) is the hold ladder's head floor at
  520: the edge from (3392, -316, 520) west failed `edge_holds` at pace
  on the node, the plan was dropped, and the body carried its pace off
  the floor's south edge, down to 400 and on into the hold. No jump
  edge took part; the fresh-seed runs above fall there on master too
  ((3267, -419, 56) to -64). On master the second 1 152 seeds kill six
  bots, at the ledge over the hold ((3719, -396, 92), three), the crow's
  nest's foot at 535 (two) and (4983, -34, 616); the branch kills six at
  the same places. Seeds 1-2303 are the ones earlier rounds were tuned
  on, so their 0 says less than the fresh run's 6 against 6.
- Left: seed 1713's 6 bot-ticks (master only; the run diverges on the
  branch) are a bot bouncing on the hull crates at (2285-2300, 575-600,
  -48 to -40), on the ground one tick in two, short of (2272, 605, -32);
  two unstick spells take it off. Seed 861's one bot-tick as before.

The 2026-10-16 round took the fresh-seed deaths. VERIFIED (measured,
same detector and settings, release; master 730aa7bb against the branch,
ours only):

| | seeds | stalled bot-ticks | falls | deaths |
|---|---|---|---|---|
| `mp_ship` master | 1-2303 odd | 4 | 7 | 1 |
| `mp_ship` branch | 1-2303 odd | 26 | 5 | 3 |
| `mp_ship` master | 2305-4607 odd | 201 | 5 | 6 |
| `mp_ship` branch | 2305-4607 odd | 79 | 7 | 3 |
| `mp_depot` master / branch | 1-191 odd | 0 / 0 | 1 / 1 | 0 / 0 |
| `mp_hurtgen` master / branch | 1-191 odd | 0 / 0 | 2 / 2 | 0 / 0 |

- The ledge over the hold, three of master's six fresh deaths (seeds
  2575, 3099, 3959, traced). Every one roamed to the ledge at 400 by the
  only way onto it, the back down from (3647.0, -352.9, 611.1) to
  (3685.5, -354.3, 400.1), risky since 10-13 (section 2), and flew past
  the ledge into the hold at 92. Scratch walks of that back down from
  the body's state in seed 2575 (on the ground at (3637.0, -356.8,
  618.0), 131 and 69 units/s) land at a weapon scale of 0.9 and fall
  at 1 and 1.16 (the bot held 0.96), and from rest on a 5 by 5 grid 4
  units apart round the node 13 of 25 land at 0.96 and 4 fall: no rule
  on the approach makes it safe. The
  graph now keeps the largest strongly connected component with the
  risky edges left out (`NavGraph::safe`), and a roam from inside it
  passes over a destination outside it, as it does a perch
  (`nav::tests::a_roam_never_ends_where_only_a_risky_edge_leads`, which
  fails without it). On `mp_ship` that leaves out 443 of the main
  component's 13 954 nodes (the ledge at 400, parts of the hold at 56
  and 352 under the ladder at (3369, -379), the platforms at 600 and
  616 round the crow's nest's foot). A `To` goal still plans through a
  risky edge, priced as before. No ledge death is left on either seed
  range.
- The hold ladder head floor at 520 (seed 981, the 10-15i follow-up):
  the bot ran at the node (3392, -316, 520), six units from the floor's
  south edge, `edge_holds` failed the walk west at its pace, and the
  body ran on at the node and over the edge. Tried and backed out: a
  node whose walk on fails at pace held (`Follower::held`) so the bot
  slows onto it; over seeds 2305-4607 deaths went from 6 to 14, eleven
  of them at (4464, 383, 56). Seed 981 no longer dies on the branch, but
  only because the run diverges.
- The crow's nest, the rest. VERIFIED (traces of seeds 4119, 4475,
  1053, scratch walks): the hatch at (4963.9, 64, 1219.1) is backed down
  onto its rungs (the face at x 4975-4976, y 59 to 69) only from a
  window about x 4962 to 4965, y 56 to 70; from (4964, 50) or (4960, 60)
  the back down falls past the rungs to the deck at 692 or 535. A bot
  passes the head node at `REACH` (24) and backs down from where it
  stands: seed 4475's bot from (4963.7, 41.7). In seed 4119 a second bot
  heading for the same head stood 30 units north and held the first
  bot's back down off the rungs' line. Tried on top of the roam rule
  and backed out, totals over both ranges against its 105 stalled
  bot-ticks and 6 deaths: (a) the head of a ladder whose fall kills
  passed only at rest within 3 units of it: 725 and 4, as bots waiting
  to stand on the head jam its traffic (199 on seed 1003 alone); (b)
  (a) plus a bot at such a head yielding to a body off the rungs nearer
  them: 601 and 5; (c) (a) falling back to the old pass after 10 ticks
  without closing: 404 and 8, five of them off the north side at y 165
  to 200. The branch's deaths are the
  crow's nest's (seeds 1053, 1389, 3329, 3785, 4475) and seed 2229's two
  bots meeting in the air off the ridge node (3496, -318, 611) onto the
  floor at 520, the lower knocked into the hold.

The 2026-10-18 round took the fresh-seed deaths on master 0f3ce28e.
VERIFIED (measured, same detector and settings, release; ours only):

| | seeds | stalled bot-ticks | falls | deaths |
|---|---|---|---|---|
| `mp_ship` master | 1-2303 odd | 38 | 6 | 4 |
| `mp_ship` branch | 1-2303 odd | 82 | 8 | 2 |
| `mp_ship` master | 2305-4607 odd | 130 | 7 | 2 |
| `mp_ship` branch | 2305-4607 odd | 130 | 8 | 0 |
| `mp_depot` master / branch | 1-191 odd | 0 / 0 | 0 / 0 | 0 / 0 |
| `mp_hurtgen` master / branch | 1-191 odd | 7 / 7 | 2 / 2 | 1 / 1 |

- None of the brief's places died or stalled on master: no crow's nest
  death or stall over the 2304 seeds, and seeds 861, 981, 1003, 1053,
  1389, 2229, 3329, 3785, 4475 and 4557 run clean (the runs diverged
  since 10-16). Master's six deaths: three off the 896 deck (seeds 427,
  2477, 4523), the mast top (311), the ledge over the hold (1239) and the
  x 3793 hatch (1795).
- The 896 deck, traced: all three bots came west along y 224 and passed
  (3564, 252, 896) 12 units short at (-173, 80) units/s for the run off
  to the ledge at (3521, 289, 760). The paced walk from there lands on a
  slope's corner at 798 that it clips by 2 units; from 2 units across
  the pace it misses, as the bots' own runs did, and falls to the deck at
  352. Now a run off a drop whose miss can hurt has to land from 2 units
  either side too (the pass rules, above;
  `nav::tests::a_drop_that_lands_only_on_a_corner_does_not_hold`). Asked
  of every drop, it held a bot on a node another stood on over a drop
  into the hold (seed 439, 17 stalled bot-ticks): hence `a_miss_hurts`.
  Without the coast exemption three bots on the mast top at (3301, 128,
  1030), 3 units from its lip, refused the drop to the platform at 809
  from on the node, turned round with nothing to steer by and coasted
  off to the deck at 480 (seeds 449, 1315, 1409).
- The x 3793 hatch (1795, traced): a hurt bot backing down over the lip
  at (3806, 60, 623) met a climber 140 under it, headed straight for the
  wait spot (3751, -4, 616), slid along the hatch's west side at x 3800
  and fell through to the deck at 304. Now it walks back onto the head
  first (the ladder wait, above). At a 1-unit margin a bot resting
  5 past (3337, -440, 56) stood in its climber's way (seed 3769, 117
  stalled bot-ticks), hence the half-width.
- Left: the branch's new stalls are seed 1671 (29: the x 3793 head, a
  bot at the wait spot while the climber stands beside it on its way
  off the head, until the wait runs out) and seed 2001 (15: the hull
  crates at (2395, 752, -45), as seeds 1713 and 2751). Master's mast top
  death (311: a bump off the mast turned the run toward (3335, 128) west
  off the platform) and the ledge over the hold (1239) are on the branch
  too.
- Seed 981's overrun, from scratch walks: a body at 160 units/s on
  (3392, -316, 520) headed south falls off the floor's edge with no key
  and with a full brake toward the node alike, so no rest rule saves it;
  the pace has to go before the node.

The 2026-10-18b round, on master 33013a7f (the collision and prediction
merges after 10-18 moved every run). VERIFIED (measured, same detector
and settings, release, seeds 1-4607 odd; ours only):

| | stalled bot-ticks | falls | deaths |
|---|---|---|---|
| master 33013a7f | 161 | 25 | 6 |
| back down aimed across the edge (backed out) | 164 | 26 | 7 |
| partway landing replanned, drop avoided | 220 | 11 | 6 |
| partway landing replanned (kept) | 181 | 10 | 6 |

`mp_depot` and `mp_hurtgen`, seeds 1-191 odd: 0 / 0 / 0 and 0 / 3 / 0 on
master and the kept row alike.

- None of the 10-18 follow-ups reproduce on master: seeds 1671, 2001 and
  311 run clean, seed 1239 still dies at the ledge over the hold. Master's
  stalls are 143 of 161 in three spells: seed 1415 (two bots pinning
  each other in the passage at (4140-4225, 207-256, 304) by the ladder at
  (4245, 230)), seed 2553 (two bots hopping under the hull crates at
  (5790-5830, 550-600, 56-132), each jump short of the crate top at 132)
  and seed 3513 (one bot held in a crease between the hull crates at
  (2188.9, -289.8, -47.9), on the ground one tick in two, as seeds 1713
  and 2751 were).
- The slot at (5403, -160, 396), nine of master's 25 falls (seeds 73,
  149, 183, 317, 1217, 1913, 3151, 4177, 4333; seed 3151 traced): the
  run off the slot's drop for (5405, -192, 204) landed on the ledge at
  336, 8 units from straight over the waypoint, and the bot turned on the
  spot (its back down had nothing flat to aim along) until its stall
  circle's unstick spell threw it off the ledge's east side to 204. Now a
  body on the ground more than a jump over its waypoint, within `REACH`
  of it flat and more than a step under the edge's top plans again from
  where it stands (`nav::tests::a_drop_that_lands_partway_down_is_planned_again`,
  which fails without it); traced, it walks off the ledge onto the floor
  at 352 and on. With the drop avoided as well, seed 1409's bot went
  back up to the shelf, took the jump edge to the stair at (5446, -197,
  256) and bounced under it for 150 ticks (39 stalled bot-ticks), so the
  drop stays in the plan. The kept row's extra stalls are seed 761 (26:
  at the x 3793 hatch, a bot waiting at the spot (3751, -4, 616) on the
  climber's way out along x 3751, the 10-18 seed 1671 family) and seed
  395 (2), both runs that parted from master at an earlier replan.
- The crow's nest's back down, from scratch walks off the plate
  (z 1219.125, toward the foot (4959, 64, 692)) on a 1-unit grid: from
  rest the creep lands on the rungs only from x 4960-4964 and
  |y - 64| <= x - 4960, a diamond round the head node (4963.9, 64); a
  bot passes the head at `REACH`, and its back downs in seeds 1919, 3941
  and 4161 began at (4963.5, 55.9), (4963.6, 57.8) and (4963.6, 73.5),
  outside it or on its edge. Seed 1919's was also held off the rungs'
  line by a second bot 30 units north on the plate. Aimed at the foot
  moved half the body's offset across the edge, the window from rest
  widens to y 48-80 at x 4964 but stays within x 4960-4964, and over the
  2304 seeds the crow's nest still killed four bots (seeds 995, 1307 and
  two in 4161): backed out.

The 2026-10-18c round, on master 7256c89c. VERIFIED (measured, same
detector and settings, release, seeds 1-4607 odd; ours only):

| | stalled bot-ticks | falls | deaths |
|---|---|---|---|
| master 7256c89c | 141 | 11 | 6 |
| a climber pressed on the waiter ends the wait | 75 | 12 | 7 |
| and the crow's nest creep faces the rungs | 88 | 14 | 4 |
| and a stopped run off a drop plans round it (kept) | 88 | 8 | 4 |

`mp_depot` and `mp_hurtgen`, seeds 1-191 odd, master against the kept
row: 0 / 0 / 0 and 1 / 6 / 1 on both. Without the rule's "once" (below)
`mp_depot` read 6 900 stalled bot-ticks.

- The x 3793 wait spot, seed 761 (26 stalled bot-ticks on master,
  traced): bot 1 reached the head (3760, 65, 616) as bot 4 climbed from
  the hatch's foot at 304 and waited at the spot (3751, 2, 616). Bot 4
  stepped off the head heading south along x 3751, its way out through
  the spot, and pressed against the waiter at (3751, 32); the wait
  lasted while a body off the rungs stood between the spot and the
  rungs, which bot 4 did until the wait's hung ticks ran out. Now, at a
  head whose fall does not kill, a body within two half-widths and 2 of
  the waiter flat holds no wait; at a deadly head the old rule stands
  (the crow's nest's exits put the climber that close by design). Seed
  761: 26 to 0. Seed 1415's 50 (two bots pinning each other in the
  passage at (4140-4225, 207-256, 304)) went with it: that run parted
  at an earlier wait.
- The crow's nest, four of master's six deaths (seeds 1919, 3941, 4161,
  4545, each traced): a bot waited aside at the head for a climber, came
  back onto the head off the rim and backed down from (4957-4963,
  45-73), the creep's heading held from its first tick toward the foot
  (4959, 64, 692), and fell past the rungs (x 4975-4976, y 59-69) down
  the hatch to 535 or 616. From scratch walks on a 1-unit grid round the
  head (x 4958-4964, y 48-80, at rest and at 30 and 80 units/s across):
  backed toward the foot, the body grabs the rungs only from the diamond
  of 10-18b; facing the rungs square (yaw 0, pitch up 45) and strafing
  to their middle at y 64, it grabs and climbs down to 692 from every
  cell, at a weapon scale of 1 and 0.96 alike; square without the
  strafe, from y 56 to 72 only. The bot now does that
  (`bots::tests::a_creep_off_a_deadly_head_faces_the_rungs_and_strafes_to_them`,
  `nav::tests::the_crows_nest_back_down_faces_the_rungs`), only off a
  ladder whose fall kills: every other back down keeps the graph's
  heading. The four seeds run clean. Its +13 stalled bot-ticks and +2
  falls are runs that parted (seed 995: 14, a bot at the hull crates).
- The hold floor at 56, five of master's falls (seeds 447, 1813, 2195,
  2387, 2561) and one more on the crow's nest row (1919): the drop from
  (3296, -419, 56) to (3264, -378, -64), and its mirror at y 547.
  Traced (seed 2195): the bot passed the top node at `REACH` running
  west along the wall at y -400; its walk on at that pace lands (round
  the wall's west end at x 3257), but it bumped a bot dropping ahead of
  it at (3246, -411), lost its pace and pressed north at its waypoint
  behind the wall, creeping west at under 10 units/s to x 3266, where
  the walk from rest is blocked; an unstick spell then ran it off the
  floor's south side to -64. Now a body on the ground on a run-off
  drop's top (within a step of the top node's height, the edge a
  forward walk's, not a jump), more than 24 off the waypoint flat and
  slower than `UNPIN_SPEED` (20), whose forward walk at its pace no
  longer lands, plans round the edge
  (`nav::tests::a_run_off_a_drop_stopped_short_of_the_lip_plans_round_it`,
  which fails without it). All six run clean. The rule fires once per
  edge while the edge is avoided: a plan with no way round takes it again
  from rest, and on `mp_depot`'s roof at 212 a run off from rest misses
  each of its drops to -24 (seeds 15, 99, 115, 119, 151), so bots
  planned round and back with no waypoint for whole runs.
- Left: the kept row's deaths are seed 1307 (a bot on the crow's nest
  top at (4936, 56, 1254) took a westward shove of about 240 units/s in
  one tick, ran off the top's west side and fell to 494; not traced to
  its source) and three on the hold floor at 56 ((3643, -420), (3498,
  -403), (3579, -422); seeds 2245, 2491, 4231, untraced; 2245 dies on
  master too). Seed 1239's ledge death no longer reproduces on master.
  The stalls are seed 2553's 49 (hull crates at (5790-5830, 550-600)),
  995's 14, 3443's 13 and 1109's 7.

The 2026-10-18d round, on master 1080e30d. VERIFIED (measured, same
detector and settings, release, seeds 1-4607 odd; ours only):

| | stalled bot-ticks | falls | deaths |
|---|---|---|---|
| master 1080e30d | 88 | 8 | 4 |
| rising hold and the drop's own slope | 205 | 8 | 2 |
| and of two pressed bodies one gives way (kept) | 40 | 8 | 1 |

`mp_depot` and `mp_hurtgen`, seeds 1-191 odd: 0 / 0 / 0 and 1 / 6 / 1
on master and the kept row alike.

- Seed 1307, the crow's nest top (traced, both bots' cmds and
  playerstates): bot 2 stepped east from (4928, 61, 1254) toward the
  rim node (4964, 36, 1219) with forward 127 at yaw -36.5 while bot 1
  climbed the hatch's rungs at (4960, 63, 1187), 24 units off flat.
  Bot 1's box (top 1257) met bot 2's (feet 1254), and the end frame's
  `StuckInClient` (`game::stuck`, cod11-player-clip.md 6) set bot 2's
  velocity from (102, -75) to (-183, -53), `ps.speed` along their
  separation, with the 300 ms knockback timer: the westward "shove" of
  the 10-18c follow-up. It ran off the top's west side and fell to 494.
  Now a body on the ground off the rungs holds still for up to 60 ticks
  while its step toward the waypoint would close within two half-widths
  and 6 flat of a climber rising under it, the climber's top less than
  48 under its feet (`Follower::rising_hold`,
  `nav::tests::a_body_holds_still_over_a_climber_rising_under_it`).
  Not toward a waypoint more than a jump down: the ladder waits own a
  body bound down the climber's own ladder.
- Seed 2245, the dome at (3648, -317, 618) over the hold (traced, with
  scratch point traces): the bot passed the dome node for the drop to
  (3680, -318, 352) and backed onto the dome's 45 degree east face; on
  the ground 23 under the node, 244 over the waypoint and 15 from it
  flat, it read as a landing partway down (2026-10-18b), planned again
  from there, headed for (3647, -353, 611) and walked off the dome's
  south side. The partway rule now needs the body more than a jump (39)
  under the edge's top, not a step (18): the slot's ledge at 336 is 60
  under its top at 396
  (`nav::tests::a_body_on_a_drops_own_slope_keeps_the_drop`, which fails
  with the step).
- Seed 3223, the run the two rules parted: bots 4 and 5 climbed the
  ladder at x 3792 one 72-76 over the other, at y 80-89 and y 44-55,
  both outside the hatch's lane (y 58-72, 10-12h). The upper bobbed
  under the hatch's lip while the lower caught up (2.2 against 4.7
  units a tick); outside `ABREAST_HEIGHT` neither gave way on the
  rungs, and on the plate at x 3800 each pressed toward (3779, 64, 623)
  at the lane the other's capsule stood in for 180 ticks (111 stalled
  bot-ticks). `unpin_yaw` traces the world only, so a pin on a body has
  no heading out. Now of two bodies pressed together on the ground,
  both slower than `UNPIN_SPEED` for 10 ticks and within 48 of the
  waypoint, the one farther from it (of two level, the later in
  (y, x)) steps 24 back from the other for 20 ticks
  (`Follower::pressed_yield`,
  `nav::tests::of_two_bodies_pressed_together_one_gives_way`). A taller
  `ABREAST_HEIGHT` would send the lower of every two climbers on one
  ladder back down. With it, seed 2553's hull crates (49 on master)
  and seed 995's (14) run clean as well.
- Untraced by a fix: seed 4231 (on master): passed the dome node
  (3615, -345, 635) at `REACH` 6.5 under it while climbing the dome,
  turned for (3584, -349, 640) and flew off the crest south. The walk on
  from the body at its pace, with the pace's rise or without, lands on
  the dome's south face at (3591, -380, 622), a few units from its edge;
  the bot flew 14 units further south and fell. A level edge whose walk
  on leaves the ground over a fall that kills has no aside test (the
  2-unit one is the drops'). Seed 2491 (on master): an unstick spell on
  the dome's west side. Seed 2553 on master: two bots on the crate top at
  92 by the stringer at x 5812, hopping on jump edges onto the stairs
  (7289 to 7291 at 132, 7487 to 6850 at 76) that land back where they
  started. All three run clean on the kept row (runs that parted).
- Left on the kept row: the death is seed 4215 (the hold under
  (3684, -406, 92), the 10-12 family); the stalls are 3443's 13, 3435's
  9, 825's and 3971's 6, 3493's 4 and two of 1.

The 2026-10-19b round: the ladder yaw cap and the push-off that jumps
facing the rungs (10-19, section 3 "Ladders" above; cod11-mantle.md, "The
ladder yaw cap"), master 447d904f against 47171373, the master before
them. VERIFIED (measured, same detector and settings, release, seeds
1-4607 odd; ours only):

| | stalled bot-ticks | falls | deaths |
|---|---|---|---|
| 47171373, before the cap | 40 | 8 | 1 |
| master 447d904f | 55 | 7 | 0 |

47171373 reads the 10-18d kept row to the bot-tick. `mp_depot` and
`mp_hurtgen` (seeds 1-191 odd) read 0 / 0 / 0 and 1 / 6 / 1 on both, and
United Offensive's four `tdm` maps (below) the same on both to the
bot-tick, seeds 1-63 odd.

- Every seed that moved is a run that parted, none at a push-off. Of
  the first divergences traced (both builds' every bot every tick), seed
  4229's is at the hold ladder's foot at (3369, -381.6, 56), body on
  the ground and on the rungs with the waypoint 51 behind: 47171373
  jumped facing out and the back key held it on the spot for 13 ticks
  in the air; master walked off. Seeds 2025 and 501 part at the drop
  from (5926, 64, 352) to 204, a tick's move apart with the same
  waypoint: `mp_ship`'s graph has a new digest (`nav::Sim::tick`'s walks
  rebase on `delta_angles`, section 3 above).
- Master's new stalls: seed 2025's 11 (unstick spells by the crates in
  the hull at (2189, -290, -45), no ladder), seed 501's 7 (the crow's
  nest head: a climber turned back up the rungs for a second climber
  below and waited on the head for it), seed 4229's 5 (off the ladder
  top at (7232, 261, 142), twice walked off the ledge at 166 by (7250,
  240) onto the roof sloping down to y 355 and slid to the hull floor;
  the stair edge (7232, 261, 142) to (7260, 225, 166) the graph proved),
  2747's 3 and 3601's 1. Gone: 825's and 3971's 6, 4215's death and five
  falls; master's four falls are new seeds (971, 1279, 3569, 3747).

The detector is `crates/server/examples/ship_stalls.rs` (`MAP=`, `TICKS=`,
`TRACE=<slot,...>:<from>:<to>` prints those bots' every tick with their
waypoints and ladder waits; each stall line ends with the distance to the
nearest other bot). It counts every playing bot-tick 12 s within 150
units and leaves out only a body with a UO LMG's bipod down; with its
defaults (`dm`, fire off) nothing else holds a bot still. `tests/bots.rs`'s
`uo_tdm_games_that_stalled_or_never_sprinted` also skips engaged and
crew-wait ticks, which never enter its window. `bot_match` keeps the
window as `ship_stalls` has it and splits the count into `stalled` and
`held`, a window with a tick `BotBody::holds` names in it
(bot-combat.md section 1); `ship_stalls` prints the same `held` and,
on each stall line, the ticks since the bot last held (`ago`).

The probe costs 25 us a body (8 octants, up to five capsule traces
each) and 43 us with 10-13d's half-way samples. VERIFIED (measured
2026-10-13f, release, 2 000 `mp_ship` nodes, the least of 12 rounds;
1 868 and 1 905 drops of the 16 000). Since 10-13f it runs only when
`Bot::reads_drops` says the brain reads it: 528 of 71 964 bot-ticks on
`mp_ship` (seeds 5, 101, 1001), 176 of 23 988 on `mp_depot` and 61 of
23 988 on `mp_hurtgen` (seed 5), about 2 us a tick for six bots where it
was 0.26 ms of a 2.4 ms tick; the detector's logs over 48 `mp_ship`
seeds were byte-identical with it on every tick
(`bots::tests::drops_are_felt_only_when_the_brain_reads_them`). `tests/bots.rs`,
`bots_on_mp_ship_unstick_without_walking_off_the_deck`, replays seed 5
to tick 1800 and fails at tick 1671 without the probe.
In S&D the objective names the point and can hold the bot still
(`bot-objectives.md`); a bot standing at its objective or linked by the
script is never counted as stuck.

### The lip walk

The 2026-10-20 round compared the bot with the walk that proved its edge,
tick by tick, at `mp_depot`'s roof by the ladder at x -2750, on master
180696ad (per-bot generators in). VERIFIED (measured, same detector and
settings, release; ours only):

| | seeds | stalled bot-ticks | falls | deaths |
|---|---|---|---|---|
| `mp_depot` master | 1-2047 odd | 2194 | 7 | 0 |
| `mp_ship` master | 1-1023 odd | 82 | 3 | 0 |

- The two drops 10-12h left are gone as creeps. VERIFIED (measured, the
  graph's edges and walks): the roof node (-2751, -1244.3, 508.1) drops
  to the ledge at (-2747.6, -1270.8, 326.1) by a forward run, marked
  risky, and nothing on the roof drops to the north ledge
  (-2754.6, -1150.3, 326.1). Bots reach both by backing down the ladder
  from its head (-2714.2, -1217, 527.4) to its foot (-2766, -1217,
  326.1). Every creep-only drop near a ladder on the graph also lands off
  a turn at `TURN_PACE`, or never from rest, so `Follower::creeps_off`
  holds none of them. `tests/bots.rs`
  `a_bot_gets_down_off_mp_depots_roof_onto_the_ledges` sends a bot from
  14 starts across the roof to each ledge; all 28 runs stand on it with
  the body never under 320.
- From rest on the head the bot and the walk's creep (`walk_with`, pace
  0) move alike: 5.78 units a tick backing at the first's -127, a grab of
  the rungs on the same tick past the lip, 6 units a tick down them.
  The creep on the roof is 0.754 units a tick to the walk's 0.785, the
  bot's weapon. VERIFIED (measured, both traces).
- With `RISK_COST` at 0 the plans take the forward drop: the bots come
  onto (-2751, -1244.3) running west off the turn at about 170 units/s,
  `Follower::edge_holds`' run from the body misses, and the follower
  goes round by the ladder. All 28 starts got down. VERIFIED (measured).
- The deaths are gone; the depot stalls are seed 663's 2118 (an unstick
  spell at (-1999, -470, 376) ran the bot down 32 into the gutter at
  (-2028, -451, 344), off the graph, which it never left) and seed 1145's
  76. The depot falls and seed 225's on `mp_ship` were unstick spells
  off a ledge beside another bot.
- Seed 1145 is a jump the bot makes and the walk did not. The bot stood
  in the pit at (-1569, 2087, 212) with the jump edge to the step at
  (-1533.2, 2112, 239.1) ahead; it jumped as soon as `jump_cue` read it
  pinned, carrying 5 units a tick from its last landing, fell back, and
  did it again. The flood proved the edge running at the column's centre
  (-1536, 2112): the run bobbed on the step and jumped at 1 unit a tick,
  landing at (-1540, 2112.8, 239.1) within the node's floor. Aimed at the
  node, the same run bobs on the step and never lands it. VERIFIED
  (measured, walk traces). On `mp_depot` 194 of the 963 jump edges do
  not land walked at their node: aimed at a column's centre, at a node
  made where a centre's walk came to rest, or 8 units aside
  (`walk_jobs`' sidestep).
- Tried and dropped: walking every jump edge again aimed at its node and
  keeping only those that land (`nav_census`, VERIFIED): `mp_dawnville`
  went from 178 spawns in one component to 176. Spawns 88 and 154 at
  (145, -15311, 7) are reached only by the jump from (-96, -15409, -13)
  to (-89, -15360, 19), proved 8 units aside; aimed at the node the run
  pins on the wall at y -15375 and `jump_clear` refuses the jump, while
  a bot sent there on master jumps on its cue and arrives from 2 of 3
  starts. The walk's jump and the bot's are not the same rule: the walk
  jumps after two pinned ticks and only where `jump_clear` passes, the
  bot whenever it closes under `PINNED`. Since aligned
  ("The jump cue and the re-proof" below).

### The crest run and the stringer's jump

The 2026-10-20 round took the two `mp_ship` edge follow-ups of 10-18d
(seed 4231's dome crest, seed 2553's crate-top jumps). Neither seed
shows them on master 296bed66, so both were traced on 1080e30d, where
they reproduce, and checked on both. VERIFIED (measured, same detector
and settings, release, seeds 1-4607 odd; ours only):

| | stalled bot-ticks | falls | deaths |
|---|---|---|---|
| master 296bed66 | 230 | 6 | 0 |
| aside test on every run on that left the ground | 283 | 11 | 5 |
| and the walked jump, the aside test on a dip only (kept) | 151 | 2 | 1 |

`mp_depot` and `mp_hurtgen` (seeds 1-191 odd): 0 / 0 / 0 and 0 / 2 / 1
on master and the kept row alike, the same events. United Offensive
(`UO=1`, `tdm`, 2400 ticks, seeds 1-63 odd), stalled / falls / deaths,
master against the kept row: `mp_cassino` 25 / 3 / 0 both, `mp_foy`
0 / 0 / 0 both, `mp_italy` 179 / 0 / 0 both, `mp_uo_stanjel` 26 / 1 / 0
against 0 / 3 / 0 (two unstick spells off (276, 3388, -390)).

- The dome crest (seed 4231 on 1080e30d, traced, `edge_holds` logged):
  the bot passed the node (3615.5, -344.7, 635.3) at (3636.6, -336.8,
  628.8) climbing at (-122, -124, 31) units/s for (3584, -349, 640).
  The walk on from there lands, dipping 7 under its start onto the
  south face; the same walk 2 units either side across the pace falls
  214. Now a run on along an edge that is neither a jump nor down more
  than a step, that comes down off the ground under where it set off
  and onto a node where a miss can hurt (`a_miss_hurts`), holds only if
  neither walk 2 units aside falls deeper than a jump
  (`Follower::edge_holds`, `nav::tests::a_run_on_that_leaves_the_ground_over_a_fall_lands_from_either_side`).
- Asked of every run on that left the ground (the second row), it
  turned down mp_ship's 32-unit step-downs at (3315, 508, 352) and
  (3359, -384, 352), the jump onto the crow's nest top from the rim at
  (4964, 36, 1219), and the level walk to the crow's nest ladder's foot
  at (4959, 64, 692), whose runs aside hop a lip and never arrive
  without falling (lowest 1.4 over the start). The routes that changed
  put bots on the crow's nest top in seeds 421, 525, 2369 and 2879,
  which ran off it (below). Hence the dip and the depth.
- The stringer (seed 2553 on 1080e30d, traced): the crate-top nodes
  are the stair stringer's end at (5812-5824, 567, 92), not a crate.
  The bot came onto the jump edge 7487 (5829, 544, 92) -> 6850 (5792,
  576, 76) at (5823.4, 567.1, 92.1), pace (22, 0); the walk on from
  there arrives without a jump, sliding off the stringer's end, where
  the graph proved the edge from its node with one. The bot, pinned on
  the stringer, jumped on its cue (`jump_cue`) and fell back: 155 of
  210 ticks near the stringer in the air. Now a jump edge whose walk on
  from the body arrives without the jump is run without it
  (`Follower::jump_cued`, `nav::tests::a_jump_edge_walked_on_without_its_jump_cues_no_jump`):
  on 1080e30d seed 2553 goes from 49 stalled bot-ticks to 0 and 45 of
  115 ticks in the air; the bot stood 16 ticks pinned at (5814.4,
  567.1) and an unstick spell took it off. The 7289 -> 7291 jump to the
  stair at 132 from the stringer's top still needs its jump and keeps
  it. The bot's own pin there (the walk slides, the bot stands) is not
  traced.
- Left on the kept row: the death is seed 111, the crow's nest top.
  The drop off (4928.5, 60.8, 1254) to the spar at (4888, 65, 1212)
  fails `edge_holds` from a bot coming onto the node off the turn at
  196 units/s; within `ON_NODE` the follower avoids it and replans, the
  body coasts off the top onto the spar and walks off it heading back
  for the rim. Tried and dropped: holding the edge so the bot comes to
  rest on the node first. The body is already 6 units past the node and
  coasts off before it stops (seed 525, traced), and held on every
  failing edge the bots stopped on `mp_hurtgen`'s slope at (292, 2785,
  -52) and replanned into its crease (4 deaths in 96 seeds); held only
  where the body would coast off (`coasts_off`), the crow's nest deaths
  stayed.

### The jump cue and the re-proof

The 2026-10-20 round took the lip walk's open divergence (above): the
bot's jump on a jump edge and the walk's were different rules, and the
walk proved its jumps aimed where no bot steers. VERIFIED (measured,
`ship_stalls`, `dm`, 6 bots, 4000 ticks, release, 1.1; ours only;
stalled bot-ticks / falls / deaths):

| | `mp_depot` 1-767 odd | `mp_ship` 1-1535 odd |
|---|---|---|
| master 8433f794 | 2186 / 2 / 0 | 76 / 1 / 0 |
| the walk's rule with its room test, no re-proof | 68 / 2 / 0 (1-383) | 129 / 5 / 0 (1-767) |
| the walk's rule with its room test, re-proof with it | 68 / 0 / 0 (1-383) | 341 / 6 / 2 (1-767) |
| the rule without the room test, re-proof as a bot | 131 / 1 / 0 | 126 / 1 / 1 |
| and no jump under a top while climbing a ramp, in every walk | 2 / 1 / 0 | 201 / 0 / 3 |

The last row was worse than master at `mp_ship`'s stairwell and mast
ladder, and the 2026-10-21 round traced it (master 4a427db7 after a
merge, the same detector; ship seeds in three blocks of 768):

| | `mp_depot` 1-767 | `mp_ship` 1-1535 | 1537-3071 | 3073-4607 |
|---|---|---|---|---|
| master | 2186 / 2 / 0 | 72 / 1 / 0 | 37 / 0 / 0 | 13 / 1 / 0 |
| the ramp wait in a bot's jump only | 131 / 1 / 0 | 104 / 2 / 0 | 123 / 1 / 1 | |
| and the bot pinned off its pace toward the top (kept) | 118 / 1 / 0 | 98 / 1 / 0 | 4 / 1 / 0 | 32 / 2 / 0 |
| the ramp wait in a bot's jump only, one jump an edge | 104 / 2 / 0 | 207 / 20 / 1 | | |
| kept, and one pinned tick under a top no deeper than a jump | 2186 / 2 / 0 | 28 / 1 / 1 | 9 / 3 / 1 | |

- The ramp wait (below) in every walk, the flood's too, turned 26 of
  `mp_ship`'s jump edges plain and dropped a node, and took the
  regression with it: on 11 traced seeds the last row's 201 / 0 / 3
  went to 0 / 0 / 0 with the flood's walks jumping under a top as
  master's do and the bot unchanged, and to 177 / 0 / 3 with the
  re-proof off and the wait kept. VERIFIED (measured, bisected with
  switches). It is now in a bot's jump only (`nav::as_a_bot`'s walks
  and `Bot::think`), and the flood's graph is master's again (same
  digest without the re-proof).
- The bot read pinned off the distance it moved. Sliding along the rail
  over the stair drop (5402, 321, 344) -> (5404, 354, 204), 140 down,
  it moved more than 2 units a tick and jumped only where it came to
  rest, past the drop, into the crease at (5445, 355, 292), where it
  hopped. Over seeds 1-79 odd the bots spent 153 ticks in that crease
  to master's 48; pinned off its pace toward the top, as master's bot
  was, 149, and they reached the floor at 204 88 times to master's 62.
  VERIFIED (measured, traces and tick counts). The two pinned ticks
  stay (`bots::tests::a_bot_jumps_on_its_second_tick_not_closing_on_the_top`).
  On the ground the body has to move under the pinned pace as well since
  "The recheck's leftovers" (below):
  `bots::tests::a_bot_sliding_along_a_ledges_face_jumps_where_it_comes_to_rest`.
- Tried and dropped, both measured in the table: one jump an edge, as
  the walk takes (bots stood pinned at that stair top and fell off it
  on unstick spells, 20 falls), and master's one pinned tick under a
  top no deeper than a jump (`mp_ship`'s stalls fall to 37 over 3072
  seeds, but 2 deaths and `mp_depot` seed 663's gutter spell back).
- The kept row against master over 4608 `mp_ship` seeds: 134 / 4 / 0 to
  122 / 2 / 0. Every fall is a single unstick spell somewhere new, and
  stalls come a seed at a time (a few at the mast ladder between
  (4960, 64, 1012) and its head at 1219, where two bots meet, which
  master shows at its other ladder at x 3808), so the two are within
  what reseeding moves. On `mp_depot` the kept row has seed 1145 at 0,
  seed 663's gutter spell gone, and seed 159 (36, the pit at
  (-1409, 2087, 212), below) and 221 (68, as master).

- The cue is the walk's own rule (`nav::PinJump`, run by `walk_sliding`
  and by `Bot::think`): jump on the ground after two pinned ticks, out
  of a bob (two such ticks sliding, then the next tick on the ground),
  or standing within 16 units flat under a top more than a step up. The
  walk is pinned moving under 2 units a tick (half one creeping); the
  bot closing on the top under 2 units a tick with its key held at it,
  which is the same for a walk run straight at the top. The bot used to
  jump on the first such tick, so landed short of `mp_depot`'s pit step
  at (-1533.2, 2112, 239.1) and carried back it jumped at once and fell
  back; the re-proof below drops that edge.
- The bot skips the walk's room test (`jump_clear`). With it, bots
  pinned on `mp_ship`'s stair at (5408, 333, 328) on the jump edge
  (5402, 321, 344) -> (5404, 354, 204), 6 units off the walk's line,
  where the box raised 41 meets the stair overhead toward the top, and
  fell off it on unstick spells (seeds 71, 231, 391), and on
  `mp_depot` a bot pinned under the 42-unit lip of (-1374, 2112, 254)
  fell the same way (seed 71); the old cue jumped there and the step-up
  took it over. VERIFIED (measured, traces).
- `NavGraph::reprove_jumps`, after the leaps, walks every other jump
  edge again from its node, aimed at its top (where a bot steers), with
  the bot's jump (`nav::as_a_bot`: the bob, no room test), then aimed
  8 units either side. An edge that lands only aside keeps that aim in
  `NavGraph::aims` and a bot on it steers there (`NavGraph::aim`,
  `Follower::jump_aim`); one that lands at none is dropped. The pit
  edge (-1569, 2087, 212) -> (-1533.2, 2112, 239.1) lands at none and is
  dropped (`nav::tests::a_jump_edge_that_lands_only_off_where_a_bot_steers_is_dropped`);
  `mp_depot`'s (-1760, 71, -24) -> (-1731, 32, 11) lands only aside
  (`nav::tests::a_jump_edge_that_lands_only_aimed_aside_keeps_its_aim`).
  `mp_dawnville`'s (-96, -15409, -13) -> (-89, -15360, 19), the one way
  to spawns 88 and 154, now lands at its node: the walk with the room
  test pins on the wall at y -15375 and refuses the jump, the bot's
  jump there lands (`nav::tests::a_jump_edge_is_proved_again_by_the_bots_jump`).
  Aimed jumps after the pass, on the kept row (`examples/nav_build.rs`):
  `mp_brecourt` 44, `mp_carentan` 118, `mp_chateau` 88, `mp_dawnville`
  41, `mp_depot` 81, `mp_harbor` 15, `mp_hurtgen` 71, `mp_pavlov` 87,
  `mp_powcamp` 28, `mp_railyard` 109, `mp_rocket` 44, `mp_ship` 124;
  `mp_depot` has 91 515 edges to master's 91 525. Spawns in one component
  are unchanged on all twelve (`tests/nav_census.rs`; dawnville 178).
  VERIFIED (measured). A room-tested re-proof dropped 0 to 22 edges a map
  aimed at the node and both sides, 20 to 146 aimed at the node alone,
  where `mp_dawnville` fell to 176.
- Tried and dropped: the follower's walk on along a jump edge
  (`Follower::edge_holds`) with the bot's jump too. `mp_depot` seeds 159
  and 247 cleared, but bots then climbed `mp_ship`'s mast ladder for the
  crow's nest's rim at 1254 and bobbed under the head at
  (4964, 64, 1219): stalls in ten seeds of 1535 that the room-tested
  walk on has in none (205, 423, 463, 625, 965, 1369 at the head, 57,
  381, 929, 1003 on the stair above). VERIFIED (measured, the same seeds
  each way).
- A bot's jump right under a top waits for a tick that rose under a
  unit (`nav::jump_under`; the bot and the re-proof's `as_a_bot` walks,
  not the flood's). With the re-proof above, `nav_census` at United
  Offensive lost `mp_uo_dawnville`'s spawns 220 and 248 (193 < 195):
  their one way in is the jump edge (-241, -16802, 103) ->
  (-190, -16848, 176) up a ramp, and the floor-bound walk jumped under
  the top at (-195, -16833, 138) while it still rose 2 units a tick,
  flew past the floor and fell back to 151; the flood's walk, with no
  floor and no jump under it, ran on to the ramp's head, stalled and
  jumped onto it (`nav::tests::a_jump_from_a_ramp_waits_for_its_head`;
  `nav::tests::the_floods_walk_jumps_under_a_top_while_climbing` keeps
  the flood's jump). The re-proof also tries the top's column centre,
  where the flood aimed. VERIFIED (measured, walk traces, UO rules).
- Left: the mast ladder meetings above, and the pit at
  (-1409, 2087, 212) on `mp_depot` (seed 159: the plans dither between
  the pit's exits, a bot bobbing up the slope at (-1280, 2100) to
  (-1280, 2148, 227), a plain edge).

### String-pulling

`Follower::pull` (`nav.rs`) takes the waypoint one node further along
the plan a tick, so a bot crossing open floor heads straight for a node
up to `PULL_STEPS` (3) grid steps off instead of stepping the grid's 45
degree kinks (bot-combat.md section 11). A pull needs all of:

- The body on the ground, off the rungs, past the plan's first node
  (`proved_start` and `jump_start` keep their meaning), no point goal
  walked out, no edge held (`Follower::held`), and not the tick it
  lands (the landing replan reads the edge under way).
- The edge into the waypoint, the edge on and the one after that plain
  (`NavGraph::plain_edge`: no jump, leap, crouched jump or risky edge,
  ends within a step in height) and none of them avoided. So a jump, a
  ladder, a drop or a risky edge is still met along the edge the graph
  proved, from the node it leaves.
- The pulled node within a step of the body's height and the skipped
  node within `PULL_SLACK` (0.75) steps of the line to it: off the line
  the grid went round something.
- `straight_floor`: the standing capsule swept a step up from the body
  to the node clears, and every 16 units along the line a point trace
  a step either side meets ground a body stands on (normal z 0.7 and
  up). Then the walk from the body at its pace and move scale
  (`walk_sliding`, `Gait::Forward`) arrives on the node's floor
  without a jump, dipping no more than a step, never sliding.

While a pulled waypoint is ahead the line to it is checked again each
tick (`straight_floor` alone); a check that fails falls back to the
furthest node it skipped that the body still reaches, else the node it
was pulled from. A failed try or a fall back waits `PULL_RETRY` (4)
ticks. Stuck detection runs on the pulled waypoint as on any other: a
pull resets `best`, and a waypoint not closed on in 40 ticks avoids the
edge into it, which is the plan's own edge from the node before.
Reaching a node, by reach or by passing it, ends the pull
(`a_waypoint_is_pulled_along_a_clear_straight_line`).

VERIFIED (measured, release `ship_stalls`, master d3b03cb0 then this
change with the walking view's turn limit too; bot-combat.md 11.4's
hand-back moves no number here, as nothing fights with fire off): `mp_ship` at 1.1,
seeds 1 to 31 odd, stalled 0 / 0, falls 0 / 0. `mp_cassino` at a stock
UO install (`UO=1`), seeds 1 to 31 odd, stalled 1 / 174, falls 1 / 2;
seeds 33 to 63 odd, 196 / 34 and 0 / 0; together 197 / 208. Each total
is one spell or two. The 172 of seed 23 are a bot held off the ground
in a crease at (426, -958, 60), 5 units from the node (431, -964, 57),
on plain edges into it; no pull ran within 150 units of it all game
(VERIFIED, a temporary log), so the pull only changed which bot went
there. The crease is a parked tank's, fixed under "Parked vehicles"
below. `mp_depot`
without fire, seeds 1 to 31 odd: 0 / 0, and 0 with either half alone.
`nav_census` passes at 1.1 and at UO (both halves are the follower's;
the graph is untouched).

### Parked vehicles

The graph is built round the UO vehicles where they stand at the build
(section 2, the copy keeps their collmap instances), and rebuilt only
when the script's brush links change. A vehicle driven off and left
somewhere else covers nodes the graph still offers.

mp_cassino seed 23's crease. VERIFIED (measured, release `ship_stalls`
`UO=1`, dm, master 3135508b, the trace and position tests on the
server's world at tick 2972): bot 1 drove the Panzer IV (entity 181) to
(450.0, -1099.0, 10.5), yaw -79.6, was out of it by tick 2660 on the
terrace at (364, -962, 81), and walked the stair down to the street through the
nodes (431, -964, 57) and (482, -1011, 35). The tank's collmap,
`script_vehicle_collmap` *28 (brushes 8803-8810, x -136..124,
y -72..72, z 0..128), reaches the stair with its rear. From tick 2759
the body stepped up against the rear on every grounded tick and fell
back; the follower's stuck count avoided the edges one by one and,
with no way round left, took them again (a temporary log of its
plans). From tick 2968 it hung at
(425.69, -957.52, 60.13) with velocity 0, off the ground: its capsule
touches the stair's `wood@barnboard1rd` face, brush 8730 (normal z
0.65, under `MIN_WALK_NORMAL`), under and north of it and the collmap's
brush 8807 (`clip_nosight_metal`, normal (-0.18, 0.97, 0.14)) south of
it. Forward at every 45 degrees, or with the jump key, it moves 0.0 in
20 ticks. `Bot::wedged`'s kill took it at tick 3069. The same spot with
no tank there holds nothing: a body left at rest there lands, and the
walks from it to every neighbouring node arrive. Retail has no bots and
the wedge is not measured on a retail server.

`Server::park_vehicles` (`server/bot_vehicles.rs`) watches each live
vehicle's pose. One that holds it within a unit and a degree for 20
ticks (`PARK_TICKS`) is parked: the nodes within 192 units of its origin
(`PARK_REACH`) where a standing body no longer fits, one at a time
(`NavGraph::blocked_near`, the position test the flood's nodes passed),
are blocked. A parked vehicle shoved under 48 units (`PARK_SHOVE`) keeps
its nodes until it parks again; one moved further, or freed, lets them
go. VERIFIED (measured, seed 1, a temporary log): the tank at the
stair's foot parked again at (439.7, -1087.4), (436.7, -1088.1) and
(446.4, -1088.5); with the nodes let go at each move, seed 1 stalled
102 bot-ticks on the terrace and the stair, and 0 with them kept. The
union, sorted, goes to every follower each tick
(`Follower::blocked`):

- A plan runs through no blocked node but its own destination
  (`Search::blocking`). A destination that is blocked, as a boarding's
  (bot-objectives.md 10.2), becomes the clear node nearest it within six
  grid steps that the start reaches (`NavGraph::clear_near`), and a point
  goal then walks at its point from there. Without it, seeds 1 to 63
  odd stalled 281 bot-ticks against master's 208, and seeds 1, 11, 23,
  41, 47, 49, 55 and 57 went from 279 to 0 with it (VERIFIED, measured).
  INFERRED (the stall lines, bots with no waypoint near a parked
  vehicle): most were a boarding's plan that never found its way.
- A path with a blocked node ahead, short of its end, is planned again
  from a node the body walks to. A start node that is blocked plans from
  `walkable_start`.
- A plan that finds no way round tries again round the stuck edges'
  avoidance dropped (as before), then through the blocked nodes, unless
  the start reached fewer than 64 nodes round them (`PEN_NODES`): then
  the bot is hemmed in (`Follower::penned`), gets no plan, and reads as
  stranded, so `Bot::wedged` sends `kill` after 10 s as for a dead end
  (section 3, "mp_hurtgen's crease"). VERIFIED (measured, the test
  below): the terrace at (383, -916, 76) reaches 7 nodes with the tank
  parked as above, of the 7 676 it reaches without. VERIFIED (measured,
  the traces): planned through, seed 1 stalled 402 bot-ticks on that
  terrace and stair; with no plan and no kill, seed 9's bot, out of a
  vehicle at (1384, -2659, 5), had no waypoint for 14 s and stalled 84.
  With the kill, both 0.

`nav::tests::a_plan_runs_round_the_nodes_a_parked_vehicle_covers`,
`nav::tests::a_tank_parked_over_cassinos_stair_blocks_its_nodes` (the
collmap put at seed 23's pose, yaw only; the stair node (431, -964, 57)
blocked, the terrace not, hemmed in).

VERIFIED (measured, release `ship_stalls`, 6 bots, 4000 ticks, master
3135508b then this change; stalled bot-ticks / falls):

| map | gametype | seeds | master | parked vehicles |
|---|---|---|---|---|
| `mp_cassino` (UO) | dm | 1-31 odd | 174 / 2 | 22 / 0 |
| `mp_cassino` (UO) | dm | 33-63 odd | 34 / 0 | 7 / 0 |
| `mp_cassino` (UO) | tdm | 1-31 odd | 54 / 0 | 74 / 0 |
| `mp_ship` (1.1) | dm | 1-31 odd | 0 / 0 | 0 / 0 |

Seed 23 stalls 0. Of the dm stalls left, seed 59's 4 are a driver in
the seat (`ship_stalls` prints the seat since this change), seed 43's 3
a bot on foot, and seed 15's 22 a bot on foot that turned between two
nodes beside a crewed tank at (2545, -2020) for 10 s (VERIFIED, the
trace). INFERRED (the riders near): walking up to board it. Every tdm
stall is a driver in the seat,
which the driver's route owns, not the walk's follower. Deaths with fire
off over the 32 dm seeds went from 53 to 61, the hemmed-in kills among
them. The graph is untouched: no build or digest moves.

### The 2026-10-21 recheck

The 1.1 tables above are tied to the commits they measured. This round
ran them again on master 028799bf (past botrng, shipedges, jumpcue and
the UO pins). VERIFIED (measured, release `ship_stalls`, `dm`, 6 bots,
4000 ticks, fire off; ours only; stalled bot-ticks / falls / deaths):

| map | seeds | master 028799bf | the fall off the path past its reach | the leftovers below |
|---|---|---|---|---|
| `mp_ship` | 1-1535 odd | 78 / 1 / 1 | 43 / 1 / 1 | 41 / 4 / 1 |
| `mp_dawnville` | 1-383 odd | 0 / 2 / 0 | 0 / 2 / 0 | 0 / 1 / 0 |
| `mp_depot` | 1-767 odd | 8 / 5 / 0 | 8 / 5 / 0 | 0 / 2 / 0 |
| `mp_cassino` (UO, `tdm`, 2400 ticks) | 1-63 odd | 31 / 0 / 22 | 31 / 0 / 22 | 70 / 0 / 18 |
| `mp_uo_stanjel` (UO, `tdm`, 2400 ticks) | 1-63 odd | 28 / 1 / 0 | 28 / 1 / 0 | 0 / 1 / 0 |

- The unstick falls the lip walk left are gone on master: `mp_depot`
  seed 663 and `mp_ship` seed 225 stall 0 with no fall, and no
  `mp_uo_stanjel` spell leaves (276, 3388, -390). No fall in the 384
  `mp_depot` seeds ends in the gutter at (-2028, -451, 344); its five
  are spells off the pit's rim at (-1252, 2096, 227) (seeds 529, 639)
  and (-1056, 2098, 227) (677) and off the roof at 372 (437, 483). So
  the unstick spell is unchanged. With fire on (`bot_match` `dm` seeds
  1..=8, 12 000 ticks) `mp_depot`'s `stalled` read 485 with `held` taken
  per tick and 0 with the window rule (bot-combat.md section 1): every
  one of the 485 came within 12 s of a fight or a cover hold, no nav
  stall among them. VERIFIED (measured, `ship_stalls` `SHOOT=1
  TICKS=12000` on the same matches, seeds 3-17 odd, its `ago` column).
- `mp_ship` seeds 125, 441 and 491 (21, 10 and 4): a bot up the ladder
  at x 7232 off the hull floor at 8 came onto its head (7232, 261, 142)
  and walked the edge to the ledge node (7260, 225, 166). It reached the
  ledge at (7249, 244, 166), left it at (7256, 237) onto the roof that
  slopes down to y 355, slid off it and fell to the hull floor at -64.
  Still in the air, more than `OFF_PATH_STEPS` grid steps from its
  waypoint, the follower dropped the plan but avoided nothing, so the
  next plan, from the ladder at (7287, 340) by the floor, took the same
  ladder and the same edge: four falls in 13 s in seed 125 (VERIFIED,
  `TRACE=1:1380:1800`). Now a body that far off its path and more than
  `FELL_BELOW` under its waypoint, off a climb, avoids the edge under way
  and plans from a node it walks to, as a landing under the edge already
  did (`Follower::waypoint`,
  `nav::tests::a_fall_off_the_path_past_its_reach_plans_round_the_edge`).
  In seed 125 the next plan took the ledge node by its other edge, from
  (7225, 253, 150), fell off the same way onto the roof at 104, avoided
  that edge too on the landing, and went west along the hull floor
  (VERIFIED, the trace). The three seeds stall 0, and every other seed
  in the table keeps master's stall, fall and death counts. Why the
  bot's walk on leaves the ledge where the graph's walks from the head
  nodes did not is not traced.
- Left on `mp_ship` (seed: stalled bot-ticks): the mast ladder's head at
  (4954-4964, 35-64, 1219) with another bot 34 and 177 units off (1233:
  11, 307: 9), the meetings "The jump cue and the re-proof" lists; the
  head of the ladder at x 3696 from 883 to 992 (233: 5, 1031: 4, 1227:
  4); the stairwell over the drop to (5404, 354, 204), from (5451, 350,
  240) (1275: 10). Seed 661's fall is an unstick spell from (6198, -67,
  108) to 56; seed 969's death at (4973, -33, 616) is under the mast.
  `mp_dawnville`'s falls are spells off (-1200, -16934, 97) and (-1641,
  -18304, 94), about 50 down. Traced and fixed in "The recheck's
  leftovers" below, which the last column measures.

### The recheck's leftovers

Each leftover above traced per tick (`ship_stalls` `TRACE=`, with a
temporary per-tick log of the bot's view and cmd), on the recheck's last
column, release, 1.1:

- The mast hatch at x 3696 (seeds 233, 1031, 1227). Off the rungs at
  (3704.9, 57.9, 992) bound for the exit node (3705, 85, 992) and on to
  (3648, 71, 994), the bot read the exit as passed 27 short of it: nearer
  the next node than the exit is (`Follower::follow`, the pass loop's
  `past`). Headed straight for (3648, 71), it crossed the hatch at
  (3687, 65), fell to the deck at 760 and climbed again (seed 1031, t2456
  to t2475, then again from t2540). `past` now needs floor all the way to
  the next node (`straight_floor`, the string-pull's probe). Seeds 233,
  1031 and 1227 stall 0;
  `nav::tests::a_walk_on_that_falls_at_the_weapons_scale_is_not_taken`
  holds the exit from the same spot. VERIFIED (trace, seed 1031; the
  table below).
- The crow's nest's risky drops (seed 969's death; part of seeds 1233
  and 307). Its head nodes (4964, 36, 1219) and (4964, 93, 1219) each
  have a risky edge down the hatch to (4929, 68, 692). After a meeting
  at the head the ladder edge (4964, 64, 1219) -> (4959, 64, 692) was
  avoided, and the plan round it took the risky drop, which `RISK_COST`
  only made dear. In seed 969 bot 5, bound from (4964, 93) for (4964, 36)
  and the drop, took `unpin_yaw`'s heading of -126 degrees at t2623, crossed
  the hatch at (4957, 59), fell past the rungs to 616 and died. A plan
  that avoids stuck edges now takes no risky edge (`Search::run`); when
  none goes round, the plan through the stuck edge comes next, as before
  (`nav::tests::a_plan_round_a_stuck_edge_takes_no_risky_drop`). Seed 969
  has no death. VERIFIED (traces, seeds 969 and 1233, whose bot turned for
  the drop at t3414; the graph's edges, `nav_build` `EDGES=1`).
- The stairwell's jump drop (seed 1275). (5402, 321, 344) -> (5404, 354,
  204) is a jump edge, 140 down, which only a forward walk proves
  (`walk_sliding` arms the jump for no other gait). `Bot::steer` backed
  every waypoint more than `BACK_DOWN_HEIGHT` under the feet that is no
  run off, so the bot crept backwards at it, the jump cue fired while it
  backed (cmd forward -15, up 127, t3045), and the jump put it on the
  stair at 378, down which it slid to (5514, 360, 204). Running back west
  for the node it hopped in the crease at (5451, 350, 240) until the stall
  (t2997 to t3093). A jump edge is now never backed. Seed 1275 stalls 0.
  VERIFIED (trace, seed 1275).
- The spar off the crow's nest (seed 307, and seed 1233's first trip).
  Bound for the spar at (4888, 65, 1212), the bot came onto the rim node
  (4929, 61, 1254) at the pace its jump up from the head left it; the walk
  on from there missed the spar (`edge_holds`), so the edge was avoided
  and the plan reset, and the reset dropped the roam's destination. The
  next roam went back down the ladder and met the next bot coming up
  (t312). Now a drop more than a step down whose walk on from rest on
  the node arrives is taken from rest there, as `Follower::flings` takes
  its drops; from rest the bot walks onto the spar (t316). Holding for
  any failed edge brought a bot to rest on the hull rib at (2214, -325,
  -48) and its next plan's jump edge, which the graph proved with a
  running start, slid it into the crease at (2189, -292, -45) (seed 603,
  22 stalled bot-ticks); holding for a run off a rim stopped bots at each
  of `mp_depot`'s pit drops at 212 in turn (seed 149, 36). Neither is
  held now. VERIFIED (traces, seeds 307, 603 and 149; 603 and 149
  measured with each change switched off in turn).
- The slope at (-1193, -16944, 102) on `mp_dawnville` (seeds 239, 313).
  Up the slope the body passed the node 12 under it, 4 units off the
  line, and `edge_holds` took the walk on as proved: it exempted any body
  more than `ARRIVE` under the node, meant for one on a ladder under its
  head. From there the walk on to (-1193, -16989, 119) pins
  (`Walked::Fell`, a test walk from the traced spots), as the bot did, at
  (-1199.6, -16934.8, 97.9) for 18 ticks; the unstick spell then walked it
  53 down the slope, which the detector counts as a fall. The exemption
  is now for a body on the rungs only (`edge_holds`' `on_rungs`). With
  this change alone on the recheck's tip, seeds 239 and 313 fall 0 and
  the bot plans round the edge from the node (t1071). VERIFIED (traces,
  seed 239 on the tip and with the change).
- The crate at (6207, -90, 80) (seed 661). Run into at 189 units/s on
  the jump edge (6207, -58, 56) -> (6207, -90, 80), the body slid west
  along the crate's face at 52 units/s. INFERRED (that slide): the face
  stands about 15 degrees off square to the edge. VERIFIED (trace, seed
  661): the
  bot read the tick as pinned (its pace toward the top was 0.8 a tick)
  and jumped at (6200, -60), where the walk, which reads a tick pinned off
  the distance moved (2.6), slid on to the corner. It came down on the
  80 crate's edge at (6189, -67), jumped again onto the 108 crate beside
  it, stalled there and the spell ran it off the far side to 56. On the
  ground a tick is now pinned only when the body also moved under the
  pinned pace (`Bot::think`); the bot slides to (6199, -61), jumps there
  and lands on the crate (t156). VERIFIED (measured, the seed sets, and
  traces of seeds 409 and 905): read off the distance moved in the air
  too, as the walk does everywhere, bots sliding on the hull's curve
  toward (2176, -319, -32) never jumped, and the hull crease at (2189,
  -292, -45) held bots in 2 to 16 of the 768 seeds (80 to 1683 stalled
  bot-ticks) as the other changes varied, in none on the recheck's tip.

VERIFIED (measured, the recheck's detector and seed sets, base the
recheck's tip; stalled bot-ticks / falls / deaths, `seated` the stalled
ticks in a UO vehicle's seat, which `ship_stalls` now prints):

| map | the recheck's tip | the leftovers fixed |
|---|---|---|
| `mp_ship` 1-1535 odd | 43 / 1 / 1 | 41 / 4 / 1 |
| `mp_dawnville` 1-383 odd | 0 / 2 / 0 | 0 / 1 / 0 |
| `mp_depot` 1-767 odd | 8 / 5 / 0 | 0 / 2 / 0 |
| `mp_cassino` (UO, `tdm`, 2400) 1-63 odd | 31 / 0 / 21, 31 seated | 70 / 0 / 18, 70 seated |
| `mp_uo_stanjel` (UO, `tdm`, 2400) 1-63 odd | 28 / 1 / 0 | 0 / 1 / 0 |

The recheck's tip gave 21 deaths on `mp_cassino` here, the table above
22. VERIFIED (the same runs): every leftover the recheck listed is gone: `mp_ship` seeds 233, 307,
661, 969, 1031, 1227, 1233's mast stall and 1275, `mp_dawnville` 239 and
313. So are `mp_depot` 641's 8 (the pit at (-1432, 2083, 212)) and its
falls in 437, 483, 639 and 677, and `mp_uo_stanjel` 21's 10 and 41's 18
at (-629, 956, 352) and (-767, 996, 352). INFERRED (the pin rule alone
among the changes, each switched off in turn, brings them back; not
traced): the stanjel bots jumped off their pace. Seeds that changed for
the worse:

- `mp_ship` 1233 (28) and 1521 (10, 2 falls): the stairwell. Two bots
  pressed at its top at (5343, 333, 352) for 100 ticks, then the forward
  run off the drop to (5404, 354, 204) jumped at the rail and came down
  in the crease at (5445, 355, 292) (1233, t2082 to t2148). VERIFIED
  (trace, seed 1233; 1521 not traced). INFERRED: the bot's run differs
  from the walk's, which starts on the node.
- `mp_ship` 1381's death: two bots met head-on on the spar at
  (4676, 64, 1172); the stall's unstick spell headed along the spar a few
  degrees off it and ran off its side to 535. VERIFIED (trace). INFERRED
  (the spar's traffic): more bots reach the spar since the rim's drop is
  taken from rest.
- `mp_ship` 991: a bot ran into another passing at (3789, -207, 352)
  and stood against it; the stall's spell walked 50 down the dome's
  slope to 303, which the detector counts as a fall. VERIFIED (trace).
  797 falls from the same spot, not traced. 1345 (3) at (4195, 504, 83),
  not traced.
- Not traced: `mp_dawnville` 243, a spell off (1208, -15099, -23) to
  -73; `mp_depot` 249, a spell off the pit's rim at (-1251, 2095, 227),
  where 529 already fell on the recheck's tip.
- `mp_cassino` 19 (61) and 31 (9) are drivers in the seat
  (`seated`); no seed has a stall on foot, as before. VERIFIED (the
  same runs).

VERIFIED (measured on the same sets): tried and dropped, pinned off
the distance moved in the air too (above), and the follower's walk on
along a jump edge run as a bot (`as_a_bot`: no room test, the bob) on
top of the kept rule, which cleared seed 1233's stairwell but trapped
seed 113 in the hull crease (`mp_ship` 97 / 2 / 1). VERIFIED (traces,
seeds 307 and 661): keeping the roam's destination when an edge is
avoided at the rim cleared seed 307, but seed 661's bot replanned for
the next drop at (2560, -127, 385) and fell off the hatch's lip there to
its death; dropped too.

## 4. Memory and hearing

Two goals sit between a visible enemy and `Roam`, both kept by the brain
(`Bot::remember`, `Bot::recall_goal`) from what `BotView` carries.

- Memory. Every tick an enemy is visible, its chest point pushed
  `MEMORY_LEAD_S` (0.5 s) along its velocity (`EnemyView::velocity`) is
  remembered. Out of sight the spot stays a goal for `Skill::memory_ticks`
  (100, 5 s) and is dropped once the bot stands within 64 units of it
  horizontally and 96 vertically. The memory outlives the target's identity
  (`forget_ticks`, 1 s): an enemy back in sight within another
  `forget_ticks` of being dropped is taken again on a third of the
  reaction delay, after that as a new target with the full delay
  (`bot-combat.md` 8).
- Pre-aim. While a remembered spot within `SHOOT_RANGE` is a goal and the
  waypoint is level, the view turns toward the spot at `turn_deg` per tick
  and the move keys are rotated so the body keeps the path. Ladder and ledge
  waypoints keep the path's own view. A UO bot with the breath to sprint a
  straight of `SPRINT_ROOM` or more that ends short of the spot runs it
  looking along the path and sprints: the rotated keys leave the line, and
  in a game of running fights (`uo_bots_fight_and_sprint_in_tdm` seed 19
  on c8c3fd1f) pre-aim took about 330 bot-ticks with the room to sprint
  and left 64 sprinted leg-ticks in 90 s (VERIFIED, measured).
- Hearing. The server records a noise for every `EV_FIRE_WEAPON` shot in the
  per-cmd attack pass (grenade throws and melee excluded), for every blast
  the missile pass sets off, chest high at the shooter or the blast, with the
  client it belongs to, for every round a manned turret fires, at its muzzle,
  the gunner's, and for every script `radiusDamage` (the S&D bomb), chest
  high above its origin, nobody's. `step_bots` takes the list at the top of the next
  tick and hands each bot the loudest one it did not make (`bots::loudest`:
  the smallest distance over range, inside range). No line of sight is
  needed, and no sight cone (`bot-combat.md` 8): a shot behind the bot is
  heard as well as one ahead. Teammates' fire counts: a friend shooting
  means an enemy near him.
- A heard noise is a goal for `Skill::noise_ticks` (200, 10 s, about 2200
  units at run speed) and is replaced by any newer one, but never replaces the
  spot of a seen enemy.

- Answering. On a side, with team play on, a bot hears only the noises
  fewer than `team::ANSWERS` (2) teammates stand nearer, the noise's own
  shooter not counted (`team::answers`, the server's view pass); the
  rest of the team keeps to its roam. VERIFIED (measured, ours,
  `bot_match`, `bot-combat.md` 7): before it, every teammate in range
  went to the same shot, and in a six-a-side `tdm` 72% of bot-ticks had
  two or more teammates within 400 units.

Ranges: `HEAR_GUNFIRE` 2000 and `HEAR_BLAST` 1500 units. VERIFIED
(`soundaliases/iw_sound.csv` in `pak1.pk3`): every rifle, pistol and gun
`weap_*_fire` alias has `dist_max` 7800 (one `airfield` variant of the MP40
has 3000), the grenade throws 350, and `grenade_explode_*` 6000. 7800 covers
most of a stock map and would pull every bot to every fight, so the bot
ranges are a choice that keeps roughly the aliases' 1.3 ratio, not retail data.
