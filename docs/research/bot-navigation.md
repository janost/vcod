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

VERIFIED (measured) with `cargo run --release -p vcod-server --example
nav_build`. "Spawns in one component": spawn points in the strongly
connected component holding the most of them. `tests/nav_census.rs` builds
every map and fails on one under its floor; the floors there are the
table's last spawn column but `mp_depot` 158, plus UO's nineteen maps
(test profile: about 3 minutes, 2300 s of CPU).

The census looks each spawn up from where a body dropped at it lands
(`NavGraph::spawns_in_one_component`), as a bot spawned there stands.
From the spawn's own origin, often 100 units up, `nearest` weighs height
four times and can pick a node on a crate beside it. VERIFIED (measured,
`mp_powcamp`): five spawns at z 104 by (1600-1728, 4344-4624) land on the
floor at z 0.125, in the main component, but their origins' nearest nodes
were on a jump-added platform at z 64-94, an island bots leave and never
enter.

Superseded rounds, one line each (VERIFIED, measured):

- 2026-10-06, before the ladder pass: `mp_ship` 87 / 112 spawns in one
  component, `mp_hurtgen` (pitch 64) 178 / 193, `mp_dawnville` 174 / 185,
  `mp_powcamp` 149 / 161, `mp_rocket` 136 / 153.
- 2026-10-07, the ladder pass, BVH rework, brush-and-patch world, hazards,
  walk back, 48 pitch cap, retail facet grid: `mp_ship` 107 / 112,
  `mp_hurtgen` 190 / 193; the facet grid cost `mp_brecourt` two spawns
  (161 to 159) and gave `mp_ship` two (105 to 107), not looked into.

With jumps, leaps, the fall budget, the creep only near ladders, the flood
from the ladders' ends and the sidestep on the walk back (2026-10-07).
"master" is the same census on master 2000b76, whose pmove pinned the walk
back at (11840, 4226) that `SIDESTEP` gets past. Wall times are each map's
best of three interleaved runs at load 10-17 (the same binary's CPU time
over twelve maps ran 102 to 215 s across runs); pmove ticks (a temporary
counter in `Sim::tick`) do not depend on load: 6.10 million before, 6.39
million after (+4.7%, before the sidestep).

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

`mp_ship` costs most: its hull below the deck, reached down the stair the
fall budget lets a walk land on, is 2 900 more nodes.

Later changes to the counts, each with `nav_census` passing at the floors
(VERIFIED, measured):

- Perched ladder heads (2026-10-09; "Ladders"): spawns unchanged; nodes /
  edges move only where a walk came to rest perched: `mp_depot` 11451 /
  80007 to 11438 / 79752, `mp_powcamp` 6226 / 43140 to 6222 / 43105,
  `mp_railyard` 12109 / 87257 to 12104 / 87212, `mp_ship` 14147 / 96389
  to 14139 / 96255.
- No leap from a ladder head (2026-10-10; "Jumps"): `mp_ship` 14139 /
  96255 to 14139 / 96254, leaps 138 to 137 (other maps not counted).
- The capsule leaf gating (bsp-ibsp59-format.md, "The leaf walk") moved
  `mp_carentan`, `mp_dawnville`, `mp_pavlov`, `mp_powcamp`, `mp_railyard`
  and `mp_ship` by 6 nodes, 36 edges and 2 leaps in all.

Cost. Instructions are `perf stat -e instructions:u` (or
`perf_event_open`) over the whole process, map load included, and do not
depend on load; wall times are noisy under the loads these ran at.
Current, master 65ef830f / with the partition replay stopping at the first
met patch and the leaf walk passing an axial node the sweep stays on one
side of straight to that child (graph `digest` unchanged on all sixteen
maps; VERIFIED, measured):

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
`mp_arnhem` 36.1 / 36.1, `mp_uo_carentan` 35.7 / 35.4. `mp_ship`'s
`cycles:u` 43.6-45.2 G against 44.0-44.8 G, three runs each. Build ms at
the leaf walk round (best of three, load 23-25): 127 (`mp_harbor`) to 614
(`mp_ship`). VERIFIED (measured).

How it got there, one line a round (VERIFIED, measured):

- Early single-threaded `mp_carentan`, before the diagonal shortcut and
  the stall cutoff: 10.6 s, 80% in `CollisionWorld::trace_node`. A debug
  build (test profile) of `mp_carentan` takes about 7 s.
- Binned surface-area BVH split with the static models in their own
  subtree: all twelve builds 288 s of CPU to 184 s, best wall time 1.2x
  (`mp_ship`) to 2.0x (`mp_railyard`) faster; a trace entered 111 BVH
  nodes on `mp_rocket` and 284 on `mp_depot` before, 58 and 85 after.
  Edge counts moved by at most 14 (INFERRED: walks whose contacts tie).
- 2026-10-08 cheaper traces, every count unchanged: instructions roughly
  halved on every map (`mp_hurtgen` 192.7 G to 87.1, `mp_ship` 108.8 to
  50.0, `mp_harbor` 22.2 to 10.9). The second pass of a trace replays the
  leaves the brush pass logged (-29% on `mp_hurtgen`, -39% on `mp_ship`);
  a terrain triangle out of the sweep's reach skips the clip
  (`mp_hurtgen` a further -34%); a patch facet whose axial bevels keep
  the sweep out is skipped (`mp_ship` -25%); a world with no posed model
  skips the poses' `RwLock` (`mp_rocket` CPU 15.7 to 10.0 s). Before,
  `perf record` put `trace_node` at 47% and `Terrain::clip_capsule` 35% on
  `mp_hurtgen`, `trace_node` 31% and `PatchCollide::trace` 26% on
  `mp_ship`. Each held to the old answer by a test (`cod11-mantle.md`,
  "Terrain is a swept sphere, a patch is a facet").
- 2026-10-16 the leaf walk (a sweep that meets a model-0 patch's box is
  clipped again in retail's leaf order; 845 000 of `mp_ship`'s 11.4
  million world traces meet one) took `mp_ship` 105.2 G to 142.8; a
  listing's box cut at the fraction (patch clips 14.1 million to 2.0) and
  a BVH walk after it that enters only subtrees with a prim outside model
  0 took it back to 110.6, digest unchanged.
- Of the met sweeps' cost on `mp_ship`: BVH pass 6.5 G (brush walk 5.65,
  partition replay 0.85), leaf walk 10.0 G (patch clips 4.9, tree descent
  2.9, listings' box tests 1.1, brush clips about 0.5), the walk after it
  1.2 G. Rejected: model 0's patches tested in the brush walk (950 000
  sweeps take the leaf walk, the digest moves on `mp_carentan`,
  `mp_dawnville` and `mp_uo_carentan`, +1.0 G `mp_brecourt`, +3.0 G
  `mp_hurtgen`); a per-node "holds a model-0 patch" BVH flag (127.3 G); a
  per-node listing box on the leaf tree (0.4 of 36 million nodes passed
  over, +2.6 G); `trace_node` without recursion (103.1 G but `cycles:u` 44
  to 49-50 G); ending the BVH pass at the first met patch and keeping the
  walk's stack between walks (nothing measurable).
- Not changed: the flood's layers are a barrier each (walks kept 16
  threads 66% busy on `mp_ship`'s 149 layers, 85-89% on `mp_rocket` and
  `mp_hurtgen`'s 46-54, at load 57); a back down off a ledge away from a
  ladder, about 8% of `mp_hurtgen`'s pmove ticks, stays, since trying it
  only near a ladder or after a forward run that fell more than 64 cost
  `mp_ship` 2 spawns and `mp_chateau` 1.

The server:

- The server builds the graph on the first tick with bots, not at map
  load, and keeps every graph for the life of the process
  (`nav::graph_for`), so a map cycle back to a map, and test gates that
  start one server per test, build it once.
- The binary builds it on a thread of its own (`nav::NavJob`) over a copy
  of the collision taken on that tick, 5-40 ms on the tick thread
  (VERIFIED, measured), so the server keeps its 20 Hz through a build of
  seconds and the bots wander until the graph lands. A `vcod --net-probe`
  on a `mp_rocket` server with two bots read 20 snapshots a second through
  the whole build (VERIFIED, measured). Tests leave
  `Server::build_nav_in_background` off and wait for the graph on that
  tick.
- The copy leaves out the brush models marked `script_noteworthy rubble`
  (`World::rubble`): UO's `bas.gsc` lifts each 1000 units at the start and
  drops it back only over a dead base, and every other gametype deletes
  it. The cache keys on which submodels clip, since each gametype deletes
  the others' brushes and one map's graph differs by gametype. The graph
  follows the script's brush links: once `GameHost::brush_links` (bumped
  by every `delete`, `notSolid` and `solid` that changes a brush model's
  link) has held still 2 s, a new build starts over the collision as it
  stands and replaces the graph when it lands, the bots' paths and zone
  stands dropped (`Server::relink_nav`; bot-objectives.md 7). `bas`
  deletes a breached base's walls, the only way to its charge.
- The copy keeps the mover poses, world copies and vehicles' collmap
  instances where they stand, so a rebuild walks round a parked tank. The
  cache keys on those too (`CollisionWorld::placements`) and keeps the
  last 24 graphs; a graph is a function of its key
  (`nav::tests::the_cache_key_covers_every_placement`). Trap: keyed on the
  links alone, a test got whichever graph another test in the process had
  built round its own vehicles (VERIFIED, measured, UO mp_foy bas, seed 7:
  digest `04c80a17fac01c91` in use against `12e1ab16edbe74f5` fresh).
- A background build's walks leave one core to the tick (`nav::par_map`
  under `SPARE_CORE`). `vcod-server --trace` logs a `tick:` line a second:
  mean and slowest tick and the worst lag of a tick's start behind its
  slot. VERIFIED (measured, release, 4 bots, 16 threads, load 17-31):
  through `mp_ship`'s 2.8 s build and `mp_carentan`'s 1.3 s the slowest
  tick was 9-11 ms and the worst lag under 8 ms; the tick that takes the
  finished graph and plans every bot's first path ran 33 ms once on
  `mp_ship`.

