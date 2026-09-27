# CoD 1.1 MP: spectator follow

How a spectator's view rides another client: where the state lives, which
buttons move it, what the copied playerstate carries and what the spectator
keeps of its own, when a follow ends and where the spectator is left, which
entities a follower is sent, and what the killcam, which rides the same
fields, would need on top.

Evidence rules as everywhere in this directory. This document carries no
document-level default: every claim carries its own label. VERIFIED is a byte
read out of a module, an asset or a live capture. INFERRED is anything read
off control flow, which includes the order of two stores and every branch
condition, and anything that says what a field means.

Modules: `game.mp.i386.so` (the 1.1d Linux dedicated server's MP game
module), addresses as `nm -D` and `tools/re/annotate_func.py` print them; a
Ghidra export loading it at +0x10000 shows each function 0x10000 higher.
`cod_lnxded` (the 1.1d engine), addresses virtual as `objdump -d` prints
them. `cl` is a `gclient_t`, whose playerstate sits at `cl+0`.

## 1. The state

| offset | meaning | evidence |
|---|---|---|
| `cl+0x20d0` | `sessionState`: 0 playing, 1 dead, 2 spectator, 3 intermission | VERIFIED, `ClientConnect` 0x42520 stores 2; the value names are `cod11-gsc-object-model.md`'s |
| `cl+0x20d4` | the script field `spectatorclient`, the forced follow | VERIFIED, client field table entry 0xc00a, offset 8404 (`cod11-gsc-object-model.md`) |
| `cl+0x20dc` | the script field `archivetime`, the killcam's replay age | VERIFIED, client field table entry 0xc00b, offset 8412 |
| `cl+0x21d4` | the client being followed or cycled to, -1 for none | VERIFIED, `Cmd_FollowCycle_f` 0x490c9 stores the slot it found there; the meaning is INFERRED |
| `cl+0x21e8` / `+0x21ec` | the buttons of the cmd just run and of the one before | VERIFIED, `SpectatorThink` 0x3fad3..0x3fae6 moves `+0x21e8` into `+0x21ec` and the cmd's buttons byte (`cl+0x20f4`) into `+0x21e8` |
| `ps.pm_flags` 0x10000 / 0x20000 / 0x40000 | following / a forced follow / own view | VERIFIED setters in `cod11-events-and-fx.md` section 7 |

- VERIFIED: `ClientConnect` writes -1 to `cl+0x21d4` (0x4252a) and to
  `cl+0x20d4` (0x42534), so `spectatorclient` reads -1 before any script
  writes it.
- VERIFIED: `ClientSpawn` carries two 0x41-dword copies between `cl+0x20d0`
  and a local (0x427ea, 0x42824), a `bzero` of the whole 0x22c4-byte
  `gclient_t` (0x42804) and a store of -1 at `cl+0x21d4` (0x4282c). INFERRED
  from their order: a spawn
  keeps `spectatorclient` and `archivetime` and drops the follow target, and
  its zeroed playerstate carries none of the three `pm_flags` bits until the
  next end frame.

## 2. The buttons: `SpectatorThink`

`ClientThink_real` (0x3fee0) calls `SpectatorThink` (0x3fab8) for a client
whose `sessionState` is 2, once per usercmd. INFERRED, from the compare
against 2 on `cl+0x20d0` ahead of the call.

- INFERRED (0x3fb0b..0x3fb2e): `StopFollowing` runs when `spectatorclient` is
  negative, `cl+0x21d4` is not, and bit 0x10 (`BUTTON_ADS`) differs between
  the latched buttons and the previous ones. Either edge counts: the press
  and the release.
- INFERRED (0x3fb3c..0x3fb7c): a rising edge of bit 0x1 (`BUTTON_ATTACK`)
  calls `Cmd_FollowCycle_f(ent, 1)`; otherwise a rising edge of bit 0x20
  (`BUTTON_MELEE`) calls it with -1.
- VERIFIED: the free-flight arm stores `pm_type` 4 (0x3fb94) and `speed` 400
  (0x3fb9b). INFERRED: it is skipped when `pm_flags` 0x10000 is set
  (0x3fb8a), so a follow the last end frame landed runs no pmove.

## 3. The cycle: `Cmd_FollowCycle_f`

- INFERRED (0x4905f): it does nothing when `spectatorclient >= 0`, so the
  buttons neither cycle nor end a forced follow.
