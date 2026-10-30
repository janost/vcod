# CoD 1.1 MP: spectator follow

How a spectator's view rides another client: the state, the buttons, what
the copied playerstate carries and what the spectator keeps, when a follow
ends and where it leaves the spectator, which entities a follower is sent,
the killcam (section 12, the same fields over a replay) and what a spawn
leaves in the playerstate (section 13).

No document-level default: every claim carries its own label. VERIFIED is a
byte read out of a module, an asset or a live capture. INFERRED is anything
read off control flow (the order of two stores, every branch condition) and
anything that says what a field means.

Modules: `game.mp.i386.so` (1.1d Linux MP game module), addresses as `nm -D`
and `tools/re/annotate_func.py` print them; a Ghidra export loading it at
+0x10000 shows each function 0x10000 higher. `cod_lnxded` (1.1d engine),
virtual addresses as `objdump -d` prints them. `cl` is a `gclient_t`; its
playerstate sits at `cl+0`.

## 1. The state

| offset | meaning | evidence |
|---|---|---|
| `cl+0x20d0` | `sessionState`: 0 playing, 1 dead, 2 spectator, 3 intermission | VERIFIED, `ClientConnect` 0x42520 stores 2; value names from `cod11-gsc-object-model.md` |
| `cl+0x20d4` | script field `spectatorclient`, the forced follow | VERIFIED, client field table entry 0xc00a, offset 8404 (`cod11-gsc-object-model.md`) |
| `cl+0x20dc` | script field `archivetime`, the killcam's replay age | VERIFIED, client field table entry 0xc00b, offset 8412 |
| `cl+0x21d4` | client being followed or cycled to, -1 for none | VERIFIED, `Cmd_FollowCycle_f` 0x490c9 stores the found slot there; the meaning is INFERRED |
| `cl+0x21e8` / `+0x21ec` | buttons of the cmd just run and of the one before | VERIFIED, `SpectatorThink` 0x3fad3..0x3fae6 moves `+0x21e8` into `+0x21ec` and the cmd's buttons byte (`cl+0x20f4`) into `+0x21e8` |
| `ps.pm_flags` 0x10000 / 0x20000 / 0x40000 | following / forced follow / own view | VERIFIED setters in `cod11-events-and-fx.md` section 7 |

- VERIFIED: `ClientConnect` writes -1 to `cl+0x21d4` (0x4252a) and to
  `cl+0x20d4` (0x42534), so `spectatorclient` reads -1 before any script
  writes it.
- VERIFIED: `ClientSpawn` carries two 0x41-dword copies between `cl+0x20d0`
  and a local (0x427ea, 0x42824), a `bzero` of the whole 0x22c4-byte
  `gclient_t` (0x42804) and a store of -1 at `cl+0x21d4` (0x4282c). INFERRED
  from their order: a spawn keeps `spectatorclient` and `archivetime` and
  drops the follow target. The zeroed playerstate gets the own-view bit back
  from the spawn's own `ClientEndFrame` (section 13).

## 2. The buttons: `SpectatorThink`

`ClientThink_real` (0x3fee0) calls `SpectatorThink` (0x3fab8) once per
usercmd for a client whose `sessionState` is 2. INFERRED, from the compare
against 2 on `cl+0x20d0` ahead of the call.

- INFERRED (0x3fb0b..0x3fb2e): `StopFollowing` runs when `spectatorclient`
  is negative, `cl+0x21d4` is not, and bit 0x10 (`BUTTON_ADS`) differs
  between the latched and previous buttons. Press and release both count.
- INFERRED (0x3fb3c..0x3fb7c): a rising edge of 0x1 (`BUTTON_ATTACK`) calls
  `Cmd_FollowCycle_f(ent, 1)`; otherwise a rising edge of 0x20
  (`BUTTON_MELEE`) calls it with -1.
- VERIFIED: the free-flight arm stores `pm_type` 4 (0x3fb94) and `speed` 400
  (0x3fb9b). INFERRED: it is skipped when `pm_flags` 0x10000 is set
  (0x3fb8a), so a follow the last end frame landed runs no pmove.

## 3. The cycle: `Cmd_FollowCycle_f`

- INFERRED (0x4905f): it does nothing when `spectatorclient >= 0`, so the
  buttons neither cycle nor end a forced follow.
- INFERRED (0x49074..0x490b4): it starts from `cl+0x21d4` (0 when negative),
  steps by the direction, wraps at `level.maxclients` both ways, asks
  `trap_GetArchivedPlayerState` (0x490a8) for each slot and stops at the
  first that answers. The start slot is tried last.
- VERIFIED: on success it stores the slot at `cl+0x21d4` (0x490c9) and 2 at
  `cl+0x20d0` (0x490d5).
- VERIFIED live, `tools/run_server.sh mp_carentan` (dm), players in slots 0
  and 1 (`--probe-team allies`, `--probe-target --probe-team axis`),
  `--probe-follow` in slot 2 of 8: the first attack press landed on 1, the
  second on 0, the melee press back on 1. That is the walk above from a
  start of 0: slot 1, then 2 (the spectator, no answer), 3 to 7 (empty), 0.

## 4. Who can be followed

- VERIFIED: `trap_GetArchivedPlayerState` is game syscall 0x42 (0x63bac),
  which `cod_lnxded` dispatches to 0x808ef7c.
- INFERRED (0x808ef8f..0x808efc1): with no archived frame for the age
  (0x808eeb8 answers none) and an age below 1, it asks the game module's
  export 8 when the client's engine state reads 4 (`CS_ACTIVE`, 0x808efaf),
  and fails otherwise.
- VERIFIED: export 8 is `GetFollowPlayerState` (0x415c4, `vmMain` case 8).
  INFERRED (0x415dc): it refuses a client whose `pm_flags` lacks 0x40000.
  VERIFIED: otherwise it copies 0x834 dwords of the client (0x415ea), zeroes
  the 0xd90 bytes at `ps+0x5a8` (0x415fb) and returns 1 (0x41600).