Spawn gaps, explained:

- `mp_ship`'s gap was its upper decks (ladders, closed by the ladder
  pass) and five hull spawns in two islands. VERIFIED (measured,
  2026-10-07, point traces and walks): three on the hull's deck at z
  56-200 (126 nodes, the spawn at (3534, 61, 64) among them), which a
  walk left up the stair at x 3430-3472, y 288 but never entered (a run
  down falls past the steps and its budget ran out in the air, "The
  flood"); two in a lifeboat at x 3968-4250, y -380 to -225, floor z
  394-412, walled by a gunwale about 40 over the floor, deck outside at z
  341-351, which no walk crossed without a jump. A gap pass between
  neighbouring columns added 235 edges and joined none. With the fall
  budget, jumps and the flood from the ladders' ends all 112 are in one
  component; a bot gets into the lifeboat by jumping along the deckhouse
  roof at z 641-664 from the hold ladder's head and dropping in from z
  648.
- `mp_carentan` has no playable ground at x -1180, y 1900-3300, z
  -47.875 (VERIFIED, measured, point traces, 2026-10-07): the map's base
  brush west of a concrete wall (brush 3164,
  `textures/normandy/walls/concrete@dirtywhite_wallwthrd`, solid at z
  2000) from (-1100, 1650) to (-260, 3430), the town east of it. A flood
  seeded there reaches the town only under the terrain, through (389,
  575, -46.7), 37 units under the terrain triangle at z -9.27, which
  `under_ground`'s rays from 18 units up catch ("The flood").
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

United Offensive's census. The first eleven maps are `pakuo00`/`pakuo01`,
the last eight the map pack (`uomappack00`). "Reach it" counts the spawns
whose node reaches the component holding the most spawns
(`NavGraph::spawn_census`): a UO base's spawns mostly stand behind a drop
out of it (mp_cassino's allied base at (192, 1072, 8), 36 spawns of three
classes, reaches the town and is never reached back), which a bot spawned
there leaves by and never needs again. Nodes, edges, ms and leaps are the
2026-10-14 build (VERIFIED, measured, release `nav_build` `UO=1`, `COD_DIR`
at the stock 1.5 `main/` with UO's `uo/`, ms rough under load); the spawn
columns are today's floors in `tests/nav_census.rs` (`UO_FLOORS`), after
the fixes below.

| map | spacing | nodes | edges | ms | in one component | reach it | spawns | leaps |
|---|---|---|---|---|---|---|---|---|
| mp_arnhem | 48 | 7640 | 49915 | 511 | 265 | 332 | 332 | 19 |
| mp_berlin | 48 | 17679 | 123026 | 1833 | 202 | 303 | 303 | 262 |
| mp_cassino | 48 | 8558 | 57393 | 707 | 94 | 180 | 180 | 11 |
| mp_foy | 48 | 62126 | 470101 | 7924 | 386 | 391 | 391 | 178 |
| mp_italy | 48 | 77171 | 590070 | 6130 | 371 | 371 | 371 | 192 |
| mp_kharkov | 48 | 40855 | 297995 | 4649 | 187 | 402 | 402 | 98 |
| mp_kursk | 48 | 65367 | 495439 | 5856 | 273 | 282 | 282 | 95 |
| mp_ponyri | 48 | 59089 | 447994 | 3605 | 265 | 272 | 273 | 61 |
| mp_rhinevalley | 48 | 145710 | 1107889 | 14672 | 362 | 386 | 386 | 737 |
| mp_sicily | 48 | 24301 | 177922 | 2227 | 236 | 241 | 241 | 44 |
| mp_uo_stanjel | 48 | 8762 | 62052 | 724 | 366 | 380 | 380 | 61 |
| mp_peaks | 48 | 8570 | 55656 | 1810 | 113 | 128 | 128 | 170 |
| mp_streets | 32 | 8087 | 53523 | 635 | 180 | 226 | 226 | 23 |
| mp_uo_carentan | 32 | 10227 | 72789 | 755 | 285 | 285 | 285 | 11 |
| mp_uo_dawnville | 48 | 7666 | 52863 | 767 | 195 | 249 | 249 | 5 |
| mp_uo_depot | 32 | 15655 | 109243 | 1638 | 275 | 351 | 351 | 184 |
| mp_uo_harbor | 32 | 7842 | 54432 | 442 | 113 | 292 | 292 | 20 |
| mp_uo_hurtgen | 48 | 27484 | 207848 | 2785 | 282 | 286 | 286 | 522 |
| mp_uo_powcamp | 48 | 6440 | 44020 | 333 | 226 | 258 | 258 | 2 |

The 2026-10-14 build's spawn counts (in one component / reach it), where
they differ: mp_arnhem 231 / 287, mp_cassino 94 / 168, mp_foy 317 / 341,
mp_kursk 258 / 270, mp_ponyri 219 / 242, mp_sicily 236 / 238,
mp_uo_stanjel 366 / 378, mp_peaks 113 / 126, mp_streets 180 / 197,
mp_uo_dawnville 189 / 243, mp_uo_powcamp 226 / 254, mp_uo_hurtgen 281 /
282.

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
- `mp_ponyri`'s 242 read 240 after the UO vehicles merge (a7b99cd6),
  before the later fixes took it to 272. VERIFIED (measured 2026-10-15, release, the
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
hearing, heal, sidestep, in that order per bot), every bot's cover look
and the objective roles over all of them; then per bot the retreat's cover, the hunt's
vantage, the goal, the seat, the follower below, the drop and lob probes
and `think`; then the replies, gamestates and cmds. The follower's rand
draws come off the bot's own generator, so a bot's walk depends on the
plan budget the bots before it in slot order spent that tick, not on
their draws. VERIFIED (`bot_match` and the `bots::` tests byte-identical
before and after the driver moved behind `BotWorld`, 2026-10-23).

CoD2's bots run the same driver in the same order. The route leaves out
what the trait's consts name: no yields, no crowd and no blocked nodes
(`YIELDS`), no plan round another body (`SHUNS_BODIES`, "Bodies in the
way" below), and none of the edge cues that step CoD1's pmove (lip, push
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
  stall, and the defuse comes 886 ticks after the plant. A body in the
  way is planned round after 4 ticks held up now ("Bodies in the way",
  below).

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
onto the leap's foot at its head. With both rules, seeds 1-4 had 7 to 59
stalled bot-ticks, none on a ladder (`tests/bots.rs`,
`bots_on_mp_ships_ladders_keep_climbing`).

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

A random heading is never one with a hazard 64 units along it
(`BotView::hazard_ahead`, one flag per 45-degree octant), nor, standing
on the ground off a ladder, one with a drop deeper than a jump (39) within
64 units before a wall (`BotView::drop_ahead`, `nav::drop_ahead`). The
probe sweeps the standing capsule a step up along the octant, then feels
for the floor every 16 units with the capsule, each floor against the one
before, so a stair down is no drop and a body overhanging a lip is held by
it. It also feels half way between (a point trace, the capsule only where
that finds no floor) and where the sweep stops short of a wall
(2026-10-13d), and, against a wall less than 45 degrees off the heading
within the first 16 units, along the slide
(`nav::tests::a_drop_ahead_is_felt_along_a_wall_it_slides_on`). The server
runs the probe only on the ticks the brain can read it
(`Bot::reads_drops`: no waypoint, a spell, guard, or a stall window's last
tick with the body still in its circle), under 1% of bot-ticks
(2026-10-13f). A heading is picked first with no drop in its octant or
either neighbour (a heading into a wall slides along it), then with none
in its own octant, then off hazards alone. In the first two tiers a
heading whose octant pins the body on a wall within 4 units comes after
every open one (`BotView::wall_ahead`, `nav::wall_ahead`: the capsule
swept a step up, the wall less than 45 degrees off square): the drop
probe reads such a heading as no drop, though it goes nowhere. A
wandering bot picks again when a drop turns up in its heading's octant or
in its velocity's. Two cases take the drop anyway: a waypoint more than a
jump below (the path drops there), and a bot cornered, its third unstick
spell in a row starting within 96 units of the one before (without it, a
bot on the 8-unit rim at (4223, 345, 276) beside the deck 220 below paced
it for 2 248 bot-ticks; VERIFIED, measured). A cornered bot still shuns a
drop past `bg_fallDamageMinHeight` (256): the probe reports the depth.
Engaging an enemy overrides all of it. The deck test
(`bots_on_mp_ship_unstick_without_walking_off_the_deck`) counts a fall
only where the graph's way back is longer than twice the flat distance
plus 128.

A stall inside a spell picks a new heading without restarting the spell,
so the waypoint gets its turn when the spell ends
(`bots::tests::a_pinned_spell_hands_back_to_the_waypoint`), and a spell's
end starts the stall window afresh
(`bots::tests::a_spell_end_starts_the_stall_window_afresh`). Each spell
drops the held creep heading.

**The `mp_ship` stall log.**

The detector is `ship_stalls` (described after the log). A bot-tick counts
as stalled when every position of the bot over the 12 s before lies within
150 units of the first; "falls" are unstick spells that start on the
ground off a ladder and end 48 or more below. A run diverges from its
first changed waypoint, so a seed's count moves by chance as well as by
cause, and a 32-seed total by tens. Each round below traced its stalls
tick by tick (`TRACE=`), fixed the cause, and re-measured on the same
settings (`mp_ship` `dm`, 6 bots, shoot off, 4000 ticks, release, unless
noted). Every row VERIFIED (measured). The current numbers are the
2026-10-21 recheck at the end of this section.

- 2026-10-09, the hull beam: a flood edge merged onto the beam's slope
  edge at -47.875 under the top node (2172.9, 418.2, -31.875) led bots
  there. Node floors within 8 and the drop pass (section 2, "The flood"):
  seeds 1-15 odd 160 to 153 stalled, the beam area (x 2080-2280, y
  350-560) 93 to 4.
- 2026-10-09, three pass rules: the jump from the deck at 616 to a ledge
  at 652 by (3360, -90) passed its top in the air and fell back; a bot on
  a hull beam's edge ledge slid back off the slope; bots passed the mast
  ladder's head at 992 from 25 under it and let go of the rungs. A jump's
  top and a ladder's head are passed only from the ground; seeds 1-15
  odd 375 to 133, 17-31 odd 172 to 27.
- 2026-10-09, the ladders (`climb_yaw`, the climb's line, the head passed
  on the ground): the mast hatch (plate x 3711-3712, y 43-69, z 760-992)
  dropped bots down the shaft to 760; a flood foot at (3814, 33, 304) 20
  south of the rungs at x 3792 pinned the climb under the deck; the
  ladder at x 3353-3354, y 496-520 (head (3323, 481) south-west of foot
  (3370, 508)) grabbed at the edge y 495 and pinned at z 281.9 under a
  spar; a climb strafing to the mast's middle (y 56) landed where the walk
  west falls, hence the foot's line. Seeds 1-63 odd 210 to 50, 65-127 odd
  274 to 265; `mp_depot` seeds 1-11 176 to 106, `mp_pavlov` 117 to 47.
- 2026-10-09, drops shunned: before, a bot tipped off the ladder at
  (3280, -460) took the unstick heading off the deck's lip to the hull.
  Drops never taken (no cornered case): stalled 2 284 and 3 974 on two
  seed sets; shipped: 44 and 58 (from 138 and 28), falls 40 to 19.
  Trap: a cornered bot at the ladder at x 2206-2230 ran west off the floor
  at 216 to the hull at -64, 280 down; hence the 256 bound.
- 2026-10-09, perched heads (section 2, "Ladders"), the hull corridor and
  the slide probe: 514 to 463 stalled over seeds 1-191 odd; mast spells 16
  to 0, hull 6 to 0, hold 0 to 4. The hull corridor (seed 95): hull beam
  patches 104 and 106 along y -800, 22 high; patch 106's end stands at x
  3840 for y -802 but clears a body at y -805. The bot passed the node
  (3837.1, -804.9) 3 north of it and pinned on the beam's end at x
  3824.9; from there the walk arrives only by jumping, and
  `Follower::edge_holds` took any arrival as proof. A walk that needed a
  jump proves only a jump edge
  (`nav::tests::an_edge_that_needs_a_jump_from_aside_does_not_hold`).
  The slide (seed 5): a bot at (6095.8, -303.9, 97.1) against brush 316
  (normal (-0.98, 0.21)) slid off a ledge 41 to the deck at 56; the probe
  due south read nothing, now 41.
- 2026-10-10, the hold ladder's head (seed 149): the head (4332, -87.1,
  615.1) stands on the ladder brush's lip (y -80 to -72) over the shaft
  (down to 56 from y -80 to -130), the floor at 600 round it. The next
  edge, the leap to the ledge (4322.3, -158.1, 641.1), arrives from the
  head node but falls from where the climb tipped the bot, (4333.2,
  -91.5); backing down, a second bot's box on the floor at (4347.7,
  -110.5, 600) slid the creep south-west at 0.35 a tick; the spell's
  heading went into the wall and, restarted every 10 ticks, never ended.
  Fixes: a ladder's head is no leap's foot (section 2, "Jumps"), headings
  into a wall rank last, the spell change. Seeds 1-383 odd 798 to 684,
  hold spells 4 to 0. Trap: without the spell change seed 55's bot at
  (3360.1, 496.1, 352.1) spent 175 ticks in one chained spell.
- 2026-10-11, four mechanisms: the ladder at x 3808 (y 59-67, z 340-556,
  seed 357), a climb met a bot coming down at z 511 and its spells walked
  it down the rungs because the stall window straddled each spell's turn
  (climb rate 2.67 a tick both ways), hence the window reset at a spell's
  end; the ledge at (3347, 250, 616) (seed 341), the bot passed the jump
  top in the air 14.9 under and 22.7 off it (`JUMP_PASS` bounds only the
  height), hence `a_jumps_top_is_passed_only_on_the_ground`; the hold
  ladder's floor at 304 (seed 11, 2 292 bot-ticks), a back down passed the
  node (4318.5, -96, 304.1) from 42 over it and hung 0.6 under the lip at
  x 4328.5 where the strafe projects to nothing, hence
  `a_node_below_the_rungs_is_passed_only_from_its_floor` and
  `a_back_down_slides_along_the_face_onto_a_floor_beside_the_ladder`; the
  lip at (5424, -187, 344) (seed 59), a bot overshot the shelf node
  (5383.6, -161.1, 396.1) onto a slope lip (brush 2194) over the stair
  (brush 790) and `nearest` planned from a node it could not reach, hence
  a node more than a step below passed only on the ground,
  `edge_holds` from over a node's floor,
  `a_follower_that_fell_off_its_path_plans_again` and
  `a_plan_starts_at_a_node_the_body_walks_to`
  (`NavGraph::nearest_walkable`). Seeds 1-383 odd 684 to 620 (falls 376
  to 338), 385-767 odd 493 to 567 (falls 342 to 264); the x 3808, ledge
  and shelf spells 9 / 7 / 3 to 2 / 0 / 6 and 1 / 4 / 2 to 0 / 0 / 4.
- 2026-10-12, the shelf slot, the corner and the rungs: the drop from the
  shelf node (5402.6, -160.2, 396.1) to (5404.9, -192, 204.1) falls
  through a slot between the stair's top and the shelf; the forward walk
  arrives from within 8 of the node, the backward and creep walks fall, so
  a bot backing onto it missed onto the stair. The pass now re-proves the
  drop at the body's velocity and holds the node while that fails, and a
  drop the forward walk ran off is run off
  (`a_drop_is_proved_again_at_the_pace_the_bot_comes_onto_it`). At the
  corner (5375.1, -193.1, 352.1), against brush 2194 (normal (0, -1, 0)),
  the edge to (5344.4, -156.1, 352.1) moves a body at rest 0.003 a tick;
  avoided with no way round, the plan took it again and the pass dropped
  it again every tick, so an edge planned while avoided is now taken as
  it is. On the ladder at x 3354 (seed 643) and at (3369, -379) (seed 275,
  803 bot-ticks) the nearest node was a floor behind the climber's back,
  hence `a_plan_from_the_rungs_starts_at_a_ladders_end`. Seeds 1-767 odd
  1 187 to 1 004 stalled, spells 87 to 67, shelf spells 10 to 0. The
  roam pool (above) took `mp_depot` seeds 1-15 odd 642 to 37. Trap,
  backed out: the walk on at the bot's own weapon scale, a drop's walk 2
  either side, and a slow onto a node whose walk fails at pace took
  stalled 559 to 2 907, nearly all at the ladder at (2150, -214).
- 2026-10-12b, the push-off, the unpin and the slope: at the ladder at
  (2150, -214) (foot (2160, -215, 56), head (2160, -156.1, 216), rungs
  facing north), a back down from the head hung at z 102.8, 1.3 under the
  floor at 104 to the south and 16 in front of it, a body that no ladder
  move takes off the rungs; the push-off jump lands on it
  (`a_push_off_lands_on_the_floor_behind_the_rungs`; point traces at x
  2146.5 find 56 under y -230 to -250 and -64 from y -260: the floor holds
  the capsule by an edge). At the corners at x 5375.1 ((5375.1, 321.1) and
  (5375.1, -193.1), brush 2100, normal (0, 1, 0)) a body at rest moves
  under 0.02 in 4 ticks at every heading 195-240 or 90-170 and 30 due west;
  the pinned bot now runs aside (`a_body_pinned_in_a_crease_runs_aside`,
  the compass axes added to the 10-degree steps, which missed at 179.7).
  VERIFIED (retail, `client-probes/probe_shipcrease` on `cod_lnxded` 1.1d,
  `probe_fall` client with `--probe-fall-walk`, 2026-10-18): from rest in
  the north corner retail runs free at yaw 200 and 225 to (5343.12,
  227.13) and at 140 to (5343.12, 332.87); in the south one it stays at x
  5375.1 at 140 and runs at 200 and 225 to (5343.12, -204.87). The north
  pin was ours only through the BVH's order (the capsule stops at fraction
  0 on brushes 2100 and 2101 at once); since the leaf walk names 2101
  (bsp-ibsp59-format.md, "The leaf walk") ours matches all six runs. The
  south crease is retail's. The slope: an unstick spell ran down the
  hull's slope (49 in 75, no 16-unit sample a jump under the one before)
  into a wedge at (3752.1, 413.7, 303.4), so a floor more than a jump
  under the body anywhere within the look reads as a drop
  (`a_drop_ahead_is_felt_down_a_steep_slope`). Seeds 1-767 odd 1 004 to
  409 stalled, spells 67 to 40, corner falls 116 to 1. `mp_depot` seeds
  1-63 odd 2 620 to 62.