- INFERRED (0x49074..0x490b4): it starts from `cl+0x21d4`, or 0 when that is
  negative, steps by the direction, wraps at `level.maxclients` in both
  directions, asks `trap_GetArchivedPlayerState` (0x490a8) for each slot and
  stops at the first that answers; the start slot is the last one tried.
- VERIFIED: on success it stores the slot at `cl+0x21d4` (0x490c9) and 2 at
  `cl+0x20d0` (0x490d5).
- VERIFIED live, 2026-09-27, `tools/run_server.sh mp_carentan` (dm), players
  in slots 0 and 1 (`--probe-team allies` and `--probe-target --probe-team
  axis`) and `--probe-follow` in slot 2 of 8: the first attack press landed
  on 1, the second on 0, the melee press back on 1. That is the walk above
  from a start of 0: slot 1 first, then 2 (the spectator, which does not
  answer), 3 to 7 (empty), 0.

## 4. Who can be followed

- VERIFIED: `trap_GetArchivedPlayerState` is game syscall 0x42 (0x73bac),
  which `cod_lnxded` dispatches to 0x808ef7c.
- INFERRED (0x808ef8f..0x808efc1): with no archived frame for the requested
  age (0x808eeb8 answers none) and an age below 1, it asks the game module's
  export 8 when the client's engine state reads 4 (`CS_ACTIVE`, 0x808efaf),
  and fails otherwise.
- VERIFIED: export 8 is `GetFollowPlayerState` (0x415c4, `vmMain` case 8).
  INFERRED (0x415dc): it refuses a client whose `pm_flags` lacks 0x40000;
  VERIFIED: otherwise it copies 0x834 dwords of the client (0x415ea), zeroes
  the 0xd90 bytes at `ps+0x5a8` (0x415fb) and returns 1 (0x41600).
- INFERRED: 0x40000 is set by `ClientEndFrame` for a client whose
  `sessionState` is neither 2 nor 3 (`cod11-events-and-fx.md` section 7), so a
  dead client is followable and a spectator is not.
- VERIFIED live, both runs of section 9: the follower stayed on the target
  through its death (`pm_type` 6, `health` 0) and, in dm, through its respawn
  at a new spot.
- INFERRED, from `cod11-combat.md` 4.2 step 9: `player_die` sends
  `Cmd_Score_f` to every spectator whose target is the victim, so a follower
  is pushed the scoreboard on the death.

## 5. The copy: `SpectatorClientEndFrame`

`ClientEndFrame` calls it for `sessionState` 2 (0x40f24, 0x40f30; object-model
doc). INFERRED from `G_RunFrame`'s loop: it runs in slot order, so a
spectator reads a lower slot's own-view bit from this frame and a higher
slot's from the last.

- VERIFIED stores: `svFlags` loses 0x2 and gains 0x1 (0x40778..0x40782),
  `ent+0x171` 0 (0x40788), `r.contents` 0 (0x4078f), `cl+0x220c` and
  `cl+0x2210` 0 (0x40799, 0x407a3), `ps+0xbc` 0 (0x407ad), `pm_flags`
  loses 0x40000 (0x407b7).
- INFERRED (0x407c1..0x40859): when `spectatorclient >= 0` it becomes the
  target, and the copy is retried with `archivetime` lowered 50 at a time
  until it answers or the age reaches 0; a copy that still fails writes -1 to
  both `spectatorclient` and the target.
- INFERRED (0x40865..0x40890): with the target negative, or its copy
  refused, `StopFollowing` runs (0x4091f) and nothing is copied.
- VERIFIED: the copy is 0x834 dwords over the spectator's own (0x408ba),
  which covers the whole playerstate, `clientNum`, `commandTime` and the
  event ring included, and ends before `sess` at `cl+0x20d0`.
- VERIFIED: it calls `HudElem_UpdateClient` with array mask 2 (0x408bc),
  the mask that rebuilds only the unarchived half at `ps+0x5a8`. INFERRED: the
  follower sees the followed client's archived HUD and its own unarchived one
  (`docs/protocol-1.1.md`, block 5).
- VERIFIED stores: `pm_flags` loses 0x40000 (0x408e0), gains 0x10000
  (0x408ea), and gains 0x20000 (0x40903) or loses it (0x408fd); `eFlags` is
  the copied word with 0x20000 taken from the spectator's own (0x408a5,
  0x408c1, 0x40910). INFERRED (0x408f4): `spectatorclient`'s sign picks
  between the two 0x20000 stores.