- INFERRED: `ClientEndFrame` sets 0x40000 for a client whose `sessionState`
  is neither 2 nor 3 (`cod11-events-and-fx.md` section 7), so a dead client
  is followable and a spectator is not.
- VERIFIED live, both runs of section 9: the follower stayed on the target
  through its death (`pm_type` 6, `health` 0) and, in dm, through its
  respawn at a new spot.
- VERIFIED, `player_die` (0x49a48, `cod11-combat.md` 5.1 step 9): the call
  to `Scr_PlayerKilled` is at 0x49bb6; the loop after it compares
  `cl+0x20ec` with 2 (0x49bd9), `cl+0x20d0` with 2 (0x49be2) and `cl+0x21d4`
  with the victim's entity number (0x49bed), calls `Cmd_Score_f` (0x49bf9)
  and steps 0x22c4 (0x49c07). `Cmd_Score_f` (0x48f8c) calls
  `DeathmatchScoreboardMessage` (0x459c0), whose one send is
  `trap_SendServerCommand` (0x45b42).
- INFERRED, from those compares and their order: every connected spectator
  following the victim is sent the `b` scoreboard once per death, after the
  killed callback has run to its first `wait`, so the rows carry the score
  and deaths that callback wrote. Nobody else is sent one.
- VERIFIED: `player_die` is reached from `Cmd_Kill_f` (call at 0x4960b),
  from the `suicide` builtin (0x45358, call at 0x453e5) and through the `die`
  pointer at `gentity+0x218`, which `ClientSpawn` sets to `player_die`
  (0x42753) and `finishPlayerDamage` calls (`cod11-combat.md` 4.5).
  INFERRED: the walk runs inside whatever did the killing: the `kill`
  packet, the shooter's cmd, the touch pass, the blast, the script frame.
- VERIFIED: `Scr_PlayerKilled` (0x5cb30) calls `Scr_ExecEntThread` at
  0x5cc04 and `Scr_FreeThread` at 0x5cc10, with a `ret` at 0x5cc1e.
  INFERRED, off that order and the walk's place after the call at 0x49bb6:
  the walk runs once `CodeCallback_PlayerKilled` has run to its first `wait`,
  before anything else the killing engine call does.
- VERIFIED: the `suicide` builtin calls `player_die` at 0x453e5 and has a
  `ret` at 0x453f0. INFERRED: no call or branch lies between them, so the
  `b` is queued before the script line after `suicide()` runs.
- VERIFIED: the `radiusDamage` builtin (0x5eef4) calls `G_RadiusDamage` at
  0x5ef55; `G_RadiusDamage` calls `G_Damage` at 0x4a87c and has a `jmp` to
  0x4a888 at 0x4a884; `G_Damage` calls `Scr_PlayerDamage` at 0x49e62, and
  `Scr_PlayerDamage` calls `Scr_ExecEntThread` at 0x5cb15. INFERRED: the
  `G_Damage` call sits inside the candidate loop and the `jmp` goes to its
  step, so each victim's damage callback, its `finishPlayerDamage` and that
  death's walk all run before the next candidate is damaged. With two
  victims the order is the first's callback commands, its `b`, then the
  second's.
- VERIFIED live, the scoreboard run of section 9: the follower got the `b`
  in the death frame's packet for a `kill` and for a head shot (the frame
  whose copy first read `health` 0), and none once it had let go of the
  victim.

## 5. The copy: `SpectatorClientEndFrame`

`ClientEndFrame` calls it for `sessionState` 2 (0x40f24, 0x40f30;
object-model doc). INFERRED from `G_RunFrame`'s loop: it runs in slot order,
so a spectator reads a lower slot's own-view bit from this frame and a
higher slot's from the last, and copies a higher slot before that slot's own
end frame.

- VERIFIED stores in `ClientEndFrame`'s playing and dead arm: the own-view
  bit (0x40fb0), `ps.stats` health (0x40fa4), `viewmodelIndex` (0x40fb4),
  `pm_type` (0x4103c, 0x4104e, 0x41079), `gravity` (0x410cc), `speed`
  (0x41100), and calls to `G_CheckForCursorHints` (0x41119),
  `P_DamageFeedback` (0x41128) and `G_GetNonPVSFriendlyInfo`, whose answer
  goes to `iCompassFriendInfo` (0x41201).
- VERIFIED live, the three-probe dm run of section 9: on the frame the
  followed client in slot 1 killed itself, the slot 0 follower read
  `pm_type` 0, the slot 2 follower 6, the target's own frame 6, and all
  three read `health` 0, `weapon` 0 and `eventSequence` 2; the next frame
  read 6 in both followers. INFERRED: the lower follower's copy carries the
  fields the target's end frame writes one frame late and the rest current;
  health is current because the kill wrote it before the end frame.

- VERIFIED stores: `svFlags` loses 0x2 and gains 0x1 (0x40778..0x40782),
  `ent+0x171` 0 (0x40788), `r.contents` 0 (0x4078f), `cl+0x220c` and
  `cl+0x2210` 0 (0x40799, 0x407a3), `ps+0xbc` 0 (0x407ad), `pm_flags` loses
  0x40000 (0x407b7).
- INFERRED (0x407c1..0x40859): when `spectatorclient >= 0` it becomes the
  target, and the copy is retried with `archivetime` lowered 50 at a time
  until it answers or the age reaches 0. A copy that still fails writes -1
  to both `spectatorclient` and the target.
- INFERRED (0x40865..0x40890): with the target negative, or its copy
  refused, `StopFollowing` runs (0x4091f) and nothing is copied.
- VERIFIED: the copy is 0x834 dwords over the spectator's own (0x408ba),
  covering the whole playerstate (`clientNum`, `commandTime` and the event
  ring included) and ending before `sess` at `cl+0x20d0`.
