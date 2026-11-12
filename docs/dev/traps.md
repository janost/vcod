# Traps already paid for

Each entry below cost a debugging session. Read the section for the game and
area you are touching before you change it. The research section an entry
names holds the evidence.

- [All games](#all-games): script host and VM, assets, net, bots
- [CoD1 1.1 and 1.5](#cod1-11-and-15) (UO shares these unless its section
  says otherwise): wire, map cycle, movement and collision, combat and items,
  script, assets and rendering
- [United Offensive 1.51](#united-offensive-151): wire, vehicles, movement,
  rendering and sound, bots
- [CoD2 1.3](#cod2-13)
- [CoD4 1.7](#cod4-17)

## All games

### Script host and VM

- A game version's rule in the host, spawn, item, vote, field and print code
  reads a named `HostProfile` field (`game::profile`), never `profile.game`.
  The VM's rules go through `VmProfile` (`Vm::apply_profile`). The load picks
  the profile and `GameHost::new` takes it. A test that switches version calls
  `GameHost::set_profile`: a write to `defs` alone leaves the builtins and
  rules on the old version. `host.cod2()` panics on a CoD1 host;
  `GameHost::new` and `set_profile` pick the `HostExt` arm off the game.
- gsc folds case for identifiers, fields, paths and event names, not for
  string values or array keys. A misrouted event name hangs its `waittill`
  silently.
- Unary `!` and a condition read a string as the number it spells (`!"0"` is
  1, `if ("1")` passes). `""` or `"a"` there is fatal.
- A float prints `%g`-style (`1e-05`, `1.23457e+06`), not Rust's format.
- An array is a value on every engine: `b = a; b[k] = v` leaves `a` alone.
  The VM flags an array stored a second time and copies it at the next write
  (`Heap::share`, `Heap::own`). A write path that skips the
  `EnsureArray*`/`OwnLocal` ops writes into every holder. CoD4's
  `CountPlayers` shared `level.players` that way and flooded
  `spawnPerFrameUpdate` (cod4-gsc.md 10).
- `Cx::spawn` runs a script thread before the calling builtin's next
  instruction. `Cx::spawn_then` adds `Host::spawn_returned` once that thread
  first suspends. Only a `Cx` handed to a builtin (or to `spawn_returned`)
  honours either: a spawn through `get_field`, `set_field` or `Vm::with_cx`
  is dropped, so test such a builtin through `ScriptRuntime`.

### Assets

- The CoD2 loads read ahead on every core (`assets::DecodedImages::prefetch`,
  `Pk3Fs::read_unflagged`). A read that must set a pak's referenced flag goes
  through `Pk3Fs::read` or `mark_read`, never the unflagged read alone.
- A load made faster must keep the texture digest and the marked paks
  `cod2_load_profile` prints ([testing.md](testing.md#performance)).

### Net

- The CD key goes to the authorize server on its own socket and thread
  (`net::cdkey::request_authorization`), never through the connection's
  transport, and never into a log: `CdKey`'s `Debug` hides it. 1.1, 1.5 and
  UO send the bare key. CoD2 and CoD4 add `PB <md5>`, an MD5 from
  Activision's own initial state over the upper-cased (CoD2) or lower-cased
  (CoD4) key, the same hash their `getchallenge` carries
  (cod11-server-handshake.md, "CD key authorization (the clients)").

### Bots

- The per-tick driver (`bots::driver::step`) reads and writes the game only
  through `BotWorld`, one call per point of the tick, in CoD1's old
  `step_bots` order. A call moved across another moves rand draws, the enemy
  cache and the plan budget. Its state (`BotDriver`) stays in the game for
  the tick: the view builders, the vehicle code and the admin read `bots`,
  `nav` and `paths` mid-tick. All three games run it in one order. What a
  game's route leaves out the trait carries as a const (`YIELDS`,
  `PMOVE_PROBES`; CoD1 true, CoD2 and CoD4 false), and flipping one moves
  that game's draws. A per-game step goes in a hook (`feel`, `pre_route`,
  `routed`, `pre_think`, `keeps_alive`, `cmd`, `tick_done`), not in `step`.
- Nav build: `Prim::Tri` is terrain only, and `under_ground`'s ray counts
  terrain only, or a patch spar overhead makes a ladder foot read as
  underground. Minefields and `trigger_hurt` brushes are excluded.
- A* is resumable under a shared per-tick expansion budget, so a plan can
  take several ticks, and the body may have left the start's floor when it
  lands (`Follower::proved_start`).
- A body on the rungs off the ground sends three to six cmds a tick, bots and
  the graph's walks alike (`nav::tick_times`). The rungs let go 8 under a
  ladder's top, and whether the step-up clears the lip depends on the frame
  length. A walk that steps a ladder some other way proves edges no bot can
  follow.
- On the rungs a strafe held under a full-rate climb never turns the sideways
  pace the grab carried in. Only a released key bleeds it, so bots let the
  key go while the body slides away from the rungs' middle.
- A bot backs down a drop the way `Gait::Creep` walks (`Bot::steer`), not
  `Gait::Backward`. Re-prove a backed drop with the creep.
- A bot on a jump edge jumps by the walk's own rule (`nav::PinJump`: two
  pinned ticks, a bob, or right under the top; no `jump_clear` room test;
  pinned off its pace toward the top and, on the ground, off the distance
  moved too) and steers at `NavGraph::aim`. Every non-leap jump edge is
  walked again that way, aimed there (`NavGraph::reprove_jumps`,
  `nav::as_a_bot`). A change to the bot's jump goes in `PinJump` or
  `as_a_bot`, or the bot and the graph part. What a bot's jump adds (no room
  test, the ramp wait) stays out of the flood's walks: in them it reshapes
  the graph. A jump edge is run forward whatever its drop: only the forward
  walk jumps, so every jump edge was proved that way.
- A plan round avoided edges takes no risky edge (`nav::Search`). A risky
  drop is taken only by a plan that avoids nothing.
- A following bot runs at the graph's 190 units/s. On the ground its keys are
  cut by the weapon's `moveSpeedScale` (`nav::paced_key`), never on a jump's
  tick, in the air or on the rungs. A probe that runs the bot's own move
  holds the same key.
- `PM_CheckJump` refuses a jump inside 500 ms of the last, and the graph's
  walks set off on every edge with one to hand. A bot on a jump edge waits on
  its foot until the jump is ready (`Follower::jump_wait`), and on 1.5 and UO
  until the jump penalty has run out (`nav::jump_ready`).
- Every walk's body moves by the rules `nav::with_rules` set on its thread
  (`nav::body`). The build, `par_map`'s workers and every server's bot pass
  set them (CoD2's through its graph's `NavGraph::moving`). A walk that
  builds its `PlayerState` with `PlayerState::spawn` instead proves the edge
  under 1.1's rules. A game's own pmove goes in through `nav::with_step`
  (`nav::WalkStep`); every `player_step` in a walk dispatches to it when one
  is set. CoD2's (`bots::cod2::Cod2Walk`) is off (bot-combat.md 16, 17).
  CoD4's (`bots::cod4::Cod4Walk`) is off by default, on with `VC_NAV_COD4=1`
  (bot-objectives.md 12.9, 12.10).
- The follower pulls its waypoint past grid nodes only along plain edges
  (`Follower::pull`, `NavGraph::plain_edge`), and the walking view turns at
  `Skill::walk_turn_deg` only on a plain run (`Bot::walk_turn`). Leave a new
  edge kind or gait the bot must take as its walk proved it out of both, or
  the bot meets it off the line and the view the graph proved it on. A view
  set after the walk's is seen as owned (`Bot::view_owned`) and handed back
  at once; one set before it, as the recall look is, flags itself.
- The driver fills `BotView::drop_ahead` only on the ticks `Bot::reads_drops`
  names. A new reader in the brain widens it.
- The graph's cache keys on which submodels clip, and the server rebuilds it
  once the script's brush links (`GameHost::brush_links`) hold still 2 s
  (`Server::relink_nav`). A test that runs two gametypes on one map builds
  two graphs, and one that deletes a brush mid-level waits on a build.

## CoD1 1.1 and 1.5

United Offensive shares everything here unless its own section says
otherwise.

### Wire and netcode

- Protocol 6 (1.5) renumbered the top of `pm_flags`. The decoder rewrites it
  into 1.1's layout (`Cod1Wire::pm_flags_to_v1`), so a `PlayerState` never
  holds the raw 1.5 value, and the writer maps it back
  (`Cod1Wire::pm_flags_from_v1`). 1.5-only state (`flags::PMF_MAY_CYCLE`,
  `PMF_MAY_FREE`, `PMF_WEAPON_DISABLED`) sits above 1.1's 19 bits, which a
  protocol 1 wire masks off. 1.5's jump penalty keeps its 0x2000, which 1.1
  never sets, and `predict::from_wire` turns it on under 1.5's rules only
  (`MoveRules::v15()`, which UO's `MoveRules::Uo` keeps).
- A 1.5 body's `eFlags` stance follows the eye's stepped target, so the
  predictor reads 1.5's stance off the flags.
- 1.5 and UO send `B` where 1.1 sends `n` for a `map_restart` that drops
  `pers[]` (`n` for one that keeps it). Their gametypes open
  `serverinfo_<gametype>` before the team menu, so a `t` index is not
  `g_scriptMainMenu`'s.
- vcod-server builds every entity and playerstate in 1.1's numbering
  (events, `eFlags`, an event entity's `eType`) and maps it to the protocol's
  once a snapshot (`game::wire::entity_to_protocol`, `ps_to_protocol`;
  baselines too). `pm_flags` maps in the writer. A new wire path that skips
  them sends 1.1 numbers to a UO client.
- Every playerstate field `predict::from_wire` reads, `ClientSim::to_wire`
  has to write. The predictor rebuilds from the snapshot each time, and a
  field left 0 replays differently from the server. A missing
  `iFatigueSoundTime` breathed on every snapshot and played rounds twice
  (coduo-movement.md 15). `predict_ab`'s 1.5 and UO side-by-side gates catch
  the next one.
- The stock pak set is the protocol's (`PakRules::id_pak_numbers`, the
  protocol's `transport.paks`: 1.1 `pak0`..`pak8`, 1.5 `pak0`..`pakf`, UO's
  `pakuo` ones in `uo/` too), not the install's. A retail server streams a
  stock pak anyone asks for.
- A 1.5 match timeout freezes snapshot `serverTime`, so the client clock
  counts snapshots by message number, not by time.
- 1.5 and UO list a stock pak in `sv_referencedPakNames` only when the level
  read a file from it (`Pk3Fs`'s read flags, cleared at each map load), and
  keep a systeminfo past 1023 bytes. 1.1 lists every pak. A load that stops
  reading a file can drop a pak from the list (`Server::level_paks`,
  handshake doc "The four cvars").
- `entityState.weapon` is 1-based into configstring 7. The ammo and clip
  index tables also start at 1.
- `health`, `ammo[]`, `ammoclip[]` sit in the playerstate array blocks. A
  retail client predicts its ammo counter from the clip it is sent.
- `eventSequence` counts after the write: the new events are `(prev..cur)`.
  An entity entering the snapshot replays its ring from 0 when it is a
  general entity, a missile or a fresh corpse (`CG_ResetEntity`). A shell
  that strikes in its first step is only ever sent exploded.
- The cmd's weapon byte is the client's own selection (`cg_weaponSelect`,
  `play::input::PlayInput::selected`), never an echo of `ps.weapon`.
  `switchToWeapon` and a weapon pickup only send the client `a <weapon>`
  (combat doc 9.8). pmove reads a byte of 0 as retail does, a switch to
  empty hands (1.8), so every caller that steps a player (bots, nav walks on
  a bot's playerstate, test cmds: `common::holding`, `echo_weapon`) sends the
  weapon it holds.
- The first usercmd of a message decodes against a base built from the
  playerstate (sight bit = `fWeaponPosFrac != 0`, stance from `eFlags`), not
  the last cmd sent. vcod's writer sends the full branch every cmd for that
  reason.
- `eFlags` 0x8 flips on every spawn. Leave it pinned and a client smears a
  respawn from the corpse. A respawn clears the event ring.
- The damage feedback fields never clear. `damageEvent`'s increment is the
  edge.
- A configstring range's first slot comes from its indexer: status icon,
  head icon and script menu scan from 0, localized strings and shaders
  from 1.
- A player's wire `solid` is packed at link time (after each cmd's move, at
  spawn), never at end frame.
- A client's entity goes out as its own end-frame turn copied it
  (`ClientSim::end_frame_entity`). A higher slot's turret round after that
  turn shows its knockback a snapshot later, and a victim it kills is still
  sent that frame (`cod11-turrets.md` 13.3).
- The netchan scramble keeps `%`. The server reads client commands with the
  server-side string reader. The client reader's `%` mapping garbles every
  later message after a chat line containing one.
- A move message's ack is stamped with the newest frame already sent when
  the packet arrived, not the tick time, or loopback ping reads 21-50 ms.
- On a 1.5 server `sv_time_ms` is the game clock (`level.time`, the
  snapshot's `serverTime`) and `svs_time_ms()` the network's. A match
  timeout (`timescale 0`) holds the first only, so whatever paces, pings or
  times a client out reads `svs_time_ms()` (`cod15-protocol-diff.md` 12).
- A cmd's clock is clamped to [-1000, +200] ms around the last frame's
  `level.time`, and the entering cmd seeds `commandTime` inside that window.
- Server commands queue until the next snapshot and squash like
  `SV_AddServerCommand` (rule in `docs/protocol-1.1.md`).
- The frame loop sends a fragmented snapshot, download or gamestate message
  one fragment per tick, as retail does. An off-LAN client waits out its rate
  and its `snaps` interval between messages. A test that reads one tick's
  packets sees only the first fragment of a big message.
- A client not in the world gets a 9-byte bare message on each turn (every
  tick on a LAN, 1 s apart off it), so a test drains those too. That message
  and a zombie's go out on their own slot's turn in the one slot-order pass,
  so a lower slot's precedes a higher slot's snapshot.
- A pure server drops a client that enters the world without a valid `cp`
  (pak checksums keyed with the gamestate's `checksumFeed`). On UO the list
  opens with the paks of `uo_cgame_mp_x86.dll` and `uo_ui_mp_x86.dll`
  (`Cod1Wire::pure_dlls`), not the 1.1 modules its `main/` still holds.
  Extra arguments to `tools/run_server.sh` land after `+map`, so `+set
  sv_pure 1` there leaves `sv_paks` empty yet still drops. Pass it before
  the map (`PRE_MAP_ARGS='+set sv_pure 1'`) to test a pure server.
- Every `L` cvar latches until the next load, so a latched `g_gametype` must
  not reach serverinfo early. `devmap` sets `sv_cheats 1` after the load and
  `map` sets it back.
- The server ignores an OOB `disconnect`. A client honours one only after 3 s
  without a packet.

### Map cycle

- A map change is pull-shaped. The server sends only the OOB
  `loadingnewmap`, and resends the gamestate when a client's next message
  carries the old serverId. Nothing goes on the reliable stream.
- `map <current map>` is a restart, not a spawn.
- A restart sends every configstring the incoming level wrote ahead of its
  `n`/`B`, one `d` per slot after the queue's squash. The fog `G_InitGame`
  clears and the map sets back still goes out, and `ExitLevel` zeroes the
  team scores in the game only, so the next level's `setTeamScore(..., 0)`
  goes out as `d 6 0`. The burst and the frame's diff go in write order, off
  `ConfigStrings::set`'s log. A game write that bypasses `set` is not
  possible (no `DerefMut`); keep it that way (map-cycle doc 4.5).
- `map_restart` keeps the configstring table and the item registry; only a
  map load clears them. vcod's restart also keeps the compiled scripts
  (`Carry::scripts`). A test that swaps a script mid-level goes through
  `Server::overlay_script`, which drops them.
- A map load resumes the last level's gametype compile (`Carry::gametype`)
  once every file its `Loader` read reads the same (`LoaderState::current`).
  The key is that text, the gametype entry and the builtin set. Anything
  else a compile comes to depend on (a cvar, a new `CompileOptions` field)
  goes in `GametypeScripts`' check, or a map change runs the old compile
  (cod11-map-cycle.md 4.7).
- `game[]` and `pers[]` survive only `map_restart(1)` / `exitLevel(1)`, and
  entity handles inside `game[]` never survive. `dm` passes 0, `sd`
  passes 1.
- Time and score limits are script. The engine has no `CheckExitRules`;
  nothing reads `g_intermissionDelay` or `nextmap`.

### Movement and collision

- Movement constants come from retail rodata. Retail 1.1 MP has no mantling,
  no swimming and no water jump.
- One cmd step for every caller: `crates/common/src/pmove/cmd.rs` (`chop`,
  `player_step`). The server, the client predictor and bots wrap it.
- The player is a capsule. A box sits `15 tan` higher on every grade.
- A spectator collides: a (-8,-8,-8)..(8,8,16) capsule under mask 0x800011
  (no playerclip, no bodies), stepping in the air because its
  `groundEntityNum` reads 0 (protocol doc, "Spectator").
- The ground trace runs origin ±0.25. A test that places a player exactly on
  a face lands allsolid and stuck. Place it 0.125 up.
- A brush hit reports the struck side's material (lump 3), not the brush's.
  A caulk brush's faces carry the plaster or rock a surface reads.
- No render soup collides. Model 0 clips through its brushes and lump 24
  (terrain partitions, patch facet grids). A patch's box is its subdivided
  grid grown by a unit, not its control points.
- Static props and 0x2080 kerb brushes clip bullets, blasts, missiles
  (clipmask 0x2802091 on all three) and items, never a moving player.
  `shot_trace`, `missile_trace` and `item_trace` see props, `box_trace` does
  not.
- A submodel's brushes, and the patches and terrain lump 27 gives it (bytes
  32..40, its run in lump 23), clip only while its entity is linked. Script
  `delete`, `notSolid` and `_gameobjects` unlink them. A world no script runs
  on applies the rule through `CollisionWorld::unlink_script_brushes`.
- A brush entity with no `*N` (any script `spawn` of one) links inline model
  0. It clips as a copy of the whole world where it stands, pushes like any
  brush model, goes out with `index` 0 and is clipped by client prediction
  (`CollisionWorld::set_world_copy`, movers doc 19-20). Brush bounds are the
  lump's grown a unit (`bsp::Model::clip_bounds`), before the link's own
  unit.
- A turned copy or submodel is traced in its own frame: the box's centre
  turned, not the origin, so a pitch shifts a player's capsule, and the box
  axial there, not in the world. A capsule meets the axial sides as its
  upright world box, `|turned sphere offset| + r` a side, and a non-axial
  side with the offset's y negated, so on a roll it is not upright.
- The turn takes `AngleVectors`' stored rounding (`x87::rotation_matrix`;
  `x87::angles_to_axis` for `AnglesToAxis`), the same in every engine and
  game module of the three servers, never float products. pmove's turns take
  the move world's `Arith` (`x87::angle_vectors_in`): the Windows cgames
  multiply by a float `pi / 180` and store `sr * sp` (movers doc 22,
  "pmove's turns").
- A trace an entity's clip cuts short of the world's ends where `SV_Trace`
  puts it, off the unshifted points (`sv_trace_endpos`), not where
  `CM_BoxTrace` would (movers doc 21, 22).
- A fall onto a slope slides as far as its cmd split lets it: a cmd that
  starts within the ground trace's reach turns the fall into ground speed.
  An A/B over one replays the client's three cmds a frame, never one (movers
  doc 21, "The slide down the slope").
- 1.5b's and United Offensive's engines are an unoptimized gcc 3.2.3 build
  that stores every float variable, where 1.1d's keeps them on the stack.
  What stores:
  - A parked vehicle's wheels read the last bit of the terrain point clip
    (`coduo-vehicles.md` 12, the second shove).
  - Patches are built with cracks along other facet seams, which a round
    slips through.
  - The brush clip stores its distances (`bsp-ibsp59-format.md`, "Patch
    collision on 1.5 and UO"), and so do the patch capsule arm and the
    terrain capsule clip, which reads the UO-built planes and edge frames.
  - pmove's fall stores its gravity, its tail and the trace's end point, and
    `PM_ClipVelocity`, `VectorNormalize` and `PM_Friction` store theirs
    (`cod15-movement.md` 7).
  - A missile's arc and bounce (`Missile::kept`, combat doc 12.5).
  - The locational bone trace (`BoneTraceCtx::arith`, 3.1). Its hit point on
    a body keeps the stack (`combat::trace_point`).

  A 1.5 or UO server sets
  `CollisionWorld::set_arith(Arith::for_protocol(..))`; a replay world in a
  test sets it too. A 1.5 or UO body's bones for a trace or a tag link come
  from the engine's own unnormalized anim records and skeleton build
  (`PoseBuffer::stored_worlds`, combat doc 3.1, "The pose on 1.5 and UO"),
  not the glam pose. The client's world runs `Arith::for_client()` on every
  protocol: the Windows clients are one MSVC build that stores some of those
  and keeps the rest (8), so a client replay of a Linux capture corrects in
  the last bits.
- A sweep, point or capsule, that meets a model-0 patch is clipped again in
  retail's BSP leaf order (`leafwalk.rs`). A patch clips only from a leaf on
  the path that lists it: mp_carentan's 69, 109, 283 and 284 are in no list
  and never clip; mp_railyard's 120 is missing from leaves its facets cross.
  A point tests its unpushed fraction against the trace's, so a brush or
  patch from an earlier leaf can keep it out (`bsp-ibsp59-format.md`, "The
  leaf walk"). The leaf order also decides:
  - a sweep whose contact nearly ties another (brushes a float step apart,
    terrain within its 1e-5 back-off of a brush or a partition, a submodel
    against the world);
  - a box stopped at fraction 0, a capsule stopped there on two contacts or
    starting all in solid;
  - a sweep stopped on terrain or a brush no leaf on its walk lists (out of
    the world, or a brush's pushed-out planes overreaching an unbevelled
    edge): it never clips.

  Leaf order names the surface a body standing on two floors steps on.
- A trace whose start is its end is a position test. Terrain reads
  `startsolid` only through `Terrain::position_test`.
- A push's candidate and spot tests are retail's box against brushes, not the
  capsule (`pusher_box_trace`, `box_position_test`). The candidate test is a
  whole trace that must name the pusher: a body in the world, or met first
  by another entity on the area walk, is passed through
  (`push::names_pusher`, movers doc 12). A sine mover never stalls: it
  crushes a player it cannot place (movers doc 21).
- A `Pmove` clips only clients linked in the area tree. A blocked push leaves
  its player out until his own next cmd, and a two-client gate on that orders
  the clients' packets by the capture's `w=` send stamps
  (`push_candidate_ab`).
- One spectator anywhere turns `StuckInClient` off on 1.1 only. 1.5's and
  UO's scans pass over him, so two players set down on one spot part at once
  there, in a `rand()` direction a gate replays from the glibc seed recovered
  off the capture (`Server::pin_stuck_rand`, player-clip doc 13.4).
- `setOrigin` and a teleport link at the unsnapped origin until the next cmd
  relinks.
- A brush model is traced at its live `r.currentAngles`, not its link's. A
  finished rotate leaves them truncated to 65536ths (60 reads 59.99634), so
  the clip turns by that, on all three servers (movers doc 22).
- A cmd's `Pmove` is one `movetrace::Pm`, handed to every chopped step's
  world (`MoveWorld::in_pmove`). 1.5's and UO's `PM_trace` drops bodies from
  its mask for the rest of the cmd once a trace starts in one, and its touch
  list is `ClientImpacts`' `"touch"` on both players, all three patches. Make
  one per cmd, never per step (cod11-player-clip.md 13).
- Wish speed multiplies the weapon's `moveSpeedScale` and the sight, back,
  strafe and lean scales: a plain carbine run is 224, sighted 89.7.
- Every mover's speed is `ps.speed` (`PlayerState::speed`): `g_speed` on 1.1,
  the client's `maxspeed` on 1.5 and UO, 400 for a spectator. A wire
  playerstate built by hand without `speed` predicts a body that never moves
  (cod15-movement.md, "ps.speed"). Gravity is `ps.gravity` likewise,
  `(int)g_gravity` on all three: one built without `gravity` never falls. A
  map load writes `g_gravity` back to 800, so a `+set` of it on retail's
  command line never sticks ("ps.gravity").

### Combat and items

- A 1.1 shot, swing or throw starts at the truncated eye over the truncated
  origin. 1.5's and UO's start at the unsnapped origin's eye plus the walk
  bob, never truncated (`combat::muzzle_point_in`, combat doc 2.1).
- 1.1 MP has no grenade cook; "cooking" in the code means held.
  `grenadeTimeLeft` is `fuseTime` at the pullback and 0 at the throw, and the
  fuse always runs in full from release. UO has a cook (see its section).
- A weapon file's millisecond keys are stored truncated (`weapon::stored_ms`:
  1.8 is 1799) on all three Linux servers (`coduo-movement.md` 13). Never
  round one.
- The fuse rides the fire event's parm inside vcod only. The wire ring
  carries 0, as retail sends.
- The explode rides the missile's own entity (`eType` flips to 0), so an
  `eType == 4` filter drops the explode frame.
- A missile's ground snap is a locational trace like its move: a frag rests
  on a player's posed bones with his number as its ground. 1.5 and UO drop a
  resting missile whose ground is not the world once nothing is under it
  (`Missile::falls`, combat doc 12.6); 1.1 leaves it hanging.
- A player's head model and his helmet each carry a `bip01 head` box, and the
  trace scores both. The grafted skeleton keeps an attachment's second box on
  a merged bone as a box-only child (combat doc 3.6).
- A standing body's torso yaw stays where his spawn left it until he moves,
  fires or goes prone (16.4), so a hit-pose capture swings it first.
- The engine's hit-box walk clips each box to the fraction already found. A
  higher priority beats a nearer box only when its bone comes first
  (`bonetrace::bone_trace`'s `limit`, combat doc 3.1).
- The aim block's sight sway reads the last frame's `level.time`, the cmd's
  (15.1).
- A stock frag bounces off a live player: `fraggrenade_mp` has `damage 0`.
  The move trace scores his posed bones under `bulletPriorityMap` and
  reflects off the struck bone's face, not his link box (combat doc 12.4).
  1.1's ground snap then lifts it 1.5 onto his bones. 1.5's and UO's snap
  onto the world only (12.2), and UO's bounce off a body keeps a tenth
  (coduo-diff.md 21). Off a surface 1.1 keeps 0.75 along it and 0.3 off it;
  1.5 scales the whole reflection by 0.5 and UO by 0.4 (12.4).
- On 1.5 and UO a prone body keeps the bend its last airborne cmd sampled
  (cod15-movement.md 4). A gate that drops a prone body sends the net probe's
  16/16/18 ms cmds (`ProbeHold::framed`), or its wire body and every bone
  land a fraction of a unit off retail's. Keep it opt-in: forced on, the UO
  vehicle hit gates and 1.1's prone-rest gate fail.
- 1.5's and UO's `ClientSpawn` thinks on the client's last cmd, not 1.1's
  null one (`ClientSim::spawn_cmd`). A client holding prone lies down along
  the spawn point's yaw before the script turns him, and a spectator flies
  off the spot on his held keys (cod15-movement.md 4, "The spawn's
  lie-down"). The think runs twice in vcod, both through
  `ClientSim::spawn_think_on`: on a fresh sim in the `spawn` builtin, for the
  origin the spawning thread reads, and on the real sim in the frame's spawn
  pass. A playing client's `angles` then read (0, the think's view yaw, 0):
  0 on 1.1, the spot's yaw through `SHORT2ANGLE` (180 is -180) on 1.5 and UO
  (cod11-gametypes-re-bel.md 8.6).
- 1.1's legs idle off `viewHeightLerpTarget` and move off the effective
  stance, never `pm_flags` alone (player-model-anim-system.md, "How retail
  picks the movetype"). A test that sets `ps.stance` by hand idles standing;
  `PlayerState::settle_stance` sets the whole stance.
- A blast, the touch pass, the use key and `positionWouldTelefrag` walk
  `crate::area::AreaTree`, retail's entity area tree. Victim and touch order
  is link history, not entity order, and an entity no link put in the tree is
  invisible to all four. A client links through `GameHost::link_client`,
  everything else through `GameHost::link_entity`. An `origin` write on a
  non-client relinks it. A player `setOrigin`, spawn, death or turret release
  (`TeleportPlayer`) is an unlink then a link (combat doc 14.7).
- Blast victims are measured per turn, after earlier callbacks ran. A client
  killed this frame keeps `takedamage` until its own end frame.
- `ClientEndFrame`'s dead arm (`pm_type` 6, no contents, off the wire) keys
  on `sessionstate` `dead`, not on health. A `suicide()` that stays
  `playing` keeps `pm_type` 0, and a test that kills through the vitals sets
  the session too (`ClientSim::session`, combat doc 5.1).
- `player_die` notifies the victim `death` with the attacker before the
  killed callback. UO's rank icon is destroyed on it (coduo-diff.md 22).
- A killed gunner keeps the turret until its own end-frame turn.
  `player_die` releases nothing.
- `setPlayerIgnoreRadiusDamage` is a level flag read only by the
  `radiusDamage` builtin.
- The body queue is eight entities at 64..71 with no timer. A corpse lives
  until its slot is reused.
- A temp entity takes its number from `G_Spawn`'s first-in-first-out free
  list like a script `spawn`, rides every snapshot, and is freed on the first
  entity pass more than 300 ms past its event (combat doc 14.7, "Entity
  numbers"). Engine notifies raised in the entity pass (`movedone`) wake
  their waiters on the next frame's threads.
- A body's head is a random draw from `xmodelalias/head_*`, and the `a` and
  `b` heads' boxes differ. A gate that reads head cells pins it
  (`probe_turret`'s `probe_head`, `cod11-turrets.md` 13.6).
- A `clipOnly` weapon (the frag) has no reserve entry on the wire.
- An item's `count` of 0 means unset. A drop writes -1 for empty.
- A `trigger_lookat` is never touched; it fires off `ClientEndFrame`'s aim
  trace. A `trigger_use` is never touched either (contents 0x200000); it
  fires off the use key's aim pick. A trigger-woken thread runs on the next
  frame's clock.
- A trigger's own `"trigger"` (touch, hurt, lookat, damage) goes through
  `G_RunFrame`'s queue, one per entity a round (`host.trigger_fires`, pushed
  through `GameHost::queue_trigger`, which refuses one past 0x100 for the
  caller to notify on the spot). What retail notifies on the spot (a touch,
  the use key, an item take, a mover's end, a blast's or a round's
  `"damage"` on an entity) goes in `host.engine_notifies`
  (`cod11-gsc-language.md`, "Engine notifies and the trigger queue").

### Script

- A BSP key reaches script only if it is in the entity field table or
  `radiant/keys.txt`. Anything else is dropped silently.
- A brush model's `.model` reads `""` (its `*N` key goes to `s.index`).
- `setTimerUp` has no `> 0` check, unlike the other three timers. `bel`
  starts with `setTimerUp(0)`.
- Threads due in one frame resume newest-queued first. `wait 0` resumes at
  once. A notify's waiters run after the notifier's step, in start order, and
  it reaches only the threads registered when it runs: a thread the notifier
  starts after it keeps its `endon` and its `waittill`.
- The script clock is not `level.time`. A thread due at a frame's
  `level.time` runs the frame after, so a `wait 1` from
  `Callback_StartGameType` at 0 wakes at 1050. Between frames the clock is
  the last frame's `level.time`; `Vm::set_time` starts it at a load. It is
  24-bit (`CLOCK_MASK`, wraps about 4.7 h into server uptime), and `wait`
  refuses a negative wait and one of 16777 s or more.
- A callback a cmd raises (damage, fall, `kill`) runs before the frame and
  reads the last frame's `getTime()`. Nothing on that path may advance
  `host.level_time_ms`.
- A thread's own `notify` doesn't fire its own `endon`.
- A runtime error ends its thread only at `developer 1`, `run_probe.sh`'s.
  vcod-server's default `developer 0` runs on past it as retail does (the
  failed instruction's results undefined, a conditional jump not taken) and
  still lists it in `script_aborts` (cod11-gsc-language.md, "Runtime errors
  below developer 1"). A gate replaying a probe capture that dies sets
  `developer 1`.
- After the bootstrap's first `wait` these are script errors
  (`GameHost::initializing`), even of a name already precached: `loadFx` of
  a new effect, `precacheModel`, `precacheShader`, `precacheString`,
  `precacheItem`, `precacheShellShock`. A test that precaches late fails
  there. So is `RegisterItem` of an item not yet registered (`giveWeapon`,
  `setWeaponSlotWeapon`, a script `spawn` of an item, a drop): a test that
  hands a player weapons by hand registers them first
  (`GameHost::precache_items_for_test`). Menus and icons may precache any
  time.
- Past that wait `setModel`, `attach`, `setViewModel`, `setShader`,
  `setText`, `label` and the objective icons only find a name already indexed
  (`Allocators::index_precached`). `statusicon`, `headicon` and `shellshock`
  never index one, so a test that sets one fills its configstring first.
- `attach` of a pair the entity already carries, or of a seventh model, is a
  script error, and its tag indexes nothing. A player's tags take their
  configstrings at his own end frame (`client_attachments`; object-model doc
  24). `addTestClient` runs the connect callback inside the call and the
  `begin` waiters after the caller's step (25).
- The weapon builtins queue `WeaponOp`s, and any pass may drain them (a
  vehicle round's callbacks drain every slot's). The playerstate's ammo is
  the queue's alone: `self spawn()` queues its emptying as
  `WeaponOp::TakeAll` and the spawn pass keeps `ps.ammo`. A reset of ammo
  outside the queue lands out of order with the loadout; a spawn came up with
  every weapon at 0 rounds (object-model doc 20).
- On 1.5 and UO every script thread runs before the frame's anim update, so a
  script `bulletTrace` poses bodies at the last frame's time
  (`GameHost::dobj_time_ms`). A UO spawn's `setModel` rebuilds the DObj in
  the thread, so its clips run a frame ahead (`ClientSim::dobj_ahead`,
  `cod11-turrets.md` 13.7). The two cancel until clips change.
- A thread due on the last frame's clock runs before the frame's anim update.
  A player bone it asks for keeps that pose, with every bone above it, for
  the rest of the frame (`GameHost::dobj_posed`, movers doc 15, "A bone asked
  for before the anim update"). Its `bulletTrace` and `radiusDamage` pose
  bodies at that clock too and fix every bone they pose for the frame, as a
  bone ask does (`GameHost::dobj_time_ms`, combat doc 3.6). A gate replaying
  a `--probe-uo-weapon direct` capture sends `ProbeHold::framed` cmds: on
  even ones `leanhit_ab`'s standing cells read 0.1 off. Where a `setOrigin`
  drop lands rides the exact cmd times, which `--probe-uo-weapon` and
  `--probe-vehicle` log as `CMDS` lines and `ProbeHold::times` replays.
- Every trace that poses a player, script or engine, builds his DObj through
  `GameHost::dobj_posed` (`combat::DobjStamps`). Each client packet (on
  retail the bots' pass as one) and `G_RunFrame` start a stamp. On 1.5 and
  UO each build eases the controls a step from the last build's, so a body
  nothing traces keeps its controls. `setPlayerAngles` tilts the traced body
  by its pitch and roll until his next cmd (cod11-combat.md 3.7). A new trace
  that skips the stamps poses a body retail never built.

### Assets, rendering, sound

- A coarse xmodel LOD ships no `xmodelparts`: its vertex bone ids index LOD
  0's bones. A model past its last LOD distance is not drawn at all (xmodel
  doc, "LOD selection").
- xanim translation keys are offsets from the bind pose. Root `tag_origin`
  sits at the feet.
- A non-player entity rotates through `AnglesToAxis`: positive pitch is nose
  down.
- A control bone (`back_*`, `neck`, `head`, `pelvis`) turns about the model's
  axes, not its own; about its own Y the body leans sideways. The torso
  pitch, torso yaw and legs yaw swing after the view on both server and
  client, and `tag_origin`'s local tag turns the body by the legs' yaw off
  the view, so a test that poses a turned body must step it to settle
  (combat doc 16.3, 16.4).
- A player's clip plays at the rate its ground speed gives, starts at a phase
  off the clip it replaces or the level time, and blends 120-250 ms (30 into
  a fire anim) (`vcod_common::animclock`, movers doc 15). Its clock runs on
  level time and the slot number, so a pose A/B replays cmds on retail's
  level times. The client runs the same clock on render time and measures
  the rate on the newest snapshot's time and `trBase`. Both pose through
  `playerpose::pose_player`: the torso and legs clips blend per bone as two
  tree nodes, weighted 1 and 0.01 under a torso anim.
- A 1.5 or UO server samples a clip at the engine's node time, a float
  stepped by 50 ms frames (`Layer::time`, `PoseInputs::node_time`), not its
  seconds (combat doc 3.1, "The clip's float time"). A UO seat's leaves each
  step their own (`SeatRamp::leaf_times`) and stand still with their weights
  while their legs node is faded out (`SeatRamp::fade_held`). An MG42
  gunner's fire group takes its leaves' weighted mean
  (`set_sync_frequency`).
- A gunner's turret group is a node over row nodes (`skeleton::Blend`), each
  row normalized to its weight. On 1.5 and UO its rows and leaves carry their
  weights from frame to frame (`turretpose::GunnerTree`, turrets doc 13.9,
  "The weights across frames"): a trace before the next end frame reads last
  frame's goals, a row or leaf just added at 0.001 times its goal, one just
  dropped at its old weight. A node any setter starts from 0 ramps from
  `0.001 * goal` (`Ramp::raised`), so a clip just set never reads 0.
- The 24/30/32 pt fonts span two or three atlas pages per glyph, named in the
  `.dat`. HUD colours are display values, scaled into the framebuffer by the
  material's gen (`colour_scale`) in the HUD pass.
- The scene target holds retail's framebuffer bytes: textures upload without
  sRGB decode and blends run in byte space. The overbright x2 lives only in
  the gamma pass's ramp, so a pass that draws a display value scales it by
  `identityLight` (0.5 full screen, 1 windowed) first. `vc_hdr 1` swaps the
  world's target for a float one that keeps the same units and is
  tonemapped back to bytes before the HUD (`crate::post`).
- Effect shaders live in `fxshaders/` in `pak5.pk3`. Some map paths have a
  leading slash.
- A level's gamestate prewarms its models, effects and their images
  (`prewarm_level` in `main.rs`). A new thing the client draws or plays off
  an event goes in that list, or its first use stalls a frame.
- The asphalt sound alias suffix is `asphault`, and vcod uses that spelling
  on purpose.
- The probes' flat floor on mp_carentan (`probe_legs`, `probe_bump`,
  `probe_passthru`, z -151.875 at y -376) is the map's bottom brush, 145
  under the garden terrain. A view from there shows walls floating over sky:
  the town from below, not a culling bug.
- Portal walk: a portal's plane faces out of its owning cell. vcod adds three
  over-marking rules in `common/src/vis.rs` for the mp_ship decks.
- No cell lists a brush model's soups. `WorldVis` marks them by the model's
  bounds against the frustum, and the renderer keeps only those whose model
  a snapshot entity carries (movers doc, "The client").

## United Offensive 1.51

### Wire

- A UO script model or turret carries its model in `xmodel`, not `index`
  (`G_DObjUpdate`). A brush model keeps its number in `index`.
- Protocol 22 sends an `eType` 12 entity (a vehicle) through a second
  netfield table behind a select bit after its number. A decoded
  `EntityState` keeps the main table's order whichever table carried it
  (`Cod1Wire::vehicle_field_slots`). Its `bits -100` fields are 16-bit
  angles held as f32 degrees; one equal to its base after quantizing is
  nudged a mantissa bit so a capture re-encodes.
- UO's `eType` 9..15 are its own (`flags::ET_VEHICLE_V22` and kin) and
  `ET_EVENTS` is 16: ask `Cod1Wire::event_of`, never `eType >= ET_EVENTS`.
- Its events, `eFlags`, `pm_flags` and means of death are mapped onto 1.1's
  at decode. The predictor's ring is too (`predict::from_wire`), or every own
  shot plays twice (`coduo-movement.md` 15).
- The configstring blocks and the fixed slots past 11 (UO's fog is 13, its
  start time 14) are `Cod1Wire::cs`, never the `CS_*` constants. The ammo and
  health items follow `Cod1Wire::max_weapons` weapon rows
  (`weapon_table::static_item`: 65..69, UO 129..133).
- UO's own events, its `EV_BULLET_HIT` and its projectile explodes keep their
  UO number under `EV_FOREIGN` (`EV_UO_*`). Its sprinting, winded and
  overheated `pm_flags` sit above bit 23 (`PMF_*_V22`), so a 1.1 handler
  never fires on one (`coduo-diff.md` 19).

### Movement and weapons

- UO's sprint button is `buttons` 0x8 (`msg::BUTTON_SPRINT_V22`), sent to a
  UO server only.
- An LMG's sight flag is its bipod (`PlayerState::deployed`). Nothing else
  sets 0x20 on one, and the deploy writes the stance itself
  (`coduo-movement.md` 7).
- A crouched `trigger_mount` reaches the client as hint 7 with value 0 (the
  server's own contents found it), so the predictor deploys it standing for a
  snapshot, as retail's would (11). The server's pmove point contents meet
  the mount triggers (`MoveWorld::with_boxes`); the client's never do.
- UO has a grenade cook, started only by the sight button's edge on a pulled
  grenade (`weapon::cook`, `coduo-movement.md` 12).

### Vehicles

- A seat's pmove moves nothing. The body is put at the seat tag in
  `ClientEndFrame`, so the predictor keeps the snapshot's origin. The view's
  12000 pitch clamp (16000 on foot) and the jeep passenger's arcs push
  `delta_angles` (`coduo-vehicles.md` 3).
- The server places the rider off the vehicle as posed before the frame's
  step, and the step reads the rider's last run cmd, not the last received
  (`coduo-vehicles.md` 11.4, 12).
- The cmds left in an exit's frame run linked and move nothing (`pm_type` 1
  until `G_RunClient`, 11.2) but stand the rider's legs up: 1.5's and UO's
  `PM_Footsteps` select a linked player's idle, airborne or not (14).
- An undriven vehicle thinks on its own turn in the entity pass, by number,
  the driven ones after it. A pushed vehicle moves the frame after the hit,
  and what is linked to it moves with it (12, "The think's turn").
- A vehicle's `eType` 12 and its sound blends' 9 are not 1.1's types. The
  server builds them with `game::wire::ET_RAW`, so the protocol map leaves
  the type alone and still maps the events.
- Vehicle clip brushes (contents 0x200) are in the collision world; only a
  vehicle's clipmask meets them.
- A vehicle's collmap clips as that vehicle's own model instance
  (`CollisionWorld::set_model_instance`; one inline model serves every
  vehicle of a kind), passed by its own traces as `Pusher::Copy(entity)`.
  Its brushes clip in lump order and a contact at fraction 0 ends them, so a
  later brush never reads a sweep all solid (`coduo-vehicles.md` 11.7).
- Its hull against rounds, missiles and blast probes is the xmodel's
  collision posed per bone (`vehicle::placed_models`), not the collmap. The
  server poses it with `Scr_Vehicle_Controller`'s controls only
  (`vehiclepose::apply_hull`): the steering wheel and front wheels stay at
  their bind there, unlike the drawn pose. A body's broad box is the fixed
  (-64,-64,-32)..(64,64,72), and a bone past what the trace already met never
  scores (`coduo-vehicles.md` 13, "The silhouette").

### Rendering and sound

- UO's impact tables spell their explode paths with backslashes. `Pk3Fs`
  reads `\` as `/`, as the engine's lookup does (coduo-diff.md 19,
  "Backslashed rows").
- A rider poses through the server's `Controllers::rider` and
  `playerpose::seat_inputs` on both sides, but the cgame scales the jeep's
  steer by 1/60 where the server takes 1/45: a drawn seat is
  `BodySeat::drawn` (`coduo-vehicles.md` 9, "Other riders").
- A sound blend entity (`eType` 9) is never drawn, so `step`'s entity
  positions never move its voice. `AudioSystem::set_sound_blends` places it
  on its vehicle every frame (coduo-diff.md 16).

### Bots

- A bot sprints only along `Follower::straight_run`: level, straight, no
  jump, leap or risky edge. A sprint takes no weapon scale, so the graph's
  190 is 304 there; a turn or a drop at that pace was never proved.
- `bas` breaches delete a base's walls, so a bot test on a `bas` map runs a
  second graph build.
- A vehicle moved since the build does not rebuild the graph. Once it holds
  still 20 ticks its nodes are blocked for the followers
  (`Server::park_vehicles`, `Follower::blocked`), and a bot hemmed in by them
  reads as stranded. A test that parks a vehicle by hand waits those ticks
  before the bots see it.

## CoD2 1.3

- The server builds its entities in CoD2's numbers as it builds them
  (`HostProfile::entities`, `cod2_wire::COD2_ENTITY_NUMBERS`: types,
  `eFlags`, `trType`s, ring events), held in vcod's `EntityState` over 118's
  table (`Cod2Wire::as_cod1_shim`, identity maps) until the engine takes
  `EntityState2`. `cod2_wire::to_cod2_entity` only copies, so a CoD2 builder
  that writes a 1.1 number sends it as is (cod2-protocol.md 15.9).

## CoD4 1.7

- A CoD4 bot never writes a message, so the commands it reads must be marked
  sent (`reliable_sent`) as well as acknowledged. 1.7's
  `SV_AddServerCommand` squashes only unsent ones; a squash of one already
  read moved the next level's team menu under the acknowledge, so no bot
  played past a map change.
- A CoD4 nav graph clips the brush entities standing once the level has run a
  frame (`bots::cod4::BrushedClip`), and its cache key carries their set. A
  test's `bot_world(.., Vec::new())` and `nav_build` clip the clip map alone.
- `CountPlayers`' shared `level.players` array is under
  [Script host and VM](#script-host-and-vm).
- Cmds are thought on arrival, so a hit lands between the victim's cmds of a
  frame as the two clients' packets arrived. A gate replaying two retail
  captures sends each frame's packets in the order retail got them
  (`cod4::capture_ab`); a fixed order read as damage applied a frame late
  (cod4-combat.md 15).