- 2026-10-12c, mp_hurtgen's crease, mp_ship's boat: on `mp_hurtgen` (seed
  43, 3 730 bot-ticks in one run) a bot came to rest in the air at
  (436.8, 2824.3, -97.2) (ground normal z 0.684, under `MIN_WALK_NORMAL`),
  touching terrain triangle 25398 0.05 under its feet, partitions 1005 and
  1007 4 to either side and brush 1747 (`clip_nosight`) 4 north; no key
  moves it. VERIFIED (retail, `client-probes/probe_wedge` on `cod_lnxded`
  1.1d, `probe_fall` client, 2026-10-12): respawned at rest there retail
  settles at (436.77, 2824.30, -97.26) (ours 436.769, 2824.3, -97.261) and
  stays 2 s standing and 2 s holding forward at yaw 45; respawned 17
  above, or at (430, 2810.4, -87.5) or (416.7, 2799, -60), it slides in
  and stays; with a running start it leaves at yaw 45. The crease is
  retail's; a player there has `kill`. A bot held off the ground, off the
  rungs and unlinked within a unit of one spot for 100 ticks sends `kill`
  as a client command (`Bot::wedged`), once per 100 ticks, not on an `sd`
  or `re` level (`bot-objectives.md` section 3;
  `bots::tests::a_body_held_in_the_air_asks_for_a_kill`, `tests/bots.rs`
  `a_bot_held_in_mp_hurtgens_crease_kills_itself`). The boat (seed 297):
  a spell ran a bot off the deck at (5209, -352, 375) into a dish of
  patches 122 and 124 (floor z 5 to 29) between x 4995 and 5350, y -440
  and -590, walled from the deck at 56, its way out over the south rim to
  -64; every plan started at a deck node beyond the wall. A plan from a
  body whose nearest node is more than a step off its height walks the
  nodes below too (`NavGraph::off_floor`,
  `a_body_in_a_pit_off_the_graph_plans_from_the_floor_over_its_rim`,
  `tests/bots.rs` `a_bot_in_mp_ships_boat_drops_out_over_its_rim`).
  `mp_ship` 409 to 466 over seeds 1-767 odd, inside the divergence;
  `mp_hurtgen` seeds 1-63 odd 3 730 to 0. Trap, backed out: a yield for
  any climber within 96 on an edge rising or falling more than a jump,
  466 to 1 043.