- VERIFIED: it calls `HudElem_UpdateClient` with array mask 2 (0x408bc),
  which rebuilds only the unarchived half at `ps+0x5a8`. INFERRED: the
  follower sees the followed client's archived HUD and its own unarchived
  one (`docs/protocol-1.1.md`, block 5).
- VERIFIED stores: `pm_flags` loses 0x40000 (0x408e0), gains 0x10000
  (0x408ea), and gains 0x20000 (0x40903) or loses it (0x408fd); `eFlags` is
  the copied word with 0x20000 taken from the spectator's own (0x408a5,
  0x408c1, 0x40910). INFERRED (0x408f4): `spectatorclient`'s sign picks
  between the two 0x20000 stores.
- VERIFIED live (section 9): a free follow's frame reads `pm_flags` 0x10000
  exactly, and its `clientNum`, `commandTime`, origin, `pm_type`, `eFlags`,
  `health` and `weapon` are the followed client's.

## 6. Which entities a follower is sent

- VERIFIED: the snapshot builder copies the client's playerstate
  (0x808f254), loads its `clientNum` (0x808f25f), takes the eye as `origin`
  plus `ps+0xd0` (0x808f288..0x808f2a0) and passes the `clientNum` to the
  entity pass (0x808f301).
- INFERRED: the pass skips the entity numbered `clientNum` (0x808e35b) and
  applies the `svFlags` 0x800 and 0x2000 single-client tests against that
  number (`cod11-events-and-fx.md` section 2). A follower is culled from the
  followed eye, is not sent the followed client's own entity, and gets that
  client's 175/176 copy, not the 173/174 one.
- VERIFIED live: the free spectator was sent players `[0, 1]`; following 1,
  `[0]`; following 0, `[]`.

## 7. Letting go: `StopFollowing`

- VERIFIED: it writes -1 to `spectatorclient` (0x46a3d) and to the target
  (0x46a47). INFERRED (0x46a51): the rest runs only when `pm_flags` 0x10000
  is set, i.e. when the last end frame copied.
- VERIFIED constants: the spot is a `trap_TraceCapsule` (0x46b54) with box
  -8 (0x73950) to 8 (0x73954), mask 0x810011 (0x46b39) and pass entity
  0x3ff, from the eye (`origin` plus `ps+0xd0`, with `G_AddLean`) to the eye
  plus forward times -40 (0x73948) plus up times 10 (0x7394c), both vectors
  off the copied view angles. The view gets 15 (0x73944) added to its pitch
  and goes to `SetClientViewAngle` (0x46be2).
- VERIFIED stores: `ps+0xb8` 0 (0x46b6e), `clientNum` the spectator's own
  (0x46b86), `eFlags` byte 1 masked with 0x3f (0x46b8c, clearing 0xc000),
  `ps+0x374` 0, `ps+0x378` 0x3ff, `ps+0x380` 0 (0x46b93..0x46ba7),
  `pm_flags` loses 0x10020 (0x46bb1), the trace end becomes `ps.origin`
  (0x46bc8..0x46bd7), and `ps+0x3dc..0x3e4` are zeroed (0x46be7..0x46bfb).
- VERIFIED: none of `StopFollowing`'s own stores is to `ps.velocity`
  (`ps+0x20..0x28`). INFERRED: its callees `G_SetOrigin` and
  `SetClientViewAngle` do not write it either, so the copy's velocity stays.
- VERIFIED: `SpectatorThink`'s free-flight arm stores `pm_type` 4 and
  `speed` 400 (0x3fb94, 0x3fb9b) ahead of its `Pmove` (0x3fc02).
- VERIFIED live, dm: the sight press put the spectator at the followed eye
  moved 27.9 back and 7 up (fraction 0.7, a wall behind), pitch 15, with
  `health` 100 and `weapon` 9 still the followed client's. INFERRED: the
  rest of the copied playerstate stays until something writes it.
- VERIFIED live, the three-probe dm run: `health` 100 and `weapon` 9 stayed
  on every free frame from the sight press to the next follow 6 s later; the
  press frame read `pm_type` 4, `viewHeightTarget` 0 and
  `viewHeightCurrent` 0.0 where the copy before it read 60 for both.
- VERIFIED live, sd: when the followed client went spectator 2 s after its
  death, the follower's next frame read its own `clientNum`, 40 back and 10
  up from the dead eye (view height 8), pitch 15, still `pm_type` 6; the
  frame after read `pm_type` 4.

## 8. A disconnect

- INFERRED (0x42b25..0x42b5b): `ClientDisconnect` walks every connected
  client whose `sessionState` is 2 and whose target is the leaver, calls
  `Cmd_FollowCycle_f(ent, 1)` (0x42b4b), and `StopFollowing` (0x42b5b) when
  that finds nothing.
- VERIFIED live, dm: when the followed probe's run ended, the follower's
  next change read `clientNum` 0, the remaining player.

## 9. The retail runs

All against `tools/run_server.sh mp_carentan`. The follower is
`--net-probe --probe-follow`: it presses attack at 8 s, attack at 14 s,
melee at 20 s, the sight for 2 s at 26 s and attack at 32 s after going
active, and prints every snapshot whose `clientNum`, `pm_type`, `pm_flags`
or `eFlags` moved.

- "dm": `--probe-team allies` (slot 0), `--probe-target --probe-team axis`
  (slot 1, kills itself every 45 s, ends at 120 s), the follower in slot 2.
- "sd" (`+set g_gametype sd`): the same three, the target started 4 s
  before the follower so its death at +10 s and its `spawnSpectator` 2 s
  later fall between the follower's first two presses.
- VERIFIED live, sd: at the round's restart the follower, which had been
  following slot 0, read its own `clientNum` and `pm_type` 4 at the
  spectator spawn. INFERRED: that is `ClientSpawn`'s drop of the target.
- "The three-probe dm run": `--probe-follow` in slot 0,
  `--probe-target --probe-team axis` in slot 1 and a second
  `--probe-follow` in slot 2, each started 1.5 s after the last. The probe
  also prints `serverTime`, the velocity, both eye heights, `health`,
  `weapon`, `damageEvent` and `eventSequence`, and every snapshot for six
  after any change. Both followers rode slot 1 through its death at 23750
  and its respawn at 26800, whose frame read `commandTime` 26800 in both.