- VERIFIED live (section 9): a free follow's frame reads `pm_flags` 0x10000
  exactly, and its `clientNum`, `commandTime`, origin, `pm_type`, `eFlags`,
  `health` and `weapon` are the followed client's.

## 6. Which entities a follower is sent

- VERIFIED: the snapshot builder copies the client's playerstate (0x808f254),
  loads its `clientNum` (0x808f25f), takes the eye as `origin` plus
  `ps+0xd0` (0x808f288..0x808f2a0) and passes the `clientNum` to the entity
  pass (0x808f301).
- INFERRED: the pass skips the entity numbered `clientNum` (0x808e35b) and
  applies the `svFlags` 0x800 and 0x2000 single-client tests against the same
  number (`cod11-events-and-fx.md` section 2). A follower is therefore culled
  from the followed eye, is not sent the followed client's own entity, and
  gets that client's 175/176 and not the 173/174 copy.
- VERIFIED live: the free spectator was sent players `[0, 1]`; following 1 it
  was sent `[0]`, and following 0 it was sent `[]`.

## 7. Letting go: `StopFollowing`

- VERIFIED: it writes -1 to `spectatorclient` (0x46a3d) and to the target
  (0x46a47). INFERRED (0x46a51): everything after runs only when `pm_flags`
  0x10000 is set, i.e. when the last end frame copied.
- VERIFIED constants: the spot is a `trap_TraceCapsule` (0x46b54) with box
  -8 (0x73950) to 8 (0x73954), mask 0x810011 (0x46b39) and pass entity 0x3ff,
  from the eye (`origin` plus `ps+0xd0`, with `G_AddLean`) to the eye plus
  forward times -40 (0x73948) plus up times 10 (0x7394c), both vectors off
  the copied view angles. The view gets 15 (0x73944) added to its pitch and
  goes to `SetClientViewAngle` (0x46be2).
- VERIFIED stores: `ps+0xb8` 0 (0x46b6e), `clientNum` the spectator's own
  (0x46b86), `eFlags` byte 1 masked with 0x3f (0x46b8c, clearing 0xc000),
  `ps+0x374` 0, `ps+0x378` 0x3ff, `ps+0x380` 0 (0x46b93..0x46ba7),
  `pm_flags` loses 0x10020 (0x46bb1), the trace end becomes `ps.origin`
  (0x46bc8..0x46bd7), and `ps+0x3dc..0x3e4` are zeroed (0x46be7..0x46bfb).
- VERIFIED live, dm: the sight press put the spectator at the followed eye
  moved 27.9 back and 7 up (fraction 0.7, a wall behind), pitch 15, and its
  `health` 100 and `weapon` 9 were still the followed client's. INFERRED: the
  rest of the copied playerstate stays until something writes it.
- VERIFIED live, sd: when the followed client went spectator 2 s after its
  death, the follower's next frame read its own `clientNum`, 40 back and 10
  up from the dead eye (view height 8), pitch 15, and still `pm_type` 6; the
  frame after read `pm_type` 4.

## 8. A disconnect

- INFERRED (0x42b25..0x42b5b): `ClientDisconnect` walks every connected
  client whose `sessionState` is 2 and whose target is the leaver, calls
  `Cmd_FollowCycle_f(ent, 1)` (0x42b4b) and `StopFollowing` (0x42b5b) when
  that finds nothing.
- VERIFIED live, dm: when the followed probe's run ended the follower's next
  change read `clientNum` 0, the remaining player.

## 9. The retail runs

Both on 2026-09-27 against `tools/run_server.sh mp_carentan` on port 29012,
the follower being `--net-probe --probe-follow`, which presses attack at 8 s,
attack at 14 s, melee at 20 s, the sight for 2 s at 26 s and attack at 32 s
after going active, and prints every snapshot whose `clientNum`, `pm_type`,
`pm_flags` or `eFlags` moved.

- dm: `--probe-team allies` (slot 0), `--probe-target --probe-team axis`
  (slot 1, killing itself every 45 s and ending at 120 s), the follower in
  slot 2. The sections above cite it as "dm".
- sd (`+set g_gametype sd`): the same three, the target started 4 s before
  the follower so that its death at +10 s and its `spawnSpectator` 2 s later
  fall between the follower's first two presses. The sections above cite it
  as "sd".
