# The server tick

Read this before moving work inside a server frame. Moving a step across this
order makes a snapshot or a later round read a frame-old field.

## One skeleton for every game

`Server` is `engine::Engine<Cod1Game>`, `Cod2Server` is
`engine::Engine<Cod2Game>`, `Cod4Server` is `engine::Engine<Cod4Game>`. Each
is an `engine::EngineCore` (`core`) beside the game. `engine::tick`
(`engine/game.rs`) is retail's `SV_Frame` order, calling the game's
`GameModule` hooks:

1. `frame_head` (CoD1's `Cbuf_Execute`, its `quit` and `killserver` stop).
   Then, when a frame runs, `HEAD`'s order of the bots, the pings and (CoD2,
   CoD4) configstrings 0/1, each once.
2. Per owed frame (at least `MIN_FRAMES`: CoD1 1, CoD2 and CoD4 0): the
   clock, the queued cmds under `ThinkTiming::InFrame` (CoD1's
   `replay_moves`; CoD2 and CoD4 think each cmd as its packet arrives,
   `OnArrival`), `run_frame`,
   the archive between frames, `frame_out`.
3. `SV_CheckTimeouts` on the advanced clock (the stale zombies freed there),
   then `engine::send_messages`, the one `SV_SendClientMessages` pump:
   `snapshots_begin` once, then in slot order each slot `message_due` gets
   `next_fragment` if one is left, else `send_client_snapshot`, then
   `snapshots_end`. The pump serves every state on its own slot's turn: a
   zombie's message and a client not in the world's bare one go out between
   their neighbours' snapshots (`cod11-server-handshake.md`, "The order of a
   frame's messages").
4. The last frame's archive under `ARCHIVE_AFTER_MESSAGES` (CoD1's
   `snapshots_end` archives its own), then the heartbeat.

CoD4 1.7 sets two more consts (`cod4-server-frame.md`):

- `POST_EACH_FRAME`: the timeouts, messages, archive and heartbeat follow
  every owed frame instead, so a hitch sends one message per frame.
- `FRAGMENTS_FIRST`: the pump sends every due slot's fragment before any new
  message.

| Const | CoD1 | CoD2 | CoD4 |
|---|---|---|---|
| `THINK` | `InFrame` | `OnArrival` | `OnArrival` |
| `HEAD` | `BotsPings` | `CsBotsPings` | `PingsBotsCs` |
| `MIN_FRAMES` | 1 | 0 | 0 |
| `ARCHIVE_AFTER_MESSAGES` | false | true | true |
| `POST_EACH_FRAME` | false | false | true |
| `FRAGMENTS_FIRST` | false | false | true |

A tick is one `SV_Frame`: every frame the wall clock owes runs back to back
with only the archive between them, then one round of timeouts and messages.

`main.rs`'s `serve<G: GameModule>` is the loop for all three games. It
queues the stdin console's lines (`stdin_console`, read on their own thread)
and runs `drain_console` before it counts what is owed, so a slow
`map_restart` is caught up with a single snapshot (`cod11-map-cycle.md` 4.7). It hands each
socket packet to `Engine::packet_read` with its `engine::Arrival` (CoD1
stamps the move acks there, `Server::ack_stamp`). `engine::packet` checks
`accepts_packets`, then splits `SV_PacketEvent` on `is_connectionless` into
the game's `connectionless` and `client_packet`.

A step whose order differs between games is one of the consts or hooks,
never a branch on the game.

## CoD1's frame

`Server::game_frame` and `Cod1Module::run_frame_cod1` (`server/cod1_frame.rs`)
follow `G_RunFrame` / `ClientThink_real` / `ClientEndFrame`. The comments at
each step cite the section that measured it. In order:

1. Every packet's cmds in arrival order, each one a `ClientThink_real` with
   its shots traced and their callbacks run inside the cmd
   (`cod11-combat.md` 16).
2. Menu responses, the script threads.
3. Spawns, weapon, link and sim ops.
4. The entity pass, one entity at a time by number: items, links, movers,
   missiles, thinks, temp entity frees and linked clients. Each blast is
   walked on its grenade's turn and each mover's push on its own (14.7).
5. Every slot's end frame, then per slot the aim trace, `commit_pose` and
   that slot's turret, whose rounds deliver inside its turn.
6. Outgoing commands.

A silent client goes on its sixth stale frame. A zombie is freed in
`SV_CheckTimeouts`.

CoD2's order is in `cod2-protocol.md` 14.1. CoD4's is in
`cod4-server-frame.md`.

## Panics