- "The scoreboard run": `tools/run_probe.sh client-probes/probe_passthru
  mp_carentan +set probe_teleport 1`, then `--probe-follow` in slot 0,
  `--probe-target --probe-team axis` in slot 1 2 s later, and a `--save-hit
  --probe-sweep --probe-team allies` shooter in slot 2 50 s after that. The
  probe prints each pushed `b` as a `FOLLOW b` line beside its packet's
  snapshot and never sends `score`. VERIFIED live: two pushes,
  `b 2 0 0 1 -1 0 1 1 0 0 0 0 0` at snapshot `serverTime` 23850 after the
  target's `kill` (`K;` `MOD_SUICIDE`), and
  `b 3 0 0 2 1 0 0 0 1 -1 0 2 1 0 0 0 0 0` at 65400 after the shooter's
  second head hit (`K;` `MOD_HEAD_SHOT`). On both frames the follower's copy
  read `health` 0, `weapon` 0 and `pm_type` 0, then `pm_type` 6 on the
  next. The target's row reads `score` -1 and one death after the `kill`;
  after the head shot, two deaths and the shooter's row `score` 1. The copy
  gave way to the follower's own `clientNum` at 67450, and the four deaths
  after that pushed nothing. INFERRED: the let-go is the killcam making the
  target a spectator (section 4's followability).
- "The intermission run": `+set scr_dm_timelimit 1`, `--probe-follow` and a
  `--probe-team axis` player. The follower's intermission frames read
  `pm_type` 5, `pm_flags` 0x800, `commandTime` 59950 at `serverTime` 60050,
  60100 and 60150, and `viewangles` 0, 90.

## 10. Script

- VERIFIED, `maps/MP/gametypes/*.gsc` in `pak5.pk3`: every `spawnPlayer`,
  `spawnSpectator` and `spawnIntermission` writes `spectatorclient` -1 and
  `archivetime` 0; `killcam` writes the attacker's number and `delay + 7`
  (dm.gsc 766..767); sd.gsc's and re.gsc's bomb and goal cameras write
  `level.playercam`; every gametype's `main` calls `setarchive(true)`
  (dm.gsc 118).
- VERIFIED: `setarchive` is the builtin at 0x5f704, which passes its bool to
  syscall 0x45 (`trap_SetArchive`, 0x63c7c); `cod_lnxded` 0x808b4ac stores
  it and allocates the archive's buffers (0x440000, 0x2130000, 0x2580,
  0x2000000 and 0x3800 bytes) the first time it is set.

## 11. vcod

| Retail | vcod |
|---|---|
| cycle, stop spot, flag patch | `crates/server/src/follow.rs` |
| section 2 / section 7 | `ClientSim::spectator_think` / `stop_following` (`spectate.rs`) |
| think per cmd, no pmove while following | `Server::replay_moves` |
| section 5 | `follow_end_frame`, in the end-frame slot loop |
| section 8 | `pass_followers_on`, from `drop_client` |
| section 4's walk | `GameHost::player_die_walk` (`game/scoreboard.rs`) |
| follower's frame | `snapshots_begin`, from the followed client's wire playerstate, eye and number |

- `GameHost::die` is the one place a client dies. `finishPlayerDamage` and
  `suicide` start `CodeCallback_PlayerKilled` with `Cx::spawn_then`, so the
  VM hands back to the host once that thread first suspends; `kill_client`
  walks right after the thread it starts.
- The walk reads follow targets and connection states from a copy `Server`
  takes ahead of each script entry that can kill, and the session state
  live, as retail does. INFERRED, from vcod's tick: nothing inside one entry
  moves a follow (the cycle runs in a spectator's own cmd, the end frame
  after the script frame), so the copy reads what retail's live compares
  would.
- `spectatorclient` starts at -1 and is written back to -1 where retail
  writes it.