- 2026-10-12d, the crease's way in and the head-on ladder: the follower
  passed (381.7, 2790.8) in the air 12 off and the body slid into the
  crease, so a node is passed only on the ground or on the rungs
  (`a_node_is_not_passed_sliding_past_it_in_the_air`), and a walk that
  slides (`PlayerState::sliding`) arrives only if no spot it slid over
  drains into a crease: a body left at rest there for 40 ticks never lands
  and then stays within a unit for 10
  (`a_slide_over_ground_that_drains_into_a_crease_does_not_hold`; 1 to 75
  edges on nine maps, census unchanged, `mp_hurtgen` 207 996 to 207 921
  edges). Trap, backed out: re-proving a slide with its run turned 45 or
  90 degrees sent bots into the crease on six seeds and its first version
  cut 2 of `mp_chateau`'s spawns. The head-on pair at (3369, -379) (seed
  313, bots 30 apart, `bots_meeting_head_on_at_a_ladder_take_turns`): a
  bot off the rungs within 64 flat of its edge's ladder at its end's
  height, with another player on those rungs within 64 of that height
  moving toward it or still, walks to the end's neighbour farthest from
  the ladder (at least 32 off) and rests there, at most 200 ticks (40 with
  the climber still), then not again for 200 (`Follower::ladder_yield`,
  `a_bot_at_a_ladders_end_waits_aside_for_a_climber`). Later additions:
  the ladder box nearest both ends; at a head taller than a fall that
  kills (480), a climber from 140 under counts, and with no neighbour 32
  clear the spot is two steps off; a bot backing down such a ladder with a
  climber within 140 under climbs back (`Follower::head_on_yield`,
  2026-10-13d); a wait lasts while another body bobs between the spot and
  the rungs, a climber within a jump under the head counts either way, and
  the spot stands 30 clear of every neighbour (2026-10-13f); a bot past the
  end over the lip steps back on first
  (`a_yield_from_over_the_lip_steps_back_onto_the_head_first`, 2026-10-18);
  at a head whose fall does not kill a body pressed on the waiter ends the
  wait (`a_climber_pressed_on_the_waiter_ends_the_wait`, 2026-10-18c); a
  creep off the head of a ladder whose fall kills faces the rungs square
  and strafes to their middle (`nav::Follower::back_face`,
  `BotView::back_face`). Seeds 1-767 odd 466 to 733 (the 294 over the pass
  rule from 10 seeds whose runs part after a wait, no stall with a bot
  within 100); trap: the wait for any climber however far down, 1 670.
  `mp_depot` seeds 1-191 odd 516 to 947, the corner trap at (-433, 1129,
  -26) off the edge to (-484, 1051, -32), fixed in 10-12e.
- 2026-10-12e, four fixes (bisected first: the pass rule raised
  `mp_depot`, the ladder wait `mp_ship`): `mp_depot`'s corner trap, the
  ladder foot (-417, 1130, -32) under the bracket (-459..-448,
  1106..1127, 12..23), whose edges south were proved by a walk that
  climbed the rungs to 135, crossed the roof and dropped back; a walk to a
  floor that holds the rungs more than a jump over the higher of its ends
  proves nothing (`walk_with`'s `rung_top`,
  `a_level_walk_over_a_ladders_top_proves_no_edge`; 2 edges each off
  `mp_depot` and `mp_ship`). The drain test's cmd splits: VERIFIED
  (retail, `client-probes/probe_wedge` on `cod_lnxded` 1.1d with a
  `--probe-fall` client, 2026-10-18) a body at rest at (385.42, 2813.30,
  -29.90) comes to rest in the air at (412.85, 2797.97, -65.12), one at
  (385.52, 2813.39, -29.87) slides into the crease at (436.76, 2824.27,
  -97.24); ours under one 50 ms cmd lets go of the first after
  `PM_SlideMove`'s 0.999 (cod11-mantle.md, "The slide's same-plane test")
  and holds each under a client's 16/17 ms splits, so `nav::drains` asks
  the bot's 50 ms cmds and the three phases of a 60 Hz client's and takes
  a hold under any (18 more of mp_hurtgen's 209 877 edges refused). A
  floor sample on a plane steeper than normal z 0.7 whose spot drains
  reads as a drop that hurts, cached per 8-unit cell (`nav::Creases`;
  1.7 ms where it drains, 9 us where it lands;
  `a_slope_draining_into_a_crease_reads_as_a_drop_that_hurts`). The pass
  no longer cuts past a node whose next node is more than a jump below
  (`a_drops_top_is_not_cut_past`; seed 693 at the head (3298, -380, 352)).
  A bot whose nearest node has no edge out for 200 ticks asks for a kill,
  not in a one-life round (`NavGraph::dead_end`, `BotView::stranded`,
  `a_body_stranded_off_the_graph_asks_for_a_kill`; `mp_ship`'s bow
  gutters at y -152 and 281, z 312, and (4058, -195, 800)). Stalled /
  deaths: `mp_depot` 947 / 8 to 158 / 9, `mp_ship` 733 / 332 to 304 /
  254, `mp_hurtgen` 0 / 6 to 0 / 4.
- 2026-10-12h: `mp_hurtgen`'s deaths at (5750-6030, -190 to 74, 128) were
  `_minefields.gsc`'s `istouching` reading minefield `*2`'s box; retail's
  `isTouching` runs `trap_EntityContact` against the brushes, and so does
  ours (`trigger::entity_contact`, `cod11-gsc-object-model.md` 22.2).
  `mp_depot`'s roof falls at the 1-unit ladder plate at x -2750 (y -1229
  to -1205, z 336-504): a creep over the plate's top at 0.7 a tick read as
  stuck and the spell ran off the roof, licensed by a waypoint more than
  a jump below; a creeping body is stuck only inside 2 units in ten ticks
  (`a_creep_is_not_stuck_until_it_stops`) and a path down licenses no
  hurting drop deeper than its own plus a jump
  (`a_path_down_licenses_no_deeper_fall`). The hold ladder's head
  (4332, -87, 615): a head a step below, across the face by less than a
  half-width, is climbed over (`a_head_across_the_face_is_climbed_over`,
  `tests/bots.rs` `a_bot_climbs_over_mp_ships_hold_ladder_top`); trap:
  any head across the face pinned two bots abreast at x 3792 (seed 193).
  Stalled / falls / deaths: `mp_depot` 158 / 18 / 9 to 161 / 4 / 1,
  `mp_ship` 304 / 414 / 254 to 296 / 74 / 235, `mp_hurtgen` deaths 4 to 0.
  Trap, dropped: re-proving a backed drop by a creep doubled `mp_ship`'s
  spells (18 seeds to 37).
- 2026-10-12i: `mp_depot`'s creep-only ledge drops (-2750, -1244, 508) to
  (-2749, -1281, 326) and its mirror (-2745, -1186, 508) to (-2755, -1150,
  326): a bot backing off a turn at 148 across tops the back key's 133,
  so a creep that misses off a turn at `nav::TURN_PACE` (150) either side
  and arrives from rest holds its top while the body is faster than
  `nav::CREEP_TOP_PACE` (100) (`Follower::creeps_off`,
  `a_creeps_top_is_passed_only_slow`). The roof peak (-2660, -1216, 552):
  a 1.16 `moveSpeedScale` weapon at 220 flew past the roof's edge where
  the graph's 190 lands; a body faster than the run on a walk down off a
  crest (more than a step, no more than a jump) takes the walk at its pace
  and weapon scale (`Follower::move_scale`, `Sim::scaled`), held if that
  falls, avoided if from rest falls too (`Follower::flings`,
  `a_run_over_a_crest_too_fast_for_the_weapon_is_held_at_the_top`). Two
  bodies abreast at the ladder at x 3792 (the hatch passes a body west
  only with its centre at y 58 to 72): the lower, or of two level the
  later in (y, x), climbs down until the other is off
  (`Follower::abreast_yield`, `of_two_climbers_abreast_one_gives_way`).
  `isTouching` takes a client's capsule. `mp_ship` 296 to 270 stalled, 33
  to 17 spells. Traps, each measured: the crest's scale in `edge_holds`
  for every miss (deaths 117 to 164); holding every creep top off a turn
  (`mp_ship` 296 to 493); dropping such creeps from the build (`mp_ship`
  lost two spawns).