- VERIFIED live, sd: at the round's restart the follower, which had been
  following slot 0, read its own `clientNum` and `pm_type` 4 at the
  spectator spawn. INFERRED: that is `ClientSpawn`'s drop of the target.

## 10. Script

- VERIFIED, `maps/MP/gametypes/*.gsc` in `pak5.pk3`: every `spawnPlayer`,
  `spawnSpectator` and `spawnIntermission` writes `spectatorclient` -1 and
  `archivetime` 0; `killcam` writes the attacker's number and `delay + 7`
  (dm.gsc 766..767); sd.gsc and re.gsc's bomb and goal cameras write
  `level.playercam`; every gametype's `main` calls `setarchive(true)`
  (dm.gsc 118).
- VERIFIED: `setarchive` is the builtin at 0x5f704, which passes its bool to
  syscall 0x45 (0x73c7c); `cod_lnxded` 0x808b4ac stores it and allocates the
  archive's buffers (0x440000, 0x2130000, 0x2580, 0x2000000 and 0x3800
  bytes) the first time it is set.

## 11. vcod

`crates/server/src/follow.rs` holds the cycle, the stop spot and the flag
patch; `ClientSim::spectator_think` and `stop_following` are sections 2 and
7; `Server::replay_moves` runs the think per cmd for a client whose
`sessionstate` is spectator and skips its pmove while the follow is on;
`follow_end_frame` is section 5 in the end-frame slot loop;
`pass_followers_on` is section 8 from `drop_client`; a death the vitals
mirror sees first sends the scoreboard to the victim's followers (section
4); `send_snapshots` builds a follower's frame from the followed client's
wire playerstate, eye and number. `spectatorclient` starts at -1 and is written back to -1 where
retail writes it.

Where it is not retail's:

- The follower's frame is built when the snapshot is written, from the
  followed client's state after its own end frame. INFERRED from section 5's
  slot order: retail's copy for a follower numbered below its target predates
  that target's end frame by one frame, which shows in the fields the end
  frame writes (the pain event, the damage feedback, the dead `pm_type`).
- After a follow ends vcod's spectator is its own playerstate again, velocity
  zero; retail keeps the rest of the copy (section 7).
- A `sessionstate` moved off spectator without a `spawn` while following
  takes retail's `ClientSpawn` arm in `ClientEndFrame` (object-model doc,
  the `ps.clientNum != ent->s.number` branch); vcod just drops the follow.
- A forced follow with `archivetime` above 0 is the killcam, which vcod does
  not serve: nothing is copied and the fields are left for the stock
  `archivetime <= delay` branch, which reads the 0 vcod stores and clears
  them. Retail with an empty archive would follow the attacker live instead
  (section 5's retry reaches age 0).

## 12. What the killcam would need

- The archive. INFERRED from `cod_lnxded` 0x808fb84, which runs only while
  `setarchive` is on: every frame it writes a frame record into a ring of
  0x200 (0x83b67f0), a record of 0x2130 bytes per connected client into a
  ring of 0x1000 (0x83b67ec) holding that client's playerstate as export 8
  answers it, and a record of 0x110 bytes per linked entity into a ring of
  0x4000 (0x83b67e8), and writes the same through the `MSG` delta writers.
- The age. INFERRED from 0x808eeb8: the requested age is converted to frames
  through `sv_fps` (0x808eef4) and clamped to the last 0x4b0 frames
  (0x808ef09), and the clamp is written back to the script's `archivetime`,
  which is what the stock `wait 0.05; if(self.archivetime <= delay)` reads.
- The time shift. INFERRED from 0x808ef7c's adds after the copy: the
  record's age is added to each non-zero one of `commandTime`, `ps+0x10`,
  `ps+0x38`, `ps+0x64`, `ps+0xd4` and `ps+0x3e0`, to four time fields in
  each of the 31 archived HUD elements, and to `ps+0x20cc`.
- The entities. INFERRED from the branch at 0x808f211 on the archived frame:
  the snapshot builder takes a killcam frame's entity list from the archive,
  not from the live world.
- vcod would need a ring of every client's wire playerstate and entity list
  per frame, the seconds-to-milliseconds `archivetime` setter with the trim
  written back, the time shift, a snapshot path that sends the archived
  entity list to a spectator whose `archivetime` is above 0, and
  `ClientEndFrame`'s `ClientSpawn` arm for the return to `dead`.