- The follower's frame is built at snapshot time from the followed client's
  state after its own end frame. For a follower numbered below its target,
  `follow::before_end_frame` puts back section 5's fields as the target's
  last frame had them (`ClientSim::end_frame_wire`, kept after each frame's
  snapshots and dropped by a spawn, whose own end frame is that frame's),
  and the event ring as it stood before the target's `P_DamageFeedback`
  added `EV_PAIN` (`ClientSim::ring_before_pain`), so that follower gets the
  pain a frame late. VERIFIED: `P_DamageFeedback` is called from
  `ClientEndFrame` (0x41128) and adds event `0xBB` (`cod11-combat.md` 6,
  step 9). INFERRED, from the slot order behind section 5's `pm_type`
  reading: a lower follower copies the ring before it. No retail run has put
  a non-fatal hit on a followed client.
- `StopFollowing` keeps the copy's velocity. The last copy
  (`ClientSim::follow_wire`) stays under the spectator's own frame until the
  next spawn; `spectate::SPECTATOR_OWNED` names the fields the spectator
  writes over it, with `pm_type` and `speed` joining once a cmd has flown.
- A stopped copy keeps `pm_flags` less 0x10020 (`StopFollowing`'s store,
  0x46bb1), and the spectator's `viewangles` are on the wire, so the press
  frame reads the stop's pitch 15. VERIFIED, `PM_UpdateViewAngles`
  (0x32d7c, `docs/protocol-1.1.md`, "View angles"): a spectator's
  `viewangles` are the cmd's angles plus `delta_angles`, which
  `SetClientViewAngle` has just set so the stop cmd's angles sum to the stop
  view. INFERRED: the frames after read 0 on retail because the probe
  subtracts the new delta, and vcod reproduces that the same way. Trap:
  `to_wire` must write `viewangles` in every mode and the copy's `pm_flags`
  must give way to the spectator's own, or the press frame reads pitch 0.
- The `kill` runs ahead of the cmds of its own packet, which still read
  `pm_type` 0 until the end frame (`cod11-combat.md` 9.2). Run after the
  frame's cmds, both followers' death-frame copies read `weapon` 9,
  `viewHeightTarget` 8 and `eventSequence` 1 where retail read 0, 60 and 2.

Live runs against vcod-server:

- VERIFIED, the three-probe dm recipe: the target's death frame read
  `events` 189, 155 and `eventSequence` 2. The slot 0 follower read it as
  `pm_type` 0, `viewHeightTarget` 60, `weapon` 0 and `eventSequence` 2, then
  6 and 8; the slot 2 one as 6 with the same rest. Both rode the respawn with
  `commandTime` equal to `serverTime`, each respawn frame read `legsAnim`
  634, and the sight press left `health` 100, `weapon` 9 and both eye
  heights 0 on the free frames and read pitch 15, then 0.
- VERIFIED, the scoreboard recipe with `vcod-server --gametype-script`
  running `probe_passthru`: the follower got `b 2 0 0 1 -1 0 1 1 0 0 0 0 0`
  at 23850 after the `kill` and `b 3 0 0 2 1 0 0 0 1 -1 0 2 1 0 0 0 0 0` at
  63550 after the second head hit, each on the frame whose copy first read
  `health` 0 at `pm_type` 0, as on retail.
- A death inside the script frame walks when its killed callback returns,
  like every other death: the `suicide` builtin (every stock team menu calls
  it on a live player changing teams), a scripted `radiusDamage`, and a
  script's own `finishPlayerDamage`. Trap: walking those at the frame's end
  lets a command another thread of that frame queued after the death go out
  ahead of the `b`, and the rows read the frame's end. VERIFIED, vcod:
  `a_script_death_sends_its_followers_the_scoreboard_when_the_callback_returns`
  (`crates/server/src/server.rs`) has a script `suicide()` a followed
  player whose callback queues a line and writes its score, then starts a
  thread that writes the score again and queues another; the follower reads
  the callback's line, the `b` with the callback's score, then the thread's
  line. INFERRED, from section 4's addresses: that is retail's order. No run
  has measured a script-frame death's `b` on retail.

Where it is not retail's:

- A stopped copy is kept whole except the owned fields. Whatever the
  spectator's `Pmove` writes into `pm_flags` after the stop is not measured;
  vcod writes nothing there.

### 11.1 The client: the 1.1 follow capture and the ring across a switch

`net/snapshots-follow-1.1.bin` (with its gamestate) is a retail 1.1d
spectator following a firefight: `tools/run_probe.sh
client-probes/probe_passthru mp_carentan +set probe_teleport 1`, an axis
`--probe-target`, 1 s later a `--probe-team spectator --probe-follow
--save-snapshots --capture-tag follow-1.1 --probe-secs 50` (client 1), 5 s
after it an allied `--save-hit` (client 2, `m1carbine_mp`); the two
players' own fixtures were discarded. The client's replay
(`crates/client/src/play/replay.rs`,
`a_1_1_follow_capture_plays_each_shot_once`) plays it through the frame's
event path.

- VERIFIED, the capture (`event_dump`): the spectator follows client 2 from
  21900 to 39900 and from 45900 to 58000, then client 0. On client 2's
  playerstate ring: 17 `EV_FIRE_WEAPON` (159) from 22500 to 32850,
  `EV_FIRE_WEAPON_LASTSHOT` (161) at 29000, then `EV_RELOAD_FROM_EMPTY`
  (152) at 29150. 21 `EV_BULLET_HIT_LARGE` temp entities: three `surfType`
  7 (flesh) on the target's torso at 22500, 24550 and 24700, 18 `surfType`
  10 (grass) past him. `EV_PAIN` (187) on entity 0 at 22500 and 24550, the
  obituary (`eType` 213) at 24700.
- VERIFIED, the capture: from 39900 to 45900 the frames read `clientNum` 1,
  the spectator's own, still carrying client 2's ring (`eventSequence` 19,
  events 159, 159, 159, 152). The switch onto client 0 at 58000 reads
  `eventSequence` 0 and four empty slots (he respawned, and a respawn clears
  the ring).
- VERIFIED, cgame `0x30028be0` (the ring half of
  `CG_TransitionPlayerState`): slot `i` from the new playerstate's
  `eventSequence - 4` up to it plays when `i >= ` the old one's
  `eventSequence`, or when `i > old - 4` and the slot's event differs from
  the old playerstate's same slot. VERIFIED, the same compare in 1.5's
  cgame at `0x30029f50` and UO's at `0x30034ec0` (UO's ring sits 4 bytes
  later, `eventSequence` at 0x88). INFERRED, from that compare and
  `cod11-hud-protocol.md`'s note that the follow arm (`0x3002fc6c`) tests no
  `clientNum` first: at the 58000 switch (19 to 0) retail plays nothing.
- VERIFIED, `crates/common/src/net/events.rs` (`EventTracker`, the
  playerstate arm, `ps_slot_plays`): vcod takes that compare, so the replay
  plays no `EV_NONE` at the switch and a switch onto a lower sequence
  replays none of the new player's old events. INFERRED, from the 8-bit wire
  field (`fields_v1.rs`) and the raw compare: the frame that wraps the
  playerstate ring from 255 to 0 plays nothing, on retail and on vcod
  (`the_playerstate_ring_plays_nothing_across_the_wire_wrap`). No run has
  measured that. The own predicted ring is `PredictedEvents`
  (`crates/client/src/play/events.rs`), which compares across the wrap.

## 12. The killcam

A killcam is a forced follow whose copy comes from the engine's frame
archive, `archivetime` back, with every time in it moved forward by the
record's age, and whose snapshot takes its entities from the same archived
frame. The stock scripts drive it. The engine supplies the archive, the
lookup, the copy's retry, the shift and the snapshot's entity source.

### 12.1 What the stock scripts ask for