- 2026-10-12j, retail's `pmove::ladder_move` (`cod11-mantle.md`,
  "Ladders"): the climb 32 to 59.4 units/s at a level view and about 53
  to 95 at 45 up, sideways motion bleeds only without a strafe key, the
  rungs let go 8 under a ladder's top. A climb in 50 ms frames bobs under
  a lip (the mast hatch: the step-up's slide at the wall glue's 500
  units/s came back and `PM_StepSlideMove` reverted it), so a body on the
  rungs off the ground sends five, four, three or six cmds a tick by
  turns and the graph's walks step the same way (`nav::tick_times`); the
  strafe toward the rungs' middle stops 36 under the waypoint
  (`bots::STRAFE_STOP`). The mast top moved to (3675, -95.1, 1147.1), the
  head at x 3354 to (3358.6, -384.2, 352.1). `mp_ship` 270 to 1 178
  stalled with the retail ladder alone, 570 with the three; `mp_depot` 158
  to 84.
- 2026-10-13, seven fixes: `mp_ship` 570 / 249 deaths to 121 / 61,
  `mp_depot` 84 / 3 to 33 / 0. The hold's pit: the edge (3647.9, -316.9,
  618.5) to the ledge (3685.5, -354.3, 400.1) was proved toward the ledge
  column's centre and lands in no gait node to node (dropped; section 2,
  "The flood"), and the way left from (3647, -352.9, 611.1) falls from 4
  south, so it is risky (`RISK_COST`); a backed drop is re-proved at pass
  time by the bot's own back down (`Gait::Creep`) at its pace and weapon
  scale, held if that falls, planned round if from rest falls too
  (`Follower::backs_short`,
  `a_backed_drop_is_proved_again_at_the_pace_the_bot_comes_onto_it`;
  deaths in the hold under (3700, -390, 92) 177 to 0, at the hold
  ladder's foot (4352, -94, 56) 21 to 0). The hold crates (beam top
  (2207.9, 511.5, -31.9), crease at (2234, 494, -42)): an unstick spell
  plans round the edge it began on (`Follower::stuck`, `Bot::take_stuck`,
  `a_spell_plans_round_the_edge_it_began_on`; spells 10 to 0). A jump's
  foot is never cut to (the pass's `past`; mast platform deaths 76 to
  15). `mp_depot`'s ladder foot at (-417, 1112): a strafe held under a
  full-rate climb keeps the grab's pace across, so the key comes up while
  the body slides away from the middle
  (`a_climber_sliding_away_from_the_middle_lets_the_strafe_go`). The mast:
  from the head (3675, -95.1, 1147.1) only the creep from rest (and up to
  100 units/s along the spar) lands at the foot (3675, -103, 896.1); the
  follower holds such a head until the body is under 100
  (`a_creep_down_proved_only_from_rest_is_passed_only_slow`,
  `ships_ladders_are_climbed_both_ways`, `tests/bots.rs`
  `a_bot_climbs_down_mp_ships_mast_from_the_spar`). Traps: the Backward
  gait away from ladders (ladder foot deaths 21 to 36); holding every such
  node until rest (stalled 121 to 240).
- 2026-10-13b, the crow's nest (head (4963.9, 64, 1219.1) on the plate
  in the hatch, exits at y 35.5 and 92.5 on the rim at 1219, the top
  floor at 1254 west; a body on the rim west of x 4954.1 pins on the
  top's face, one at y 57.9 to 72.9 there drops through the hatch): 20
  deaths traced (bobs under the lip, turns off the north exit, plans that
  began a floor up and pinned on the top's face). A body that closed on
  its waypoint by 2 units in the ten ticks, across plus height, is not
  stuck (`Bot::closes_in`, `a_back_down_sliding_along_a_rim_is_not_stuck`),
  and a plan whose first node is more than a step over the feet with an
  unproved start is planned again from a walked node
  (`Follower::proved_start`, `a_plan_that_starts_a_floor_up_is_planned_again`).
  `mp_ship` 121 / 61 to 0 / 34, crow's nest deaths 20 to 3.
- 2026-10-13c, the mast platform's ledges: 15 deaths, all one edge: a bot
  off the mast's foot ran off the platform's drop node (3564, -127, 896)
  at 210 to 228 for the ledge (3520, -162, 760) and flew over it; the
  forward walk lands only at the graph's 190
  (`the_run_off_the_mast_platform_lands_only_at_the_graphs_pace`). Keys
  on the ground are cut to the graph's pace (`nav::paced_key`), the
  no-gait drop is risky, the bot rests on the last waypoint while a plan
  is searched (`Follower::rest_point`), and a jump's tick keeps the whole
  key. Seeds 1-767 odd deaths 34 to 16, 769-1535 odd 38 to 5; ledge deaths
  19 to 1 and 23 to 1. Trap: re-proving runs off at the weapon's scale in
  the pass moved 8 deaths onto the roof slopes south of the platform.
- 2026-10-13d: the jump's cooldown (section 2, "Jumps"; `mp_depot` seed
  9's crate stack at (-2256, 149, 46)); an edge with no way round is taken
  from rest (`an_edge_with_no_way_round_is_taken_from_rest`; the drop from
  (4928.5, 60.8, 1254) to the spar (4888.3, 64.6, 1211.6)); a spell
  within 10 ticks of the rungs climbs down on them and holds still off
  them, and a climb bobbing under the top is not stuck
  (`a_spell_just_off_the_rungs_holds_still`,
  `a_climb_bobbing_under_the_top_is_not_stuck`); the drop probe's half-way
  samples (`the_drop_probe_feels_the_crows_nest_hatch`; the hatch about
  40 by 46 units, x 4940-4980, y 41-86); the ladder wait's nearest box,
  two-steps-off spot, 140 look-down at a deadly head and climb back
  (`the_crows_nest_head_has_a_spot_to_wait_at`,
  `a_bot_at_a_ladders_head_waits_for_a_climber_two_bodies_down`,
  `a_bot_backing_down_onto_a_climber_climbs_back_up`); a plan from the
  rungs passes its head only on the ground
  (`a_plan_from_the_rungs_passes_its_head_only_on_the_ground`); the stall
  circle is the farthest the body went (`out_and_back_is_no_stall`).
  `mp_ship` deaths 16 to 4 and 5 to 2, `mp_depot` stalled 30 to 0.
- 2026-10-13f: the wait at a deadly head lasts while a body off the rungs
  stands between the spot and the rungs, counts bodies in the air and a
  climber within a jump either way, and the spot stands 30 clear of the
  head's neighbours ((5021.2, 35.1) or (5021.2, 92.9);
  `a_wait_at_the_crows_nest_lasts_until_the_climber_is_off_the_head`; the
  plate holds a body only with its middle east of about x 4961); the
  hatch drops (the exits' back offs into the hatch, landing only on a
  grab of the rungs) and the mast top's and (4319, -133, 600)'s deep
  drops are risky (section 2); no roam picks a perch
  (`a_roam_never_ends_over_a_fall_that_kills`; a roam that ended on the
  rail corner (5023.8, 123.7, 1254.1) ran off it); the drop probe runs
  only when read. `mp_ship` deaths 4 / 2 to 2 / 0 on the two seed sets,
  stalled 11 / 35 to 0 / 1, no crow's nest death or stall left.
- 2026-10-15f, recheck on master 32eb3a4e over seeds 1-2303 odd: none of
  10-12b to 10-13f's places stalls or kills. The mast platform's roof
  (seeds 379, 457): a bot passed (3514, -283, 623) 24 short at 158 units/s
  9 up the slope, flew over the ridge node (3496, -318, 611) and fell to
  (3421, -419, 137); such walks down beside a deadly fall are risky
  (section 2). Trap: the walk on taking the slope's 9 units/s rise
  rerouted bots over the ridge, deaths 2 to 44. Deaths 4 to 0.
- 2026-10-15h: a wait aside holds at its spot however far it stands and
  at any head lasts while a body off the rungs stands between spot and
  rungs (`a_wait_aside_lasts_until_the_climber_is_past_the_spot`; seed
  1337, three bots at the doorway west of the head (3315, -380, 352), 35
  to 0); `edge_holds` fails a walk on within two edges of a climb that
  falls at the weapon's scale under 1
  (`a_walk_on_that_falls_at_the_weapons_scale_is_not_taken`; seed 2171 at
  the mast hatch, 0.9 weapon); the follower's deadly height is the lowest
  fall `PM_CrashLand` kills at the bot's current health
  (`nav::fall_that_kills`, `Follower::fall_kills`,
  `a_hurt_bot_waits_for_a_climber_further_down`; seed 1327 fell 296 with
  its health down; the graph's perches and risky drops keep 480).
  Seeds 1-2303 odd 48 to 24 stalled.
- 2026-10-15i, seed 471's shelf: a bot turning for a jump edge's top more
  than a jump below read as pinned while it slowed and jumped straight up
  off the shelf; under such a top the body reads pinned only on its second
  cmd in a row closing under `PINNED` (`Bot::pinned_ticks`,
  `a_bot_turning_for_a_top_below_is_not_pinned`; `mp_ship` has 212 such
  jump edges of 2 727). Seeds 1-2303 odd 24 to 4; fresh seeds 2305-4607
  odd 246 / 6 deaths to 201 / 6. Traps: the cue for every jump edge (203
  stalled, 3 deaths); `closing.abs() < PINNED` (31 and 5 deaths).
- 2026-10-16, fresh-seed deaths: the ledge over the hold at 400, reached
  only by the risky back down from (3647.0, -352.9, 611.1), lands or falls
  by a weapon scale's tenth and a few units of start, so no approach rule
  makes it safe; a roam from inside `NavGraph::safe` passes over a
  destination outside it
  (`a_roam_never_ends_where_only_a_risky_edge_leads`; 443 of 13 954
  nodes left out). Seeds 2305-4607 odd 201 / 6 to 79 / 3. Traps: a node
  whose walk on fails at pace held (deaths 6 to 14); at the crow's nest
  hatch (backed onto its rungs only from about x 4962-4965, y 56-70) a
  head passed only at rest (725 stalled), with a yield (601), with a
  10-tick fallback (404 and 8 deaths).
- 2026-10-18, master 0f3ce28e: the 896 deck (seeds 427, 2477, 4523) ran
  off (3564, 252, 896) for the ledge (3521, 289, 760), whose paced walk
  clips a slope's corner at 798 by 2 units; a run off a drop whose miss
  can hurt has to land from 2 units either side too
  (`a_drop_that_lands_only_on_a_corner_does_not_hold`, `nav::a_miss_hurts`,
  the coast exemption `nav::coasts_off` for the mast top (3301, 128,
  1030)); a hurt bot backing down over the x 3793 hatch's lip walks back
  onto the head first (seed 1795). Seeds 1-2303 odd 38 / 4 to 82 / 2,
  2305-4607 odd 130 / 2 to 130 / 0. Seed 981's overrun at (3392, -316,
  520): a body at 160 units/s heading south falls off the floor's edge
  with no key and with a full brake alike, so the pace has to go before
  the node.