`vcod-server` contains a panic at the boundary where one input owns the
work (`engine::contain`), so a bug costs that input and not every
player's game. `serve` turns the boundaries on for its thread once the
level is up; an in-process server (the client's) and the tests leave them
off and see a panic unwind as before.

| Panic in | Costs |
|---|---|
| a connectionless packet's handler | the packet; one log line a second at most |
| a client's netchan packet (`client_packet`) | the client: `GameModule::drop_sender` drops the slot holding the packet's address and qport with the reason `server error` |
| a script builtin | the script thread that called it, as `ErrorKind::Panic` (the VM's other terminal errors end a thread the same way); the frame runs on |
| the frame (`tick_frames`), the loop's console drain (`exec_console`), or a panicking client's drop | the level: the loop queues `map <mapname>` (CS 0's), the console's own map change. On CoD1 that is a `map_restart` (cod11-map-cycle.md 4.2) |

- The fourth level panic inside 60 s, or one with no map named, ends the
  process through `Engine::fatal`, as the failed level load does: a panic
  the load itself repeats then stops instead of looping.
- Retail's `Com_Error(ERR_DROP)` path is not recorded for these servers;
  loading the map again is vcod's choice. Exiting would end every player's
  game for one bad input, and the reload replaces the level's state, the
  part a panic leaves unknown.
- What a caught handler had changed stays. The drop and the reload
  replace the client's slot and the level; a connectionless handler holds
  little, though a `connect` that panics after taking a slot leaves that
  slot to its timeout. A builtin's partial work on the script heap stays.
- The binary's panic hook logs a caught panic with its location and
  leaves the tty console alone; an uncaught one restores the tty and
  unwinds as before. A bots' worker counts as caught: its pool re-raises
  on the ticking thread (`bots::driver::par`).
- A test arms a panic at a boundary through `contain::inject` (`cfg(test)`
  only).

## Engine state

`EngineCore` is the engine state every game drives: transport, slot table,
configstrings and their sent copy, console queue, challenges, query limiter,
bans, masters, rcon limiter, the game clock and `rand()`. A slot's netchan, acks, frame and
ping rings, pacing, fragments and timeout clock are its `link`
(`engine::slot::Link`).

- What every server answers alike over it (`tick_frames`, `handle_packet`,
  `push_console`, `take_outgoing`) is one `impl<G: GameModule> Engine<G>`.
- CoD2's and CoD4's slots are one `engine::slot::Conn`, and what the two
  engines run alike between a datagram and the level (the netchan's
  acceptance, the reliable ring, `transmit`'s pacing, `message_due`,
  `next_fragment`, `flush_game_commands`) is one
  `impl<G: ConnGame> Engine<G>` (`engine/conn.rs`); a step that differs
  is a `ConnGame` hook.
- The frame writers are the wire's (`WireGame::write_gamestate`,
  `write_snapshot`). A game builds its frame in its own numbering (CoD1 maps
  1.1's once a snapshot).
- `getinfo` and `getstatus` reply through `engine::answer_status`: the
  game's `GameModule::status` text framed by `WireGame::oob_packet`.
- `Cod1Game` (`server/cod1_game.rs`) still holds the engine state CoD1 keeps
  itself (zombies, pure check, rcon password and redirect, the killcam
  archive, `cfg`) beside its game module, `Cod1Game::module` (`Cod1Module`).
  `Lend` is what the engine lends the module for one call.

## The bots' threads

The bots' pass stays where `HEAD` puts it, on the ticking thread, and
every write it makes stays there, in slot order. Inside it
(`bots::driver::step`) two kinds of per-bot work run on worker threads
over `&` reads of the driver and the world:

- each bot's cover look (`cover::find`), taken after every bot's sense;
- each bot's plan (`driver::plan`): the retreat's cover, the hunt's
  vantage, the way given, the goal, the route (A*, the follower's walks
  and traces) and the drop and wall probes, over the state pass 3 starts
  with, the game's own hooks (`pre_route`, `pre_think`) left out.

Pass 3's loop then runs as on one thread and takes a planned part only
where its reads match: a route's game reads (`RouteIn`), view, goal, the
bodies it gave way to and a budget of at least a search's slice; a
probe's feet and the creases it felt (`Creases::merge`). Anything an
earlier bot's turn or a hook changed fails the check and is worked out
there and then, so the bots move as on one thread: `bot_threads::`
(testing.md) runs one seeded match per game both ways and compares every
tick. What stays on the ticking thread: the commands, the enemy sight
(the games' levels are not `Sync`), the objective deal, the hooks, the
lob probe, `think` and the cmds (`bots/moves`).

The workers are a pool per ticking thread (`driver::par`), woken after
the nav tick so they are on a core by the plans, spinning until the plans
are in, parked between ticks. `VCOD_BOT_THREADS=<n>` sets the threads, the
ticking one among them (default the cores up to 4; `1` runs the pass as
before it had workers). Measured in bot-navigation.md section 3, "The
pass on threads".

## Tracing a tick

- `VCOD_TICK_TRACE=<path>` on `vcod-server` writes the tick's step log
  (`tick_trace`): a line per `SV_Frame` step, named after the retail
  function, with the slot it ran for, in execution order.
- `VCOD_TICK_PROFILE_MS=<ms>` logs each tick past `<ms>` with its phases'
  times and its wall time split into run, queued and asleep
  (`tick_profile`). The CoD4 soak's `VCOD_SOAK_TICK_LOG_MS` and
  `bot_match`'s `SPIKE_MS` print the same (cod4-server-frame.md 9).