- VERIFIED, `maps/MP/gametypes/dm.gsc` and `tdm.gsc` in `pak5.pk3`: a kill
  by another player threads `killcam(attackerNum, delay)` after
  `Callback_PlayerKilled`'s `wait delay` (2), unless `scr_forcerespawn` is
  above 0 (dm.gsc 541). `sd.gsc` and `re.gsc` do it only while the victim's
  team still has a live player and the round has not ended (sd.gsc 843,
  849). No stock script reads a `scr_killcam` cvar; `scr_forcerespawn` is
  the only switch.
- VERIFIED, dm.gsc 754..851: `killcam` sets `sessionstate` `"spectator"`,
  `spectatorclient` the attacker and `archivetime` `delay + 7`, waits 0.05,
  gives up (dm: back to `"dead"` and `respawn()`) if `archivetime <= delay`,
  draws five unarchived elements (two bars, title, skip text, a tenths
  timer of `archivetime - delay`), then waits for `waitKillcamTime`'s
  `wait (archivetime - 0.05)` or a use press after a release. The end writes
  `spectatorclient` -1 and `archivetime` 0; dm and tdm also write
  `sessionstate` `"dead"` and thread `respawn()`, sd and re leave the client
  a spectator.
- VERIFIED, sd.gsc 1147..1158: `roundcam` with a bomb camera spawns the
  spectator at the camera and writes `archivetime` without a
  `spectatorclient`: the entity half of the replay alone (12.6).

### 12.2 `archivetime`

- VERIFIED: its setter (`game.mp.i386.so` 0x41db8) multiplies
  `Scr_GetFloat`'s value by 1000.0 (0x7306c) and stores it at `cl+0x20dc`
  with `fistp` under a control word with 0xc00 set, which truncates toward
  zero. Its getter (0x41dfc) loads that integer and multiplies by 0.001
  (0x73070). The field is milliseconds; script sees seconds.
- VERIFIED: `vmMain` (0x50dd4) cases 0x12 and 0x13 read and write
  `level + 0x20dc + n * 0x22c4`, the engine's access to the same
  milliseconds.

### 12.3 The archive

- VERIFIED: `setarchive` (0x5f704) calls `Scr_GetBool` (relocation at
  0x5f712) and `trap_SetArchive` (0x5f718). `cod_lnxded` 0x808b4ac stores
  the flag at 0x83b67c8 (section 10 has the allocations).
- VERIFIED: 0x808b2c8 stores 0 into the flag and into the counters at
  0x83b67cc, 0x83b67d8, 0x83b67dc, 0x83b67e0 and 0x83b67e4; `SV_SpawnServer`
  (0x808a220, the `Server: %s` banner) stores the same zeros inline, and the
  restart (0x8083de4, the `g_gametype variable change -- restarting.`
  string) calls 0x808b2c8. INFERRED: every map load and every restart turns
  the archive off and empties it; the new level's `main` turns it back on
  with `setarchive(true)`.
- INFERRED from 0x808fb84, which `SV_Frame` calls after each `G_RunFrame`
  and after the snapshots: while the flag is on, every server frame is kept.
  A full record (frame number, `svs.time`, and the ranges of its client and
  entity records) is written at most once per `sv_fps` frames; the others go
  as a delta message into a 0x2000000-byte stream indexed by a 0x4b0-slot
  table, which 0x808e68c decodes back into records on demand. A frame holds
  every connected client's `clientState` (game export 0x11) and, where
  export 8 answers (section 4, the own-view bit), its playerstate, and every
  linked entity that is not `SVF_NOCLIENT` and has clusters or a broadcast
  bit, with its `svFlags`, `singleClient` and linked box.

### 12.4 The lookup and the trim (0x808eeb8)

- INFERRED: with the flag off, or an age below 1, it answers nothing and
  leaves the age alone. Otherwise the frame is the frame count less
  `sv_fps * age / 1000`. A frame older than the count less 0x4b0 is clamped
  there and the age rewritten as `0x4b0 * 1000 / sv_fps`; a frame before the
  first is clamped to 0 and the age rewritten as `count * 1000 / sv_fps`.
  The answer is the first frame from there on that decodes; if none does,
  the age becomes 0. The age is passed by pointer, so every rewrite lands in
  `archivetime` (12.2), which the stock `wait 0.05; if(self.archivetime <=
  delay)` reads a frame later.

### 12.5 The copy and its retry

- INFERRED (`SpectatorClientEndFrame` 0x407c1..0x40859, with section 5): a
  forced follow clamps a negative age to 0 and asks
  `trap_GetArchivedPlayerState` (0x808ef7c). Each failure lowers the age 50
  and asks again, down to 0, where no frame is found and the live client is
  copied if export 8 answers for it. A follow that still has nothing writes
  -1 to `spectatorclient` and the target.
- INFERRED (0x808ef7c): a frame found with no record for the client, or a
  record whose playerstate export 8 refused, fails with no live fallback. So
  the retry settles on the first archived frame where the followed client
  had a view of its own.
- VERIFIED live, the two tdm runs of 12.9: a killcam's age was 9000
  whenever the attacker had been playing that long, and 6600 and 7500 when
  it had joined 6.6 and 7.5 s before the killcam's first frame. The
  committed capture
  `crates/server/tests/fixtures/playerstate/mp_carentan-tdm-hit-target.txt`
  reads 8650: its replayed origins lie on the shooter capture's trail
  8650 ms back, and the replay's first frame shows the shooter standing at
  its spawn before its first move.

### 12.6 The shift and the snapshot