- 2026-10-18b, master 33013a7f, seeds 1-4607 odd: the slot's run off
  landed on the ledge at 336 8 from over the waypoint and turned on the
  spot until a spell threw it off; a body landed partway down a drop
  plans again from where it stands
  (`a_drop_that_lands_partway_down_is_planned_again`). 161 / 25 falls to
  181 / 10. Trap, backed out: avoiding the drop as well (220; seed 1409
  bounced under the stair jump). The crow's nest back down from rest
  lands on the rungs only from a diamond, x 4960-4964 and |y - 64| <= x -
  4960; aiming the back down across the edge (164, 7 deaths) was backed
  out.
- 2026-10-18c, master 7256c89c: a climber pressed on the waiter ends the
  wait at a head whose fall does not kill (seed 761 at the x 3793 spot
  (3751, 2, 616), 26 to 0); the creep off a deadly head faces the rungs
  square and strafes to their middle, which grabs from every cell of x
  4958-4964, y 48-80 at rest, 30 and 80 units/s
  (`a_creep_off_a_deadly_head_faces_the_rungs_and_strafes_to_them`,
  `the_crows_nest_back_down_faces_the_rungs`; four deaths gone); a run
  off a drop stopped short of the lip plans round it, once per edge while
  avoided (`a_run_off_a_drop_stopped_short_of_the_lip_plans_round_it`; the
  hold floor's drop (3296, -419, 56) to (3264, -378, -64); without the
  "once" `mp_depot` read 6 900). 141 / 11 / 6 to 88 / 8 / 4.
- 2026-10-18d, master 1080e30d: `StuckInClient` (`game::stuck`,
  cod11-player-clip.md 6) shoved a bot on the crow's nest top west at 183
  when a climber's box met its feet (seed 1307), so a body on the ground
  holds still up to 60 ticks over a climber rising under it
  (`Follower::rising_hold`,
  `a_body_holds_still_over_a_climber_rising_under_it`); the partway rule
  needs the body more than a jump under the edge's top, not a step
  (`a_body_on_a_drops_own_slope_keeps_the_drop`; seed 2245 on the dome at
  (3648, -317, 618)); of two bodies pressed together on the ground one
  steps back (`Follower::pressed_yield`,
  `of_two_bodies_pressed_together_one_gives_way`; seed 3223, two climbers
  at x 3792 outside the hatch's lane; `unpin_yaw` traces the world only).
  88 / 8 / 4 to 40 / 8 / 1. Untraced by a fix: a level edge whose walk on
  leaves the ground over a fall that kills has no aside test (seed 4231,
  the dome node (3615, -345, 635)).
- 2026-10-19b, the ladder yaw cap and the push-off facing the rungs
  (cod11-mantle.md, "The ladder yaw cap"): 47171373 40 / 8 / 1 to master
  447d904f 55 / 7 / 0, every moved seed a run that parted, none at a
  push-off; `mp_depot`, `mp_hurtgen` and UO's four `tdm` maps unchanged
  to the bot-tick. `mp_ship`'s graph has a new digest (`nav::Sim::tick`'s
  walks rebase on `delta_angles`).

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

### Bodies in the way

Retail has no bots; this section is measured on vcod-server only.

Before, the graph knew nothing of bodies. A bot that met another player
on an edge ran into him until the follower's stall count gave the edge
up (40 ticks without closing, `STUCK_TICKS`), or its own 10-tick stall
circle sent it on an unstick spell, and the next plan took the edge
beside it past the same body: one 2 s stall per edge the body stood
on. VERIFIED (measured, `ship_stalls` with the edge tally below, master
55164194d, `dm`, 10 bots, shoot off, 4000 ticks, seeds 1-127 odd): of
the edges the followers gave up on, those with another body within 64
units were 458 of 527 on `mp_carentan`, 790 of 835 on `mp_harbor` and
814 of 1109 on `mp_ship`; on `mp_uo_stanjel` (UO `tdm`, 2400 ticks) 58 of
410.

Two rules now, in the follower, run by the driver for every game whose
`BotWorld::SHUNS_BODIES` is on (CoD1, 1.5 and UO; off for CoD2 and CoD4,
whose `PARTS_MATES` plans round a still teammate, bot-objectives.md
11.5 and 12.7):

- Held up (`Follower::bodies_in_way`). A bot on the ground, off the
  rungs, fighting nobody, slower than 60 units/s for `BODY_TICKS` (4)
  with another body off the rungs within 48 across and 64 up or down
  toward its next node, drops its plan. The next keeps off the nodes
  that body's box stands over (`NavGraph::nodes_in_box`, two
  half-widths, a step) and the node under way when he stands on its
  edge, for 60 ticks (`SHUN_TICKS`), and starts at a node the bot walks
  to. A plan that finds no way round goes through them
  (`Search::shunning`, then the same search without them); so does one
  whose first two legs from the body pass within two half-widths of
  him, since the start node is never kept off. No second plan round a
  body while the first's nodes are kept off. The tick it plans round,
  the bot's stall circle starts again (`BotView::detour`): the circle
  and the rule fired on the same tick, and the spell walked the bot off
  its new plan. Any team counts: a dm or shoot-off enemy blocks as a
  teammate does, and so does a human.
- Head-on (`Follower::stand_aside`). With no way round, a bot that meets
  head-on one it gives way to (any player not a bot, by his velocity;
  any bot in a lower slot, by its follower's waypoint,
  `bots::driver::oncoming`) heading at it within the same reach stands
  aside: on a node one or two level edges off across the line between
  them, two half-widths clear of it (`NavGraph::sidestep`), else 64
  straight back. It rests there (`BotView::stop`) until no such body is
  ahead of it within 96 along the way it came, for at most 60 ticks,
  then plans again; not again for 60. Of two bots exactly one gives way.

INFERRED (the traces, a temporary per-tick log of the follower): a
plan's start is the node nearest the body or one it walks to
(`walkable_start`, which walks the world with no bodies in it), and two
bots meeting on `mp_uo_stanjel`'s slope at (1, 2885, -154) both planned
from the node between them, round each other every 10 ticks, each plan
restarting the stall circle, for 12 s and more (seeds 7 and 105, 642
and 756 stalled bot-ticks); hence the first-two-legs rule and one plan
round per shun.

Measured with two tallies added for this (`Server::bot_edge_stalls`,
read by `ship_stalls` and `bot_match`): the edges the followers gave up
on and those with another body within 64 units, and in `ship_stalls`
the bot-ticks held up by a body (moving under 60 units/s with another
within 64 across and 64 up or down toward the waypoint, not holding on
purpose). `bot_match` adds the mean time from a round's start to its
first objective action (`to objective`).

VERIFIED (measured, `ship_stalls`, `dm` 10 bots shoot off 4000 ticks,
seeds 1-127 odd; `mp_uo_stanjel` UO `tdm` 2400 ticks; master 55164194d
then this change; stalled bot-ticks / falls / edges given up / those by a
body / bot-ticks held up by a body):

| map | master | bodies in the way |
|---|---|---|
| `mp_carentan` | 0 / 0 / 527 / 458 / 20 907 | 0 / 0 / 55 / 31 / 11 380 |
| `mp_harbor` | 0 / 0 / 835 / 790 / 41 341 | 0 / 0 / 79 / 67 / 23 555 |
| `mp_ship` | 47 / 1 / 1 109 / 814 / 33 713 | 0 / 2 / 385 / 99 / 19 428 |
| `mp_uo_stanjel` | 0 / 2 / 410 / 58 / 4 130 | 0 / 2 / 382 / 20 / 2 965 |

`mp_ship`'s second fall (seed 65, an unstick spell off (3744, -792, 27)
to -58) is not traced. With the plan round after 10 ticks held up
instead of 4, the same sets gave up 215 / 341 / 616 / 391 edges and held
17 326 / 35 406 / 27 422 / 3 836 bot-ticks: the 4-tick wait keeps most
meetings under half a second.

VERIFIED (measured, `bot_match`, shoot on, 12 000 ticks, seeds 1-6, the
same two commits; stalled bot-ticks, kills, mean seconds to a round's
first objective action):

| set | master | bodies in the way |
|---|---|---|
| 1.1 `sd` 6 bots, `mp_carentan`, `mp_harbor` | 72, 531, 38.5 s (113 rounds) | 0, 524, 38.0 s (115) |
| 1.1 `tdm` 10 bots, `mp_carentan`, `mp_harbor`, `mp_ship` | 1 658, 3 139 | 221, 3 167 |
| UO `sd` 6 bots, `mp_uo_stanjel`, `mp_cassino` | 55, 420, 35.8 s (97) | 23, 424, 36.4 s (95) |

`mp_ship` has no `sd`. `tdm`'s stalls were `mp_ship` seeds 1 (1 162), 2
and 4 and `mp_carentan` seed 3 on master; `mp_ship` seeds 2 to 6 on this
change (26, 6, 177, 12; not traced). Kills hold within the per-seed
noise. The time to the objective moves inside it too: the meetings this
removes cost a bot a second or two, against rounds of 25 to 45 s.
Ladder meetings keep their own rules (`ladder_yield`, `head_on_yield`,
`abreast_yield`, `rising_hold`, section 3 above): this applies on the
ground only, and the mast ladder's head meetings on `mp_ship` are
unchanged.

