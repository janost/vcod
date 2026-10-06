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
about 20 000 cells, rounded up to a multiple of 16 and clamped to 32..64. The
small maps get 32; `mp_brecourt`, `mp_dawnville`, `mp_powcamp` and `mp_rocket`
get 48; `mp_hurtgen` 64. 32 is the finest worth having: with a player half
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
- On a ladder the body looks 45 degrees up, where `ladder_move`'s climb
  saturates, and the budget stretches to 200 ticks. A forward walk that fell
  and never arrived is retried backing along the line facing the way it came,
  which is how a body grabs a ladder below a ledge.
- A node is refused when it lies under a sheet of ground: terrain and patches
  are surfaces, and a gap at a sheet's edge let the first flood of
  `mp_hurtgen` walk the map's floor at z -447.875 under the whole terrain
  (VERIFIED, point traces from those nodes).
  From there the ray up stops on the underside of a triangle whose
  `(b - a) x (c - a)` faces up, or passes through a one-sided one that the ray
  back down then hits.
- New nodes stay within 512 units of the spawns' bounding box. Past it lies
  scenery: `mp_hurtgen`'s forest added 10 000 nodes.
- Walks run on every core, a layer at a time, handed out one node at a time;
  the merge is serial in job order, so the graph does not depend on the thread
  count.

### Build times

VERIFIED (measured) with `cargo run --release -p vcod-server --example nav_build` on an
8-core, 16-thread Ryzen 7 5850U laptop. Last column: spawn points in the
strongly connected component holding the most of them.

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

- VERIFIED (measured): an early single-threaded build of `mp_carentan`, before
  the diagonal shortcut and the stall cutoff, took 10.6 s. `perf` puts 80% of
  the build in `CollisionWorld::trace_node`.
- VERIFIED (measured): a debug build (the test profile) of `mp_carentan` takes
  about 7 s.
- The server builds the graph on the first tick with bots, not at map load,
  and keeps every graph for the life of the process (`nav::graph_for`), so a
  map cycle that comes back to a map, and the test gates that start one server
  per test, build it once.
- VERIFIED (the off-component spawns' heights against the map's
  `SURF_LADDER` brushes): `mp_ship`'s gap is its upper decks, which connect by
  ladders. INFERRED: the lattice meets few of them square on. The other maps'
  gaps were not investigated.

## 3. Following

The brain stays pure. Each tick the server asks `Bot::goal(&BotView)` for a
`Goal`: `Hold` (dead, spectating, mid-throw), `To(point)` (the enemy it sees)
or `Roam`. A per-bot `nav::Follower` turns that into `BotView::waypoint`:

- A* (Euclidean cost and heuristic) from the node nearest the bot to the node
  nearest the goal, at most two A* runs per server tick across all bots.
- `Roam` picks of eight random nodes the first 1000 units away (else the
  farthest), using the server's seeded generator; a reached or unreachable
  roam destination is replaced.
- A `To` point is re-planned only once it moves 128 units off the planned
  destination; at the end of the path the bot heads at the point itself.
- A waypoint is passed within 24 units horizontally, or once the bot is nearer
  the next node than the waypoint is. A waypoint four grid steps away means the
  bot left the path: plan again. Forty ticks without closing on a waypoint
  drops the path and leaves the bot 20 ticks to its own unstick.

The brain runs at the waypoint, looks 45 degrees up when it is more than 48
units above (a ladder), and backs toward it facing away when it is more than
64 below (a ladder or ledge below, the way the graph proved it). A bot that
has not left a 15-unit circle in ten ticks takes a random heading for 15 ticks
whether it has a waypoint or not. Engaging an enemy overrides all of it.
In S&D the objective names the point and can hold the bot still
(`bot-objectives.md`); a bot standing at its objective or linked by the
script is never counted as stuck.