- VERIFIED stores in 0x808ef7c after the 0x834-dword copy: the age,
  `svs.time` (0x83b67a4) less the record's time (`+4`), is added to each
  non-zero dword at `ps+0x0`, `+0x10`, `+0x38`, `+0x64`, `+0xd4` and
  `+0x3e0` (playerstate netfields `commandTime`, `pm_time`,
  `iFoliageSoundTime`, `jumpTime`, `viewHeightLerpTime`,
  `shellshockTime`); to each non-zero `+0x24`, `+0x44`, `+0x54` and `+0x5c`
  of the 31 0x70-byte elements from `ps+0x1338`, the archived HUD half
  (`fadeStartTime`, `scaleStartTime`, `moveStartTime`, `time`;
  `cod11-gsc-object-model.md`'s HUD tweens); and to `ps+0x20cc`,
  `deltaTime`, unconditionally.
- VERIFIED: `ClientEndFrame` zeroes `ps+0x20cc` before anything else
  (0x40eb4), so a live frame's `deltaTime` is 0 and a replay's is the age.
- INFERRED (0x808f130, `SV_BuildClientSnapshot`): every snapshot reads the
  client's `archivetime` through export 0x12, runs the lookup and writes the
  age back through 0x13, whether or not the client follows anyone. With a
  frame, the entities are that frame's records culled from the
  playerstate's eye against their linked boxes (0x808e4a8), with the
  single-client tests against `ps.clientNum`, the `clientNum` entity left
  out only while `pm_flags` 0x10000 is set, and the roster is that frame's
  client records. Each entity's non-zero `+0x10`, `+0x34`, `+0x54` and
  `+0x58` (`pos.trTime`, `apos.trTime`, `time`, `time2`) take the age.
- VERIFIED live: `deltaTime` read the age on every replayed frame and 0
  elsewhere; `serverTime - commandTime` read 0..34 through the replay as it
  does live; the archived round clock's `time` read 1809000 where the live
  one read 1800000; the killcam's own five unarchived elements were
  unshifted and first sent on the replay's second frame.
- VERIFIED live: the victim's own entity was sent alive in the replay and
  its corpse was not: the entity went and body 64 came on the same replayed
  frame, one age after the live death. A respawn and second death inside a
  replay window showed one age late the same way. The attacker was never in
  its own replay. The committed hit-target capture carries the kill's
  obituary twice, 8650 ms apart: the archived temp entity sent again.

### 12.7 The end

- INFERRED (`ClientEndFrame`, 0x40f45 and 0x40f82): the playing and dead
  arm runs only while `ps.clientNum` is the client's own number. A
  playerstate still holding a copy takes the other branch: `ClientSpawn` at
  the copy's `origin` with angles `(0, viewangles[1], 0)`.
- VERIFIED: `ClientSpawn` calls `ClientEndFrame` (relocation at 0x42a75) and
  `ClientThink_real` (0x42a82). INFERRED: the spawn's own end frame takes the
  arm its `sessionstate` names, so a `"dead"` one is a dead player at once.
- VERIFIED live, tdm, every natural end: the frame one age after the first
  replayed frame reads the victim's own `clientNum`, `pm_type` 6, `pm_flags`
  0x40800, `eFlags` 0x18 where the replay read 0x10, weapon 0, health 0,
  `serverTime - commandTime` 0, the replay's last origin and yaw with pitch
  0, and both HUD arrays empty; the next frame has the clock and the respawn
  text back. The committed hit-target capture's end frame reads health 0,
  `pm_type` 6, `eventSequence` 0, no clip and no reserve, the teleport bit
  flipped, at the replay's last origin, one age after its first frame.
  INFERRED: `ClientSpawn` flips the bit of the copied `eFlags`, and its
  memset clears the HUD after the frame's HUD update.
- VERIFIED live, tdm: a use press 3500 ms into the replay went from the last
  replayed frame straight to the victim alive at a spawn, with no dead frame
  between, in all three runs. INFERRED: `waitSkipKillcamButton`'s notify
  resumed `killcam`, and `respawn`'s `waitRespawnButton`, past its opening
  `wait 0`, read the same press, all inside one frame; `probe_wait0_yield`
  measures that a `wait 0` resumes inside its frame.
- VERIFIED, `probe_notify_frame` (`cod11-gsc-language.md`, the thread pick):
  a notify's waiters resume on the notifier's frame, whichever was started
  first, which is why `waitKillcamTime`'s notify ends `killcam` on the frame
  the age runs out and the natural length is the age exactly.
- VERIFIED live, sd: at the end the victim read the attacker with `pm_flags`
  0x10000, `deltaTime` 0, no teleport flip and no empty frame, and kept
  following it live for the remaining 100 s; its use presses did nothing.
  INFERRED: section 5's live copy through the target the forced follow left
  behind.

### 12.8 What the runs did not separate

- Whether the dead spawn's origin and yaw are the last replayed copy's or
  the attacker's live ones: the shooter stood still at every end.
- Whether the replay's roster is the archived one: the roster never changed
  inside a replay window.
- The objectives on the end frame: the probe did not print them.

### 12.9 The retail runs

`tools/run_server.sh mp_carentan +set g_gametype tdm +set
scr_friendlyfire 1` (and `sd`). The victim was `--net-probe --probe-killcam
--probe-team allies`: it stands still, never sends `kill`, presses use 20 s
after each death (or, with `--probe-killcam-skip-ms 3500`, that long into
the replay) and prints every snapshot from its death to 3 s after it is
alive again. The shooter was `--save-hit --probe-sweep --probe-team allies`,
writing no fixture. Six natural tdm killcams, three skipped ones and one sd
killcam. The logs are not committed. The committed evidence is the earlier
tdm hit pair, which caught a killcam of its own;
`crates/server/tests/killcam_ab.rs` holds its reading and vcod's schedule
to it.

### 12.10 vcod

- `crates/server/src/archive.rs` is the archive. While script has
  `setarchive(true)` on, `Server::archive_frame` keeps, after each frame's
  snapshots, the entities and roster they were built from and every client's
  own-view playerstate, eye, view and feet, in a ring of 0x4b0 frames that
  every level load clears (`engine::archive::FrameRing`, CoD2's ring too).
- `Archive::lookup` is 12.4 and `Archive::player_state` is 0x808ef7c.
- `follow_end_frame` runs 12.5's retry and writes the trimmed age back into
  `archivetime`, which the host keeps in milliseconds (12.2).
- `snapshots_begin` builds a replay's playerstate from the archived frame
  (`archive::replayed_ps`, 12.6), and `send_client_snapshot` sends any
  client whose `archivetime` names a frame that frame's entities and roster,
  shifted.
- A playing or dead client whose last frame was a copy is spawned at the
  copy (`ClientSim::spawn_from_copy`, 12.7): its teleport bit the copy's
  flipped, weapons and ammo gone, HUD arrays empty on that frame. The spawn's
  own think (section 13) sets `PMF_RESPAWNED`, drops the dead eye to 42 and
  puts `commandTime` at the frame's clock, which `killcam_ab.rs` holds to
  retail's 0x40800 and lead 0.
- `crates/server/tests/killcam.rs` runs the stock dm killcam end to end, the
  skip included.
- The scheduler's thread pick (`vcod-gsc`'s `step_runnable`,
  `cod11-gsc-language.md`'s thread pick paragraph, measured by three retail
  probes) is required: without it 12.7's timings lag a frame.

VERIFIED live against vcod, `vcod-server mp_carentan --gametype-script
.../client-probes/probe_passthru.gsc --set probe_teleport=1 --set
scr_friendlyfire=1` with the 12.9 probes (victim on axis): five killcams,
each starting 2000 ms after the death. The first, the shooter having spawned
5.2 s before it, read `deltaTime` 5200 and ended 5200 ms after its first
frame; the rest 9000. The end frame read the victim's own number, `pm_type`
6, the replay's last origin, pitch 0, `eFlags` 0x10 where the replay read
0x18, weapon 0 and both HUD arrays empty. With `--probe-killcam-skip-ms
3500` it went from the last replayed frame to the victim alive at a spawn,
no dead frame between, three times out of three.

Where it is not retail's:

- The archive is whole frames in memory (an entity unchanged since the last
  frame shared, not copied), not a delta stream. Every frame of the last
  0x4b0 is kept; retail loses one when its stream wraps. INFERRED from the
  sizes: a busy level can wrap 32 MB inside a minute of frames.
- The replay is culled by vcod's own box-cluster test from the archived view
  eye, where retail takes box leaves from the archived abs box.

## 13. Spawns

Every `self spawn(origin, angles)` and `ClientEndFrame`'s spawn arm (12.7)
end in `ClientSpawn`; its last few instructions are what a spawned client's
first frame reads.

- VERIFIED: `ClientSpawn` sets `pm_flags` 0x800 (0x429d6), puts
  `commandTime` at `level.time - 100` (0x42a48, 0x42a6f), and calls
  `ClientEndFrame` (0x42a75) and then `ClientThink_real` (0x42a82) with a
  local cmd it zeroes (0x42a2f), stamped `level.time` (0x42a42), angles the
  negated `delta_angles` (0x42a4e..0x42a69). INFERRED: the spawned client's
  own end frame runs before the frame's end-frame loop, so a player has its
  own view at once (section 5), and the think runs 100 ms of pmove up to the
  frame's clock on a cmd with no buttons.
- VERIFIED: `PmoveSingle` clears 0x800 (0x34000) past a `pm_type` compare
  against 5 (0x33f9a, 0x33fed) and an attack-bit test (0x33ff8). INFERRED: a
  pmove at `pm_type` 5 or below with attack up clears it, so the spawn's own
  think clears it for a player and a spectator, and a dead spawn and the
  intermission camera, whose `ClientThink_real` arm runs no pmove, keep it.
  The jump gate on it (`cod11-mantle.md`, "Jumps") is unreachable.
- VERIFIED live: the killcam's dead spawn read `pm_flags` 0x40800 and
  `serverTime - commandTime` 0 (12.7); the three-probe dm run's respawn frame
  read `commandTime` equal to `serverTime` in both followers' copies; the
  intermission run read `pm_flags` 0x800 and `commandTime` 100 below the
  spawn frame's `serverTime`, unchanged on the frames after.
- VERIFIED (`cod11-combat.md` 1.12): `PM_Weapon` returns at once on
  `pm_flags` 0x800 (0x390ee) and stores 0 into `ps.weapon` when `pm_type`
  is above 5 (0x390f8, 0x390fe); `PmoveSingle`'s dispatch sends `pm_type` 6
  to 0x34274 (jump table 0x70ce8), whose unmounted path reaches the
  `PM_Weapon` call at 0x34331. INFERRED: every dead player's move empties
  `ps.weapon`, which is why a dead client and its followers read `weapon` 0
  (the three-probe dm run, and the sd round-restart target's death frames).

vcod:

- `ClientSim::respawn` sets the flag. `spawn_think` is the spawn's own end
  frame and think: it sets the own view for a player, clears the flag unless
  the client is dead or at intermission, and runs a dead spawn's 100 ms of
  `dead_move`, the eye dropping 18 units to 42.
- A script's `spawn` of a player or spectator runs its think's 100 ms too
  (`ClientSim::spawn_move`, on the zeroed cmd with the negated
  `delta_angles`), and the tick picks a player's anims after it, which puts
  the standing idle on the spawn frame (`cod11-combat.md` 9.2).
- Every caller but the intermission's puts `commandTime` at the frame's
  clock. The intermission camera reads the spawn's frame less 100 until the
  next spawn, whatever cmds it sends (`ClientSim::become_intermission`), as
  the intermission run read.
- A dead sim's step writes `ps.weapon` 0 unless the flag is set, and the
  switch reaches the script host as the weapon machine's own do.

Where it is not retail's:

- A playing client spawned out of a follow's copy (12.7,
  `spawn_from_copy`) gets only the flag and the clock: its 100 ms of
  null-cmd pmove is not run. A dead one runs `dead_move`'s 100 ms. INFERRED
  from the negated `delta_angles` on that cmd: a spawned player's or
  spectator's frame reads `viewangles` 0 whatever the spawn yaw, on retail
  and on vcod, whose script spawns run the cmd. The three-probe dm run's
  spawn had yaw 0 and cannot tell.
