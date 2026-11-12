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

## Engine state

`EngineCore` is the engine state every game drives: transport, slot table,
configstrings and their sent copy, console queue, challenges, bans, masters,
rcon limiter, the game clock and `rand()`. A slot's netchan, acks, frame and
ping rings, pacing, fragments and timeout clock are its `link`
(`engine::slot::Link`).

- What every server answers alike over it (`tick_frames`, `handle_packet`,
  `push_console`, `take_outgoing`) is one `impl<G: GameModule> Engine<G>`.
- The frame writers are the wire's (`WireGame::write_gamestate`,
  `write_snapshot`). A game builds its frame in its own numbering (CoD1 maps
  1.1's once a snapshot).
- `getinfo` and `getstatus` reply through `engine::answer_status`: the
  game's `GameModule::status` text framed by `WireGame::oob_packet`.
- `Cod1Game` (`server/cod1_game.rs`) still holds the engine state CoD1 keeps
  itself (zombies, pure check, rcon password and redirect, the killcam
  archive, `cfg`) beside its game module, `Cod1Game::module` (`Cod1Module`).
  `Lend` is what the engine lends the module for one call.

## Tracing a tick

- `VCOD_TICK_TRACE=<path>` on `vcod-server` writes the tick's step log
  (`tick_trace`): a line per `SV_Frame` step, named after the retail
  function, with the slot it ran for, in execution order.
- `VCOD_TICK_PROFILE_MS=<ms>` logs each tick past `<ms>` with its phases'
  times and its wall time split into run, queued and asleep
  (`tick_profile`). The CoD4 soak's `VCOD_SOAK_TICK_LOG_MS` and
  `bot_match`'s `SPIKE_MS` print the same (cod4-server-frame.md 9).