### The ship's leftovers

Retail has no bots; this section is measured on vcod-server only. Each
leftover traced per tick (`ship_stalls` `TRACE=`, `bot_match`
`STALLS=1`, and a temporary per-tick log of the follower and the cmd),
release, 1.1, on "Bodies in the way"'s tip (ae407a223):

- The item's seeds (6 bots, `dm`, shoot off): 1233 and 1521 (the
  stairwell crease at (5445, 355, 292)), 991 and 797 (the dome slope at
  (3789, -207, 352)) and 1345 stall 0 with no fall or death on that tip.
  VERIFIED (measured).
- `mp_ship` 1381's death on the spar at (4676, 64, 1172). Held up by a
  bot coming the other way, the follower dropped its plan for one round
  him (t1573), and the search took 7 ticks. With no waypoint and its
  last 69 units off, past `WAIT_REACH`, the bot was not held and ran a
  random heading (`Bot::think`'s wander) east-south-east off the spar's
  side, fell to (4924, -18, 535) and died. VERIFIED (trace). The plan
  round a body is now waited for where the body stands
  (`Follower::rest_point`,
  `nav::tests::a_body_waits_where_it_stands_for_the_plan_round_a_body`);
  the seed has no death.
- The deck pocket at (3527 to 3581, -165 to -7, 352) (`bot_match` `tdm`
  seeds 4 and 5, 6 and 177 stalled bot-ticks). Its one plain way north is
  the edge (3581, -7, 352) -> (3549, 33, 352) round the end of a block
  whose south face stands at y 8 for x 3549 and over: a standing box
  passes north only at x 3548 and under (box traces at x 3520 to 3596).
  VERIFIED (traces). The graph's walk proves the edge from the node at a
  run, and from rest at (3578, -7) too. VERIFIED (the walk). A bot
  failed it twice over: a remembered spot turned its keys to a back key
  and a strafe (forward -97, right -41), which retail scales by 0.7 and
  0.8 (`pmove::SCALE_BACK`, `SCALE_STRAFE`), and against the face its
  speed along it fell from 27 to 8 units/s until its stall circle fired
  (seed 5, t7613 to t7638); and goals flipping between noises replanned
  it to a stop at the corner (seed 4, t3850 to t3880). VERIFIED
  (traces). Avoided twice, the edge is dead, and the pocket's other way
  out is the risky drop (3496, -96, 352) -> (3450, -96, 272), which no
  plan with an edge avoided takes: every
  plan failed and the bot rested 20 ticks at a time until the dead edge
  ran out 200 ticks on (seed 5 bot 8, level ticks 7640 to 7955). VERIFIED (trace,
  the follower's avoid and dead lists; `nav_build` `EDGES=1`). Two
  changes: the last plan, round the dead edges only, may take a risky
  drop at `RISK_COST` (`Search::risking`,
  `nav::tests::a_plan_round_a_dead_edge_takes_a_risky_drop`; the plan
  round a stuck edge still takes none, as seed 969 needs); and under
  `RECALL_PACE` (66 units/s) the keys keep the path's own run with no
  pre-aim (`Bot::think_wander`,
  `bots::tests::a_slow_body_keeps_the_path_under_a_recall`).
- A health item on the ledge at (4603, -586, 52) (`tdm` seed 5 on the
  pre-aim change, 388 stalled). A hurt bot's heal goal kept the
  destination of its fight's plan, the hold floor node (4578, -576, -64)
  116 under the item: within `RETARGET` (128) of the point, the goal had
  not moved. Arrived there, it walked at the point under the ledge
  while the item lay. VERIFIED (trace: `view.survive.heal`, the
  follower's `dest`). A destination more than a jump off the height of
  the node a plan to the point ends at (`NavGraph::goal_node`) now
  reads as moved, for a parked plan too (`Follower::follow`,
  `nav::tests::a_goal_a_floor_off_its_destination_is_planned_for_again`).
- Two health items a hurt bot flipped between (`tdm` seeds 11 and 24
  on the changes above, 210 and 741 stalled). The item was the nearest
  across, picked afresh each tick: between one on the hold floor at
  (4557, 631, -63) and one on the deck at (4418, 579, 56) the pick
  changed as the bot moved and it paced between their ways at (4450,
  700, -64) (seed 11); and the way to one at (5058, -319, 92) climbed the
  stairwell over (5402, 321, 344), more than `HEAL_HEIGHT` (200) over
  it, where the item dropped out, the bot's other goal took the jump drop
  back to 204, and the item came back (seed 24). VERIFIED (traces,
  `view.survive.heal` per tick). The item walked to is now kept while it
  lies within `HEAL_REACH` across, at any height
  (`retreat::nearest_pack`'s `held`, `Bot::heal_at`,
  `bots::retreat::tests::a_hurt_bot_keeps_to_the_health_item_it_walks_to`).
- `ship_stalls` seed 65's fall (10 bots). The roam ended at (3744, -794,
  27), at the foot of a slope the walk up to (3744, -767, 40) climbs only
  at a run: from rest the body hops in place, off the ground. VERIFIED
  (walks from the node at a run and from rest). The next plan, made while
  the body still came down onto the node at 137 units/s, started at
  (3744, -767, 40), nearest it then; the bot pressed at it from the foot
  until its unstick spell ran down the graph's own plain drop to
  (3714, -800, -64), which the detector counts as a fall. VERIFIED
  (trace). Not changed; the seed has no fall on this tip.
- `ship_stalls` seed 9's fall (10 bots): the spell's fall is from the
  mast deck at 480 to 352, where the plan was headed. Before it, the bot
  on the platform at (3461, -189, 798) went for the drop to (3423, -192,
  664) and fell past it to 480. VERIFIED (trace). Not traced further.
- Tried and dropped: steering a climb whose foot stands aside of the
  rungs (`NavGraph::face_ahead` finds none) at the rungs' middle on the
  foot's side, for the hold ladder (3264, 602, -64) -> (3297, 576, 56),
  where a bot run straight at the head from the east pinned on the wall's
  end north of the rungs at (3289, 596, -64) (`tdm` seed 12). Over the
  64 seeds below the edges given up rose from 385 to 687, bots ending off
  the graph at (3590, 198, 616); only on the foot's floor, 687.
  VERIFIED (measured).
- Left: that hold ladder; the mast ladder at x 3808 between 485 and 616
  and the shelf ladder at (5352, -70) between 352 and 448, each one
  bot on the rungs under a body at the head (`tdm` seeds 6 and 4 before
  the health item change: 4 and 11 stalled bot-ticks; VERIFIED,
  measured, positions only); the mast ladder's head meetings at (4954
  to 4964, 35 to 64, 1219) and the jump cue's routing leftover ("The
  jump cue and the re-proof"), which no run here showed.

VERIFIED (measured, ae407a223 then this change; `ship_stalls` `dm` 10
bots, shoot off, 4000 ticks, seeds 1-127 odd: stalled bot-ticks / falls /
deaths / edges given up / those by a body / bot-ticks held up by a body;
`bot_match` 1.1 `tdm` 10 bots, 12 000 ticks, `mp_carentan`, `mp_harbor`,
`mp_ship`, seeds 1-6: stalled bot-ticks, kills):

| set | ae407a223 | the leftovers |
|---|---|---|
| `ship_stalls` `mp_ship` | 0 / 2 / 0 / 385 / 99 / 19 428 | 0 / 1 / 0 / 384 / 107 / 19 337 |
| `bot_match` `tdm` | 221, 3 167 | 23, 3 092 |
| `bot_match` `tdm` `mp_ship` seeds 7-24 | 125 | 173 |

The 23 are `mp_ship` seed 1, a bot at (3459, 427, 352) bound down the
drop to (3423, 547, 129), not traced. Seeds 7-24 stall at the hold
ladder at (3280, 575), the mast ladder at x 3808, the hatch ladder at
(3359, 512) and the shelf slot at (5384, 289, 396) above the stairwell's
drop to 204; not traced.

Seeds 7-24 rose, so each of the five changes was switched off in turn
(a temporary environment switch per change, not kept), alone and
leaving the others on. A rerun of ae407a223 is the same to the bot-tick,
so its spread is the spread across seeds: one seed with a stall of
several hundred bot-ticks (a heal goal under a ledge, two items
flipped between, the dead-edge pocket) moves a block of 18 seeds by more
than any change. VERIFIED (measured; `bot_match` 1.1 `tdm` 10 bots,
12 000 ticks, `mp_ship` only; stalled bot-ticks for seeds 1-6 and 7-24,
kills over 1-24):

| build | 1-6 | 7-24 | kills |
|---|---|---|---|
| ae407a223 (and again) | 221 | 125 | 3 739 |
| all five | 23 | 173 | 3 711 |
| the wait round a body alone | 99 | 452 | 3 823 |
| the risky last plan alone | 24 | 411 | 3 749 |
| no pre-aim when slow alone | 1 319 | 1 183 | 3 644 |
| the floor-off retarget alone | 90 | 834 | 3 785 |
| the held health item alone | 85 | 305 | 3 663 |
| all but the wait | 0 | 371 | 3 782 |
| all but the risky last plan | 80 | 247 | 3 737 |
| all but no pre-aim when slow | 0 | 1 028 | 3 827 |
| all but the floor-off retarget | 172 | 177 | 3 761 |
| all but the held health item | 15 | 285 | 3 800 |

No change alone takes ae407a223's 7-24 under 125, and every build with
one change left out stalls more over 1-24 than all five (196). Over a
wider block, seeds 25-72, ae407a223 stalls 3 153 bot-ticks (seeds 48,
59, 31, 52 and 53 at 246 to 860 each) and all five 728 (none over 161);
kills 7 542 and 7 450. VERIFIED (measured, the same runs). Seeds 7-24
are a block where ae407a223 drew few long stalls, not a regression of
one change: over seeds 1-72 the stalls go from 3 499 to 924 and kills
from 11 281 to 11 161 (-1.1%).

### A goal far off the graph, the ladders again, 2026-11-14

Retail has no bots; this section is measured on vcod-server only. Traced
per tick (a temporary per-tick log of the follower, the cmd and the
goal), release, on c6d7e1bd9.

- UO `tests/bots.rs` `uo_tdm_games_that_stalled_or_never_sprinted` seed
  43 (bot-objectives.md 10.8): a bot bound for a noise at (4146, -2366,
  212), 220 over the nearest node (4129, -2352, -8), turned at the foot
  of mp_cassino's roof jump (4851, -1296, 256) to (4896, -1337, 280)
  between the foot and the top every two to three ticks for 12 s. The
  follower read a point goal as moved when its destination node stood
  more than `RETARGET` (128) off the point, and for a point that far off
  the graph its own node does: every tick the plan was parked and the
  one parked the tick before taken up (`PARK_TICKS`), so the waypoint
  went back to the state of two ticks before. VERIFIED (trace: the
  follower's reset from the moved branch on 134 of 255 traced ticks,
  the goal constant). A point now moves when it lies more than
  `RETARGET` off its destination and the node it maps to does too, or a
  floor off it as before (`Follower::follow`,
  `nav::tests::a_goal_far_off_the_graph_keeps_its_plan`). A point on
  the graph reads as before.
- mp_ship's ladder stalls, `bot_match` 1.1 `tdm` 10 bots, 12 000 ticks,
  seeds 1 to 72, `STALLS=1` (each 12 s window's start). VERIFIED
  (measured, positions only):
  - The shelf ladder at (5352, -70), 352 to 448, with no other body
    near: seeds 7, 16, 21, 26, 30, 41 and 55. Traced on seed 7 (bot 7,
    t5500 to t5745): up the ladder, along the shelf at 448 to (5343,
    -155), down to the slot at (5403, -160, 396), off the drop for the
    stairwell floor at (5405, -192, 204), landed on the floor at 352
    instead; the follower planned again from there without the edge
    (the drop onto a floor partway, section 3) and the plan went back
    up the ladder to the same drop, three times in 12 s. VERIFIED
    (trace). No meeting.
  - The hold ladder at (3280, -440), -64 to 56, its head (3337, -440,
    56): seeds 1, 4, 17, 20 and 21. Seed 4 (bot 4, t245 to t485): up
    the ladder, pinned by a teammate standing at (3376, -404) on its way
    on, then bound back down, it waited aside for two climbers in turn,
    the second of which stood fighting 11 units off the head for 35
    ticks (`ladder_yield`'s wait holds while a body off the rungs stands
    between the spot and the rungs). Seed 1 (bot 2, t11640 to t11890):
    two teammates held still at (3318, -415) and (3334, -441), and the
    bot pressed between them at the head, went down, came back up. Each
    a crowd at the head in a fight. VERIFIED (traces). INFERRED: a
    rule for these trades one wait for another; not changed.
  - The ladder at x 3354 (its head (3370, -380, 56), top 352): seeds 10
    and 44, positions only. The mast ladder at x 3808: no stall in the
    72 seeds. VERIFIED (measured).

VERIFIED (measured; c6d7e1bd9 then the two changes of bot-objectives.md
10.8 and this section; `bot_match` 1.1 `tdm` `mp_ship` 10 bots 12 000
ticks; stalled bot-ticks per seed block, kills over 1 to 72; `ship_stalls`
`dm` 10 bots shoot off 4000 ticks seeds 1 to 127 odd: stalled bot-ticks
/ falls / deaths / edges given up / those by a body / bot-ticks held up
by a body):

| set | c6d7e1bd9 | both changes |
|---|---|---|
| `bot_match` seeds 1-6 / 7-24 / 25-72 | 113 / 202 / 1 213 | 56 / 334 / 994 |
| `bot_match` seeds 1-72, kills | 1 528, 11 270 | 1 384, 11 398 |
| `ship_stalls` `mp_ship` | 0 / 1 / 0 / 364 / 101 / 21 060 | the same |

Inside the per-seed spread on `mp_ship`; the change closes the UO gate.
c6d7e1bd9's 1 528 over seeds 1-72 against the 924 above is the base's
own: the CoD1 script draws moved every spawn pick.

### The pass on threads

The bots' pass works its costly per-bot parts out on worker threads
(docs/dev/server-tick.md, "The bots' threads") and moves every bot as it
did on one thread. VERIFIED (release `bot_match DIGEST=1`, 12 bots, 2000
ticks, seeds 1 and 2, on 1.1 mp_carentan `tdm`, UO mp_cassino `tdm`, CoD2
mp_toujane `tdm` and CoD4 mp_crash `war`: the same digests before the
threads, serial and on eight threads, 2026-11-14; `bot_threads::`, one
match per game serial against four threads, every tick's cmds and bodies).
The gate catches a plan that reads wrong: worked out from a bot's
generator with one bit flipped, the bots part at tick 5 on 1.1, 4 on CoD2
and 63 on CoD4. VERIFIED (`bot_threads::`, 2026-11-14).

- The plan budget stays shared and spent in slot order. A route worked out
  on a worker searches from a full budget (4000), and the loop takes it only
  while its own budget left is at least `PLAN_SLICE` (2000), then spends
  what the worker's search spent. INFERRED (`Follower::plan`: a search
  runs the least of the budget and `PLAN_SLICE`, a follower plans at most
  once a tick, and from either budget the same slice stops at the same
  node). No budget is split, so no plan moves.
- A bot's route reads the followers of the bots before it twice: the way it
  gives a teammate queued behind it (`give_way`) and the bodies it stands
  aside for (`oncoming`, the lower slots' headings after their routes this
  tick). A plan reads them as pass 3 starts; the loop compares the way given
  through the view, and the bodies only when the plan read them
  (`Follower::stand_aside` reads them past its own tests). INFERRED
  (`driver::route`). Read for every route, the bodies sent 19 341 of 1.1's
  47 892 planned routes back to the ticking thread; read only when needed,
  `bots/route` fell from 0.43 to 0.03 ms a tick. VERIFIED (measured,
  mp_carentan `tdm`, 12 bots, 2000 ticks, seeds 1 and 2, 2026-11-14).
- A crease cell keeps the verdict felt from the first spot read in it, so a
  drop probe on a worker reads the level's cells and keeps those it felt
  aside; the loop keeps its probe only where no cell it felt was filled
  since with another verdict (`Creases::merge`), else probes again.
  INFERRED (`nav::CreaseRead`).
- The loop took 47 625 of CoD2's 47 940 routes and 12 505 of its 12 533
  drop probes from the plans, CoD4 47 592 of 47 940 and 4 948 of 4 962.
  VERIFIED (counted, the digest runs above).
- The cover looks come after every bot's sense, and on threads in each
  bot's plan, after the objective roles: a look writes only its own bot's
  spot and only `Bot::think` reads one. INFERRED (`driver::land_covers`;
  `BotView::cover`'s one reader is `cover_engage`).

| game, map, 12 bots | `head/bots` before: mean / p50 / p99 / max ms | after | tick before: p50 / p99 / max | after |
|---|---|---|---|---|
| 1.1 mp_carentan `tdm` | 2.31 / 1.75 / 8.94 / 14.5 | 1.54 / 1.01 / 6.65 / 9.4 | 3.55 / 11.10 / 16.0 | 2.39 / 8.20 / 10.9 |
| UO mp_cassino `tdm` | 2.13 / 1.77 / 9.10 / 12.3 | 1.50 / 1.06 / 8.30 / 12.7 | 3.82 / 11.33 / 14.9 | 3.44 / 10.96 / 15.7 |
| CoD2 mp_toujane `tdm` | 1.22 / 1.13 / 2.65 / 6.5 | 0.77 / 0.70 / 2.05 / 6.3 | 2.33 / 4.19 / 10.3 | 1.71 / 3.52 / 8.3 |
| CoD4 mp_crash `war` | 1.37 / 1.18 / 3.83 / 9.3 | 1.12 / 0.92 / 3.52 / 11.9 | 2.46 / 5.40 / 15.9 | 2.23 / 5.22 / 20.3 |

VERIFIED (release `bot_match TICKSTATS=1 JOBS=1`, seed 1, 2000 ticks,
the default four threads after; run at normal priority on a 30-core
machine whose other jobs ran at idle priority, 2026-11-14).

- A tick's plans take at least its slowest bot's plan: on CoD2 that
  averaged 0.25 ms of 0.8 ms of plans a tick, and four threads ran the
  pass as fast as eight (`head/bots` 0.95 against 0.98 ms on CoD2, 1.59
  against 1.67 on 1.1). The slowest ticks are one bot's long search or
  cover look, which no thread count splits. VERIFIED (measured, the runs
  above and their eight-thread twins).
- Threads started afresh each tick cost more than they saved (`bots/plans`
  0.94 ms against 0.70 ms of the same work on one thread: 0.22 ms
  starting them, 0.53 waiting on them); parked workers woken by the job
  joined it 0.1 to 0.4 ms late; woken after the nav tick and spinning
  until the plans are in, `bots/plans` takes 0.34 ms. VERIFIED (measured,
  CoD2 mp_toujane `tdm`, 12 bots, 2000 ticks).
- On a machine with more runnable threads than cores the pass on threads
  ran slower than serial (CoD2 `head/bots` 4.6 against 1.6 ms, threads
  started each tick, a load of 100 on 30 cores): its workers waited for a
  core. `VCOD_BOT_THREADS=1`
  runs it serial. VERIFIED (measured, 2026-11-14).
- What stays on the ticking thread: CoD2's 0.77 ms is `bots/moves` 0.26
  (the cmds through `ClientThink`), `bots/plans` 0.36 and `bots/sense`
  0.10 (the enemy sight: the games' levels are not `Sync`); 1.1's
  `bots/probes` keeps 0.26 ms (the lob probe and `pre_think` stay
  serial). VERIFIED
  (measured, the runs above).

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
