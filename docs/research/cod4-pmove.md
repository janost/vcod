# CoD4 1.7 player movement: `vcod_cod4::pmove` against retail

What 1.7's `Pmove` does, how `vcod_cod4::pmove` models it, and how close
the model replays retail's own captures (`cod4_moves_ab`). The module is
seeded from CoD2's (`vcod_cod2::pmove`, docs/research/cod2-movement.md):
1.7's `bg_pmove` and `bg_slidemove` descend from CoD2 1.3's, and the
differences below are the ones the binary and the captures show. The
collision it traces through is `vcod_cod4::collision::ClipMap`
(cod4-collision.md).

Sources, each claim naming its own:

- `cod4_lnxded` (md5 `bf8448740b3f4cb74c2254f01af14cdd`), stripped. A fact
  read out of its instructions or rodata is VERIFIED and carries the
  address; which function an address is, read off its constants and
  stores, is INFERRED unless a string names it.
- Two live captures on that server, decoded by `vcod_cod4::net`:
  `play-1.7-walk.txt` (a walker on allies, cod4-netfields.md 8) and
  `play-1.7-fly.txt` (a free spectator, its recipe in its header).
  VERIFIED live.

A claim labelled INFERRED that names no 1.7 address is not yet confirmed
on 1.7.

## 1. The float model

`cod4_lnxded`'s pmove is an optimized x87 build that keeps a local on the
FPU stack: what C stores into a struct field or an array is rounded to a
float, a scalar local is not. Two functions read off the binary:

- At 0x80509e6 (`Jump_ReduceFriction` by its 1800 compare and its
  constants, INFERRED) the curve is `fld 1.5` (0x820962c), `fimul
  pm_time`, `fmul` 1/1700 (0x820a320, float `0x3a1a33cd`), `fadd 1.0`,
  `ret`: the value goes back to the caller in `st(0)`, unrounded. VERIFIED.
- At 0x8050aac..0x8050b38 (the takeoff `Jump_Check` inlines, INFERRED off
  its `jumpTime` and `jumpOriginZ` stores at ps+0x80 and +0x84)
  `jump_height + jump_height` times `gravity`, divided by the curve, goes
  to `fsqrt` and is stored once, into `velocity[2]` (ps+0x30). VERIFIED.
- At 0x805eb63..0x805ebe5 (`PM_StepSlideMove`'s revert test, the function
  INFERRED off its prone step 10.0 at 0x805e9db and the client-number
  compare `ax <= 0x3f`) the step's progress along the entry velocity stays
  on the stack while the plain slide's goes through four stores (the flat
  deltas, then each product), and `fst`, which keeps `st(0)`, sums them
  before `fadd 0.001` and the compare. VERIFIED.

CoD2 1.3's `cod2_lnxded` stores every float variable (cod2-movement.md 2),
which the seed followed. The A/B moved these to the kept model, each change
taking rows from wrong to exact (section 13): friction's speed, drop and
scale; the accelerate's add and step and the inertia clamp;
`Vec3Normalize`'s and `Vec3NormalizeTo`'s sum, length and reciprocal;
`PM_ClipVelocity`'s dot; `PM_CmdScale` and `PM_MoveScale` returned
unrounded (`PM_CmdScale_Walk`'s stores are section 6);
`PM_ProjectVelocity`'s new z and
length scale; the view clamp's turns; the eye curve's fraction. VERIFIED
(the A/B rows). That the rest of 1.7's pmove keeps its locals the same way
is INFERRED from those, and the functions not yet converted keep the seed's
stored model (`pmove/fpu.rs`).

## 2. Types

- `pm_type`: 1.7 puts `PM_LASTSTAND` at 6, so the dead are 7 and 8,
  one past CoD2. INFERRED.
- `pm_flags` is renumbered from CoD2's: prone 0x1, ducked 0x2, mantle 0x4,
  ladder 0x8, sighted 0x10, backwards 0x20, the sighted walk 0x40, the hard
  landing 0x80, knockback 0x100, the held prone keys 0x200, respawned
  0x400, frozen 0x800, a refused prone 0x1000, the ladder just left 0x2000,
  jumping 0x4000, sprinting 0x8000, shellshock 0x10000, melee charge
  0x20000, no sprint 0x40000, no jump 0x80000. Which bit each is, is
  INFERRED off the instruction that sets or tests it in `cod4_lnxded` (the
  binary is stripped, so the names are vcod's): `PmoveSingle` (0x805bd14)
  tests 0x20000 at 0x805bd2c, 0x800 at 0x805bd39, 0x1 at 0x805bd9d and 0x400
  at 0x805bdf1, clears 0x1000 at 0x805bd89 and 0x200 at 0x805bda7, sets 0x40
  at 0x805c08a (a sighted, unprone player outside weapon states 7..11),
  0x20 at 0x805c417 and 0x2000 at 0x805c473, and clears 0x4000, 0x100 and
  0x80 together at 0x805c460; `PM_UpdateSprint` sets 0x8000 at 0x805723f
  and refuses on 0x40000 (0x8057164), 0x8 (0x8057176) and 0x1001c
  (0x80571b0); 0x80 is set at 0x805b952, 0x4000 at 0x80507b5, 0x4 at
  0x8051c88, 0x2 at 0x805914e, 0x1000 at 0x80567e8 and 0x20000 at
  0x805621b, 0x10 is cleared at 0x805fd18 and 0x80000 tested at 0x8050a53.
  The walk capture's values are 0, 0x2 (crouched), 0x1000 (prone held and
  refused), 0x4000 (in a jump) and 0x4002, each where its key was.
  VERIFIED live.
- The buttons pmove reads, INFERRED: sprint 0x2,
  lean 0x40 and 0x80, prone 0x100, crouch 0x200, jump 0x400, the held sight
  key 0x800 (what the sighted walk reads; the toggle is 0x80000), the
  stance-quiet bit 0x1000, talk 0x100000 (pmove's masks).
- The playerstate's `delta_angles` are floats (CoD2 keeps shorts), and a
  cmd's angles are read unsigned: a yaw short of 53703 adds 294.99939, not
  -65.00061, before the wrap. VERIFIED (the fly capture's velocity, section
  13, matches only so).
- `vcod_cod4::pmove::PlayerState` holds the playerstate fields pmove reads
  and writes; `wire.rs` maps it onto the 141 netfield words both ways.

## 3. `PmoveSingle`

The order is CoD2's with 1.7's additions: the melee charge forces forward
127; frozen, respawned and the frozen-by-weapon state clear the moves; talk
masks the buttons; `PMF_PRONE_REFUSED` clears each frame; `EF_FIRING` (0x40)
follows the attack key in a ready or firing weapon state with ammo; the
sprint update and the sighted walk flag run before the stance on every arm;
the turret, mantle and dead arms as CoD2's; the spectator flies; the default
arm ends with the tail that snaps the velocity with `rint`. INFERRED. The
masks are VERIFIED in `cod4_lnxded`: frozen keeps 0x1300 (0x805bd42),
respawned 0x1301 (0x805c359), talk 0x101b02 (0x805bd79); the step's
frametime is the msec times 0.001 (0x8209640), stored. The foliage sounds
(inlined at 0x805d05e) and the tail (0x805d091) follow `PM_Weapon` there,
the tail keeping the frame's displacement and both squared speeds on the
x87 stack against 0.25 (0x820966c) and storing each `oldVelocity` step.
VERIFIED. The weapon half (the aim spread, the sight flag and lerp,
`PM_Weapon`) runs when the caller hands the level's weapon defs in
(`player_step_armed`; cod4-combat.md 2); without them only the breath
does. The mantle is section 17's. The anim script's calls run when the
caller hands the anim set in (`player_step_animated`;
cod4-player-anims.md 7). The melee charge is section 19's.

## 4. The ground trace

As CoD2's (cod2-movement.md 7, 8) with one change: a contact is stood on
when the trace says it is walkable (`trace_t.walkable`, set by the
triangles' edge bits or `normal.z >= 0.7`, cod4-collision.md 3), not by a
normal's z. `PM_CorrectAllSolid` drops a nudged body 1.25, not 1.
INFERRED. The landing's events carry the surface in the parm for
a soft landing (`EV_FOOTSTEP_WALK`, `_RUN`) and add it to the event for a
hard or a painful one (`EV_LANDING_DEFAULT` 77 + surface, `EV_LANDING_PAIN_DEFAULT`
106 + surface). INFERRED; the walk capture's landing raised 99,
`EV_LANDING_ASPHALT`, 77 + 22. VERIFIED live.

In `cod4_lnxded` `PM_CrashLand` is inlined in `PM_GroundTrace` at
0x805b7bd (INFERRED off its constants). Read off the instructions,
VERIFIED:

- The fall is solved on the x87 stack: `vz^2 + (-4 * 0.5 * -g) * dz`
  (-4.0 at 0x820df0c), its `fsqrt`, the impact time stored as a float
  (0x805b83d), then `(vz - g t)^2 / 2g` kept. Only the time goes through
  memory.
- The damage percentage is `(h - min) / (max - min) * 100` (100 at
  0x820df10) stored as a float and truncated (0x805b8c1), 100 at or over
  the maximum, 0 at or under the minimum, under surface flag 1, past
  `pm_type` 6, or when the minimum is not under the maximum (which
  prints a warning, 0x805bb57).
- Over 12 (0x820dee0) the view dip is `(h - 12) / 26 * 4 + 4` (26 at
  0x820df14), stored and truncated, at most 24.
- A hurt landing under 100, not on a slick floor (flag 2), stuns `35 * damage + 500`
  ms, at most 2000, and keeps 0.5 of the speed to 500 ms, 0.2
  (0x8209690) from 1500, else `(t - 500) / 1000 * -0.3 + 0.5`
  (0x820df18, 0x8209664, 0x82096a8), kept (0x805bca4). A fatal or slick
  one damps to 0.67 (0x820df1c).
- Unhurt: nothing to 4 (0x8209660), event 74 under 8 (0x820aac4), 73
  under 12, else the 0.67 damp and 77 + surface with the dip.

## 5. The slide and the step

`PM_SlideMove` is CoD2's. In `cod4_lnxded` (0x805e030) its time left is a
stored float: the frametime (0x805e0f7), then after each bump `time_left -
fraction * time_left` with one store (0x805e6d5..0x805e6e9); each bump's
end is `time_left * velocity + origin` with one store per axis
(0x805e166..0x805e1a5), and a timer (`pm_time`) restores the whole entry
velocity (0x805e72c). VERIFIED (the instructions). `PM_StepSlideMove` differs: a step is tried when
the slide bumped or when the ground plane's z is under 0.9; the step's down
trace keeps the plain slide when it lands on a contact that is
neither walkable nor steeper than 0.3; the velocity on landing goes through
`PM_ProjectVelocity` (the flat velocity laid on the plane with its length
kept) where CoD2 clips it; the revert compares the moves along the entry
velocity, not the current one; and the step's tail damps the velocity and
re-measures `xyspeed` but raises no `EV_STEP_VIEW`: the eye's step rides
`pmove_t.viewChange`, which vcod reports as `Pmove::view_steps` for the
client's camera (cod4-client-view.md 5). INFERRED; the
revert's arithmetic is section 1's. The walk on foot uses
`PM_ProjectVelocity` for both the wish direction and the velocity, where
CoD2 clips the forward and right vectors. INFERRED.

`PM_ProjectVelocity` is 0x8056242 (INFERRED off its callers). It passes
the velocity through when `|normal.z|`, stored, is under 0.001
(0x8209640) or the flat length is 0; the plane's z, `-(n.x vx + n.y vy) /
n.z`, and the length ratio `sqrt((vz^2 + flat) / (z'^2 + flat))` stay on
the stack, and the three products are stored once, the new z unrounded
into its own (0x8056316). It scales when the ratio is under 1, the new
z negative or vz positive; otherwise it writes nothing, which its callers
make the input by passing the velocity as both. VERIFIED.

`PM_StepSlideMove` (0x805e930), VERIFIED off its instructions:

- The step is 10 prone (0x805e9db), else 18; a jumping body that bumped
  takes `Jump_GetStepHeight` (0x805072a): none at or over the jump's top
  (`jumpOriginZ + jump_height`, kept), else `jump_stepSize` or the room to
  the top when less. Under a unit it ends there.
- The lift is tried when the slide bumped or the ground's normal is under
  0.9 (0x820df28): a trace up `step + 1`, the lift `(step + 1) * fraction
  - 1` kept on the stack, none under a unit.
- The down pass's end is `z - lift - 9` (9 at 0x820dee4, only on the
  ground) in one store; a hit on a client (`ax <= 0x3f`) keeps the
  slide; a miss takes the kept lift off z; a hit on an unwalkable face
  under 0.3 (0x8209684) keeps the slide.
- The tail: the eye's rise over the slide, kept, counts when its
  absolute value, stored, is over 0.5, and rounds as `floor(rise + 0.5)`
  under a round-down control word (0x805ec46..0x805ec89). The damp is
  `(1 - |climb| / step) * 0.8 + 0.2` (0x8209694, 0x820e254, the float
  `0x3e4ccccc`) kept, each velocity product stored; `xyspeed` is the
  `fsqrt` of the stored components' squares (0x805ed1b). A rise past 3
  pushes the bob `old + 7 + 1.25 n` (0x820e25c, 0x820e258), `n` half the
  rise up to 4, stored and truncated.
- `pmove_t.viewChange` (pm+0x108) takes the rise only when
  `viewChangeTime` (pm+0x104) is behind `commandTime`, which it then
  takes (0x805f1b9).

## 6. The moves

- `PM_CmdScale_Walk` takes the back and strafe dvars, the sighted walk's
  0.4 (also while prone with the weapon raised), the sprint's
  `player_sprintSpeedScale`, the stance (crouch 0.65, prone 0.15, blended
  over the eye's lerp between them), then the weapon's
  `moveSpeedScale`, or its `adsMoveSpeedScale` while sighted, then the
  playerstate's `moveSpeedScaleMultiplier`, which the class script sets
  (0.95 in the walk capture, VERIFIED live). INFERRED.
- The weapon's scales sit in the zone's `WeaponDef` at 1048
  (`moveSpeedScale`), 1052 (`adsMoveSpeedScale`) and 1056
  (`sprintDurationScale`), six floats ahead of `adsOverlayShader` at 1072
  (`assets/weapon.rs`). The layout is INFERRED; the walker's `m16_gl_mp` (weapon 27 of `CS_WEAPONS` 2258)
  replays exactly with them. VERIFIED.
- In `cod4_lnxded` `PM_WalkMove` is 0x805987e with `PM_CmdScale_Walk`
  inlined. `PM_CmdScaleForStance` (0x8056c72) mixes the crouch and prone
  scales (0.65 at 0x820a310, 0.15 at 0x820deac) over 400 ms (0x820dea8)
  while the eye lerps to prone, or up from prone to the crouch, the
  fraction kept on the stack. VERIFIED. `PM_DamageScale_Walk` (0x8056046,
  inlined at 0x8059a8f) is `1 + damageTimer * (-player_dmgtimer_minScale
  / player_dmgtimer_maxTime)`, stored once. VERIFIED. The sprint's side key
  is `player_sprintStrafeSpeedScale` times the key stored as a float, then
  truncated (0x80598c0). VERIFIED.
- `PM_CmdScale_Walk`'s value goes through memory in `cod4_lnxded`. The
  scaled back key and side key are stored (0x8059b08, 0x80599b6; the
  forward key's store at 0x805999b holds an integer); the keys' share,
  with the sighted 0.4 and the sprint, is stored across
  `PM_CmdScaleForStance`'s call (0x8059dad; noclip's and UFO's factors
  multiply on the stack instead); the product is stored across the weapon
  def's lookup (0x8059a2c) and, with a shellshock flag, across the shock
  record's (0x8059dce); and the whole scale once more after
  `moveSpeedScaleMultiplier` (0x8059a89). The wish speed is that float
  times the damage scale and the wish length, kept into the argument
  store (0x8059c42..0x8059c4e). VERIFIED. The wish length is
  `Vec3NormalizeTo`'s (0x8192a64) return, stored at 0x8059bcd: the squares
  summed and rooted on the stack (`fsqrt` at 0x8192a94; `sqrtf` only for
  a NaN), so the length is rounded once, not the sum first. VERIFIED; so
  does `Vec3Normalize` (0x81915d8, `fsqrt` at 0x81915f7). With that
  length the stored scale replays the walk's resynced row 121 exactly,
  where the stored sum had left it one bit off in `origin[0]` and an
  unrounded scale had hidden it. VERIFIED (`cod4_moves_ab`: 760 exact
  rows).
- `PM_Accelerate` (0x80592e8) keeps its step on the stack from the
  `fcmovnbe` against the add (0x8059368) into the velocity's three adds
  (0x80593a6..0x80593c7); the inlined inertia clamp keeps the step, the
  velocity it would leave, both flat squares, their product's root and
  the dot (0x805937e..0x80594db), and a clamped step is `inertiaMax`
  loaded as a float. VERIFIED. The fly's resynced run went from 414 exact
  rows to 436 on the kept step (428 alone) and the lengths. VERIFIED
  (`cod4_moves_ab`).
- The air move is CoD2's. The spectator's fly takes its forward as
  `(0, 0, 1) x right`, so the pitch never lifts it, and flies down and up on
  the lean keys 0x40 and 0x80 at `PM_MoveScale`'s rate, accel 8, the speed
  times `player_spectateSpeedScale` (1 on 1.7, 2 on CoD2). INFERRED; the fly capture's forward flight pitched 45 down stayed level
  and the lean keys moved it on z. VERIFIED live.
- A spectator moves only when its script lets it look freely: at the stock
  `scr_game_spectatetype` 1 the team-menu spectator's `ps.speed` is 0 and it
  never moves; at 2 it is 400. VERIFIED live (the fly capture's recipe).

## 7. The ladder

CoD2's (cod2-movement.md 11) with 1.7's flag numbers; the in-air press of
50 into the ladder only once the velocity is non-zero, and the rung
footstep is `EV_FOOTSTEP_RUN` with the surface (21 for none) in its parm.
INFERRED. No capture climbs one yet.

The climb's wish velocity has only a vertical part (the forward key
times `clamp((forward.z + 0.25) * 2.5, -1, 1)`) and the strafe laid on
the ladder's plane, so no key moves the body into the face: a climber
leaves the rungs only once the probe past its top meets no ladder
surface, as on CoD2 (bot-objectives.md 12.15). INFERRED.

## 8. Stance, the eye and the view

- `PM_CheckDuck` (0x80588c6, INFERRED off its 70/50/30 box heights and
  its stance writes) reads the prone key first, then the crouch, then the
  stand-up room tests, as CoD2; a sprint stands the body
  up with `EV_STANCE_FORCE_STAND`; the last stand crouches it; a refused
  prone on the ground sets `PMF_PRONE_REFUSED` (0x1000) and raises the force
  event. INFERRED. The walk capture's prone key on mp_crash's
  slope raised 0x1000 for its whole hold and never went prone. VERIFIED
  live.
- A sprint forces the stand on every cmd, not on its edge: 1.7's
  `PM_CheckDuck` tests `pm_flags` 0x8000 (0x805894c), and its arm
  (0x8058dc4) sets `viewHeightTarget` 60, clears `eFlags` 0xc and
  `pm_flags` 3 and calls `BG_AddPredictableEventToPlayerstate` (0x8052562,
  which appends to the ring unconditionally) with 6. VERIFIED. Likewise
  `PM_UpdateAimDownSightFlag` (0x8061c52) adds `EV_RESET_ADS` (0xe, through
  `PM_AddEvent` at 0x8061d9a) on every cmd whose buttons hold the sprint
  key 2, unless the weapon has an overlay reticle and the cmd holds 0x2000.
  VERIFIED. So a held sprint raises both events each cmd, and a client
  that printed its events (vcod's did) spams them; retail prints an
  event only under `cg_debugEvents`. INFERRED.
  `a_held_sprint_forces_the_stand_on_every_cmd` holds the first.
- The client takes the forced stance as its own: `CG_EntityEvent`'s
  `EV_STANCE_FORCE_STAND`, `_CROUCH` and `_PRONE` arms (`iw3mp.exe`
  0x436155, 0x436195, 0x4361d5; the switch at 0x435f58 on the event
  less 1 sends 6, 7 and 8 there), for an event whose client is the
  predicted playerstate's (the compare with 0x794550), push 0, 1 or 2
  into `CL_SetStance` (0x462870), which stores it in `cl.stance`
  (0xc84f78) unless the `+prone` (kb byte 0x8f1708) or `+movedown`
  (0x8f1604) key is held. VERIFIED (the disassembly). The cmd's crouch
  and prone bits come off `cl.stance`, and a sprint ends on either
  (0xc735 above), so a client that kept its crouch through the forced
  stand crouched again a cmd later; vcod's had (`Cod4Session::force_stance`,
  `cod4_live_sprint_out_of_a_crouch_stays_standing`). vcod's
  `Cod4Input::set_stance` makes the same held-key test.
- The stance binds (`CL_InitInput`'s registrations from 0x4644e5):
  `+prone` / `-prone` (0x462c60, 0x462c70) and `+movedown` / `-movedown`
  (0x462920, 0x462930) only press and release their kbutton (0x8f16f8,
  0x8f15f4). `lowerstance` (0x462e20), `raisestance` (0x462e60),
  `togglecrouch` (0x462ea0), `toggleprone` (0x462ed0), `goprone`
  (0x462f00), `gocrouch` (0x462f20), `+moveup` (0x4628a0) and `+gostand`
  (0x462f40) move `cl.stance` as 1.1's do (cod11-console.md 9), each
  leaving it alone while either held key's byte is set; `lowerstance`,
  `raisestance`, `+moveup` and `+gostand` also do nothing below connection
  state 5 (0xc5f900). `CL_KeyMove` (0x4631b0) sends a held `+prone` as
  0x100 with 0x1000, else a held `+movedown` as 0x200 with 0x1000, else
  `cl.stance` with 0x1000 clear (0x463160), after the `+stance` hold timer
  (0x463205..0x46323a). VERIFIED (the disassembly). vcod sends all of
  these but the `+stance` timer (`play::cod4_input`).
- `PM_CheckDuck`'s stance keys (0x8058952..0x80592e3): a gunner takes
  the turret's stance off `eFlags` 0x300; `pm_flags` 0xc00 keeps the
  stance; `pm_type` 6 crouches; on a ladder the stance keys are dropped
  with event 6; the prone key lies down where the prone test passes, and
  a refusal on the ground sets 0x1000 and raises event 7 from a lower
  stance, 6 standing; the crouch key crouches, out of prone only where a
  50-high box fits (else event 8, parm 2); no key rises to 70, else 50,
  else event 8 parm 1 from prone, and to 70 else event 7 parm 1 from the
  crouch. Every refusal event is skipped while the cmd holds 0x1000.
  INFERRED (control flow). Four things the model leaves out: `pm_flags`
  0x100000 (0x805893e) stands the body like the sprint arm but keeps
  `eFlags`; a weapon whose def has +0x54c set (0x80609de) refuses prone;
  `weaponState` 5 with the def's +0x5b8 set skips the keys (0x805907e);
  and each change calls 0x8050194 with 0xc..0x10, the anim script's
  stance events. INFERRED.
- The prone test is inlined at 0x8058f20 and also built alone at
  0x8056a7e: a prone body passes, one off the ground fails, else
  `BG_CheckProneValid` with the box's half width, height 30, the view's
  yaw and the feet 50 behind (immediate 0x42480000 at 0x8058f56).
  VERIFIED (the immediates); the order is INFERRED.
- `BG_CheckProneValid` is 0x8054fa2 behind a forwarding wrapper at
  0x8055ad8, INFERRED off its callers and constants. Its arguments, in
  order: the entity, the origin, the half width, the height, the yaw, the
  torso and waist pitch pointers, already prone, on the ground, walkable,
  a trace handler index into 0x826af40, a mask selector (0x810011 for 0,
  0x820011 otherwise, 0x8054fd0) and the feet distance. VERIFIED (the
  stack slots and the `sbb` at 0x8054fd4). Its constants are rodata 6
  (0x820aacc), 18 (0x820a314), 22 (0x820aad0), 0.7 (0x82096a4), 2.5
  (0x820a31c), 2 (0x820964c), -0.75 (0x820aad4), -50 (0x820aad8), 70
  (0x820aadc), 5 (0x820aae0) and 10 (0x820aac8). VERIFIED. What it does
  with them, INFERRED off the control flow: a body not yet prone needs its
  box clear 10 up; one on the ground needs walkable ground; a six-unit
  probe sweeps from `height - 6` over the origin to `feet - 6` behind; a
  run cut short fails off the ground, fails under `width + 2`, and under
  the sill `0.7 (h - 6) + 18` runs again to an end 22 higher, whose line
  the later probes follow; the middle drops from 18 out by
  `2.5 width + h - 12` and the feet by twice their height over the middle
  plus the width, each needing a walkable contact; a cut-short run lays the
  legs from the middle along the line to where it stopped, half way to the
  full reach, climbing a wall on that line once by 18; the torso pitches
  off the origin to the middle, the legs off the middle to the feet, and
  the bend between them must sit in -50..70 with two point lines 5 up
  clear. A missing floor fails only a body on the ground; one in the air
  passes with both pitches zeroed (0x8055307).
- The arithmetic of 0x8054fa2 keeps the run's start height, the middle's
  height and its drop on the stack, stores each point it hands a trace,
  normalizes the bent legs' line with the length off the kept components
  and the scale on the stored ones (0x80558f8..0x805593b), and runs the
  three `AngleNormalize180`s inline with the turns kept (0x8055562,
  0x80555f2, 0x8055647). VERIFIED (the disassembly). `vectopitch`
  (0x8192510) passes the flat length to `atan2` as a double; the model's
  had rounded it to a float. VERIFIED (the disassembly).
  `stance::tests` pins the fit on a box world: those last two changes
  moved only the pitches' last bits, and no 1.7 capture lies prone yet.
- The prone arm of `PM_UpdateViewAngles` (0x8057858..0x8057e2a): the
  body swings toward the view at 55 a second (0x820deb8, times 0.001) when
  the view is within 5 (0x820aae0) of `bg_prone_yawcap` (dvar 0x827af6c)
  or a move key is held off the body's heading; a heading that does not
  fit turned is walked back a degree at a time, and a fitting one is taken
  only if the 45-feet fit passes at the view and at the heading. The turned
  fit (`BG_CheckProneTurned`, inlined at 0x8057944 and 0x8057c28) blends
  the heading back toward the view by `turn / 240` (0x820debc) and the feet
  between 45 (0x820dec0) and 50 (0x8209670). A body still off the view
  that does not fit drags the view a degree a step; then the yaw cap and
  the 45-degree torso pitch cap (0x820dec0, -45 at 0x820dec8) push their
  overshoot into `delta_angles`. A refusal sets 0x1000 and eases a turn
  of a degree or less back by 0.98 (0x820dec4). VERIFIED (the constants);
  the order is INFERRED. The pitch cap's view goes through an inline
  `AngleNormalize180` of the kept sum (0x8057dd2). VERIFIED.
- The gunner's arm of `PM_UpdateViewAngles` (0x8057691..0x8057796):
  pitch then yaw, an arc of 180 or more is skipped (0x8209668), and a view
  past `base ± range` (ps+0x128, +0x130) is put on the near edge with the
  overshoot added to `delta_angles` in one rounding. The yaw's offset is
  `AngleDelta(base, old yaw)` stored plus `AngleDelta(old yaw, view)` kept
  (0x8057739..0x8057769), so it reads past the wrap. VERIFIED (the
  disassembly); the order is INFERRED. `stance::tests` pins it.
- The calls to `AngleDelta` (0x8190590) and `AngleNormalize360`
  (0x8190540) return on the stack, the difference and the turns unrounded.
  VERIFIED (the disassembly). `pmove/fpu.rs` stores both, the seed's
  model, and the prone and gunner arms call it.
- The eye's curves are `.data` 0x826b000 (down to the crouch), 0x826b0e0
  (up to the stand), 0x826af60 (down to prone) and 0x826b080 (up from it),
  `{percent, height, posadj}` rows ending at percent -1, every posadj 0.
  VERIFIED. `PM_ViewHeightAdjust` (0x805817e) times them 400 into prone and
  up out of it, 200 otherwise (0x8058263), and meets a target off them at
  180 a second (0x8209668), comparing the kept sum with the target
  (0x8058204). VERIFIED. The curve lookup keeps the leg's fraction unrounded:
  the walk capture's eye read 54.08 at 62% up from the crouch, where a
  stored fraction gives 54.079998. VERIFIED live.
- A leg turned round puts its start back by `dur * 0.01 * pct` stored as a
  float and truncated (`fstp`, `cvttss2si` at 0x80583b4): 50% of 400 is
  200, where the double product truncates to 199. VERIFIED (the
  disassembly); the model had truncated the double. No capture turns a leg
  round yet.
- The clamp of `PM_UpdateViewAngles` wraps `delta + short * 360/65536` with
  `(x - floor(x + 0.5)) * 360` of the turns, kept (section 1). INFERRED.
- `PM_UpdateLean` (0x80567f4) counts the lean keys (0x40, 0x80) unless
  `pm_flags` 0x800 is set, the body is dead, off the ground and not
  `pm_type` 1, or on a turret; it steps out by `msec / 350 * reach`
  (0x820dea0, 0x820dea4) and back by `msec / 280 * reach` (0x820de9c,
  0x820de98), the reach 0.25 at the prone eye target (0x820966c) and 0.5
  otherwise (0x8209628). VERIFIED (the constants); the gating is INFERRED.
  Outward a lean past the reach is put on it at once; back, the step stops
  at 0. INFERRED. A held lean's wall test sweeps a ±8 box from the eye
  to `AddLeanToPosition` (0x81a9f8e) at a full lean under 0x2810011, its
  fraction through 0x81a951c. VERIFIED (the arguments at 0x80568c4,
  0x8056909).
- The wire does not carry the view. A `-87` field (the three viewangles)
  goes out only on an archived snapshot, a teleport (`eFlags` 0x2 toggled),
  `otherFlags & 2`, a locked view or intermission
  (`MSG_WriteDeltaPlayerstate` 0x813f586, cod4-netfields.md 7); the walk capture's viewangles read 45 for all
  789 snapshots after the second spawn while the walker turned and walked
  at -135. VERIFIED live. The A/B compares no viewangles.
- Every angle field (`-100`, `-87`) goes out as `(int)` of the stored
  float `f * 182.04445f + 0.5f` (`cod4_lnxded` 0x81311aa, called from the
  field writer at 0x813e8fd; the compares beside it, 0x813e73d and on,
  round the same way). VERIFIED (the disassembly). `(int)` truncates
  toward zero, so a positive angle goes out rounded to the nearest short
  and a negative one a short nearer zero than `(int)(f * 182.04445f)`
  wherever its fraction of a short is under a half. The fly spectator's
  delta of 65.0 (`SetClientViewAngle` off the intermission's 65 with a cmd
  short of 0) reads 11833, 65.00061. VERIFIED live (the fit of
  section 13 lands on 65.0 exactly). vcod-server holds the floats and
  rounds them only as it sends (`delta::angle16_sent`, `EntityState::sent`,
  `PlayerState::sent_angles`); a capture's decoded angle re-encodes as
  itself through the plain truncation.

## 9. Footsteps

`PM_Footsteps` measures `xyspeed`, then runs the bob cycle at
`xyspeed / PM_GetMaxSpeed` times the bob rate (rodata 0x820e220) for
the stance and walking (the sprint's `player_sprintCameraBob` standing), a footstep when the cycle crosses a
quarter: `EV_FOOTSTEP_RUN`, `_WALK`, `_PRONE` or `_SPRINT` with the surface
in the parm. On a ladder the cycle runs off vz. INFERRED. The
walk capture's footsteps are 73, `EV_FOOTSTEP_RUN`. VERIFIED live.

In `cod4_lnxded` `PM_Footsteps` is 0x805a516. The bob rates are twelve
floats at 0x820e220, read at `2 * stance + walking` (0x805ac99), the
backward rows repeating the prone and crouched ones. VERIFIED. The rate
`xyspeed / max * factor` stays on the x87 stack, and `bobCycle + msec *
rate` is stored as a float before its truncation to a byte (0x805a7cd).
VERIFIED. `PM_GetMaxSpeed` is inlined at 0x805a727 and keeps its value on
the stack except across the weapon def's lookup (0x805acc1) and the
stance scale's call (0x805a790). VERIFIED. The climb's cycle (0x805a9b8)
runs from 300 ms after a jump at `vz / 95.25 * 0.45`, sighted or leaning
`vz / 38.1 * 0.35` (0x820def0..0x820defc), its rate stored across the
anim call. VERIFIED.

## 10. The jump

`Jump_Check`, `Jump_Start`, the slowdown (`Jump_ApplySlowdown` 1800 ms at
0.65 under the start plus 18, else 1200 ms at 0.5), the friction curve and
the step clamp are CoD2's, plus `PMF_NO_JUMP`, the takeoff clearing the
hard landing and knockback flags and the sprint's button latch, and the
jump event `EV_JUMP` (76) with the surface in its parm (21 on a ladder).
INFERRED. In `cod4_lnxded` the slowdown at 0x8050958 compares
`pm_time` (ps+0x18) with 0x708 and loads 0.65 (0x820a310) or 0.5
(0x8209628), the 18 at 0x820a314; the curve is section 1's. VERIFIED.
Read off 0x8050958: at `pm_time` 0 the landing is high when `origin.z`
is not under `jumpOriginZ + 18` (kept); a timer past 1800 clears the jump
with the 0.65 cut; a running penalty cuts nothing; the cut scales the
three components, each stored, only with `jump_slowdownEnable`. The
friction curve (0x80509e6) clears the jump past 1800 whatever the dvar,
and returns 2.5 from 1700 on (`pm_time > 0x6a3`). The takeoff's aim
spread is stored, then compared with 255 unrounded. VERIFIED.

## 11. The sprint

`PM_UpdateSprint` (0x80570ae) starts a sprint on the sprint key with the
forward key past `player_sprintForwardMinimum`, no key of 0xc435
(0x805718d: fire, melee, both reloads, jump, both grenades), no lean, no
ladder, mantle, sight or shellshock (0x1001c at 0x80571b0; a start never
clears the sight), no melee or offhand state (12..20, waived once a jump's
`pm_time` has run out), room to stand and more than `player_sprintMinTime`
left; it ends on 0xc735 (0x80572af: the stance keys as well), a sight, a
ladder, the forward key falling, weapon states 12..20, 25 and 26, the time
used up (`sprintDelay` then holds the recharge pause, 0x8057276) or a
second press. Only `pm_type` 0 and 1 sprint (0x80570d1). VERIFIED. The
time left after a sprint is its unused allowance plus the ms since it
ended, less the recharge pause in ms stored and truncated (0x80573cb) when
`sprintDelay` is set. VERIFIED. `BG_GetMaxSprintTime` (0x80526a4) is
`player_sprintTime` times 1000 stored, times the weapon's
`sprintDurationScale` (WeaponDef 0x420) and `perk_sprintMultiplier` under
perk 0x400, truncated, at most 0x3fff ms. VERIFIED. No capture sprints yet.

## 12. Dvars

`tools/re/cod4_dvars.py` reads the registrations out of `cod4_lnxded`: the
call that registers `jump_height` is `Dvar_RegisterFloat` (0x81a273c), the
one registering `jump_slowdownEnable` `Dvar_RegisterBool` (0x81a2664), the
one registering `player_sprintForwardMinimum` `Dvar_RegisterInt`, and every
call to those three is decoded from the stores that build its arguments
(the function names INFERRED off their arguments; the values VERIFIED).
The walk capture's dvar mirror carries each name at a codinfo slot `n` and
its value at `n + 128` (`friction` at 36 and 164): every value equals the
default below. VERIFIED live. `Rules::default()` holds them.

| dvar | default | range | dvar | default | range |
|---|---|---|---|---|---|
| `friction` | 5.5 | 0..100 | `stopspeed` | 100 | 0..1000 |
| `inertiaMax` | 50 | 0..1000 | `inertiaAngle` | 0 | -1..1 |
| `jump_height` | 39 | 0..1000 | `jump_stepSize` | 18 | 0..64 |
| `jump_slowdownEnable` | 1 | | `jump_ladderPushVel` | 128 | 0..1024 |
| `jump_spreadAdd` | 64 | 0..512 | `bg_ladder_yawcap` | 100 | 0..360 |
| `bg_prone_yawcap` | 85 | 0..360 | `player_view_pitch_up`, `_down` | 85 | 0..90 |
| `bg_fallDamageMinHeight` | 128 | 1.. | `bg_fallDamageMaxHeight` | 300 | 1.. |
| `bg_foliagesnd_minspeed` | 40 | | `bg_foliagesnd_maxspeed` | 180 | |
| `bg_foliagesnd_slowinterval` | 1500 | | `bg_foliagesnd_fastinterval`, `_resetinterval` | 500 | |
| `player_moveThreshhold` | 10 | 1e-8..20 | `player_footstepsThreshhold` | 0 | 0..50000 |
| `player_strafeSpeedScale` | 0.8 | 0..20 | `player_backSpeedScale` | 0.7 | 0..20 |
| `player_spectateSpeedScale` | 1 | 0..20 | `player_sprintForwardMinimum` | 105 | 0..255 |
| `player_sprintSpeedScale` | 1.5 | 0..5 | `player_sprintTime` | 4 | 0..12.8 |
| `player_sprintMinTime` | 1 | 0..12.8 | `player_sprintRechargePause` | 0 | 0..9000 |
| `player_sprintStrafeSpeedScale` | 0.667 | 0..5000 | `player_sprintCameraBob` | 0.5 | 0..2 |
| `player_dmgtimer_maxTime` | 750 | 0.. | `player_dmgtimer_minScale` | 0 | 0..1 |
| `player_dmgtimer_stumbleTime`, `_flinchTime` | 500 | 0..2000 | `perk_sprintMultiplier` | 2 | 0.. |
| `mantle_enable` | 1 | | `mantle_check_range` | 20 | 0..128 |
| `mantle_check_radius` | 0.1 | 0..15 | `mantle_check_angle`, `mantle_view_yawcap` | 60 | 0..180 |
| `player_breath_hold_time` | 4.5 | 0..30 | `player_breath_gasp_time` | 1 | 0..30 |
| `player_breath_gasp_scale` | 4.5 | 0..50 | `player_breath_hold_lerp` | 1 | 0..50 |
| `player_breath_gasp_lerp` | 6 | 0..50 | | | |

## 13. The moves A/B

`cargo test -p vcod-cod4 --test cod4 moves_ab -- --nocapture` with
`COD4_DIR` replays each capture's own `clc_move` cmds through
`player_step` on mp_crash's clip map, from the walker's last spawn (the
snapshot whose `commandTime` no cmd carries) or the spectator's first free
snapshot, and compares every later snapshot's pmove words (origin,
velocity, `pm_flags`, `pm_time`, `delta_angles`, `groundEntityNum`, the
jump's time and height, `movementDir`, `leanf`, the eye's target, height and
lerp, `bobCycle`, `proneDirection`, the sprint state, and `eFlags`' crouch,
prone and talk bits) bit for bit. The free run never looks back at retail;
the resynced run restarts each row from the snapshot. `MOVES_REPORT=1`
prints every wrong row, `MOVES_TRACE=<capture>:<free|resync>:<row>` one
row's cmds.

### 13.1 Results

| capture | rows | cmds | free run exact | resynced exact | free run's largest origin error |
|---|---|---|---|---|---|
| walk | 787 | 1223 | 0 (first: row 53, z one bit) | 762 | 0.008 units |
| fly | 438 | 1303 | 131 | 436 | 0.000 units |
| mantle | 297 | 882 | 137 | 290 | 0.000 units |
| lunge | 147 | 441 | 23 (first: row 155, the server's events) | 146 | 0.000 units |

VERIFIED (the gate's output). The walk's free run stays within 0.008 units of
retail over 42 s of running, strafing into walls, jumping, crouching and the
refused prone; every wrong row is one of the gaps below.

The server's `delta_angles` is a float the wire rounds to a short (section
8), and a spectator's unsnapped velocity shows the difference within one
cmd. The gate takes the yaw as the float among those the writer sends as
the wire's short whose free run matches retail longest (the wire's own, or
for a negative short the one a short further from zero, on a tie): the fly
capture's lands on 65.0, the walk capture's stays the wire's 110.00061.
Ours are compared as the writer sends them.
VERIFIED (the gate's fit line). That fit uses the capture's motion; a
server holds the float itself.

### 13.2 GAPS

- The float model: the walk's z one bit off on the terrain (24 rows) and
  the sign of a zero vz after it (11 rows), pressed into a wall on a slope;
  the spectator's velocity a bit off twice (4 cells: the friction's
  leftover at a stop, row 132, and a turn, row 244). The functions section 1 lists replay exactly
  under the kept model; the trace kernels (`vcod_common::clipmap`) and the
  rest of pmove still compute in the stored or plain float model, and the
  last bits that move are there. INFERRED.
- The steep step: rows 499 and 505..508 of the walk land a few ulps off
  retail stepping onto and back off a 0.78 brush slope: ours reverts to the plain
  slide, whose move equals the step's, where retail's step went further.
  The revert test is retail's (section 1); the difference is upstream of
  it. Not found.

## 14. Open

- The rest of pmove and the trace kernels in the kept model, which the
  gaps above point at: `PM_StepSlideMove` at 0x805e930
  and the `PM_SlideMove` it calls at 0x805e030 (both INFERRED, section 1)
  are the next to read.
- Captures for the sprint, the ladder, a prone body and the sighted
  walk.
- The server's open items are section 15's last list.

## 15. The server: pmove in the think

`vcod-server --game cod4` runs `player_step` for every cmd as its packet
arrives (`game::cod4::level::Cod4Level::think`, `THINK` on arrival,
cod4-server-frame.md 2). What the server adds around the step:

- `ClientThink_real` holds the cmd's clock within [-1000, +200] ms of
  `level.time`, skips a cmd no newer than `commandTime`, writes `ps.speed`
  from `g_speed` before the move, and relinks the body after it
  (`SV_LinkEntity`). `SpectatorThink` flies a spectator that follows no
  one at `ps.speed` 400 where it may look freely
  (`G_ClientCanSpectateTeam(TEAM_NUM_TEAMS)`), else 0, with a zeroed
  `oldcmd`, and unlinks it. INFERRED.
- 1.7's `SpectatorThink` is `cod4_lnxded` 0x80a4e82. It copies
  `buttons` to `oldbuttons` (client +0x3088, +0x308c), takes the cmd's
  buttons (`sess.cmd` +0x2f94) and gathers the new presses at +0x3094.
  With no forced target (`forceSpectatorClient` +0x2f68 negative), free
  flight allowed (no 0x10 in the refusal mask at +0x3004) and a target
  followed (+0x3074 not negative), a change of 0x4 (melee) either way
  calls `StopFollowing` (0x80af24c). A fresh attack press (0x1) then
  calls `Cmd_FollowCycle_f(ent, 1)` (0x80aebee), else a fresh 0x800 (the
  sight) `Cmd_FollowCycle_f(ent, -1)`. VERIFIED (the instructions).
  `Cmd_FollowCycle_f` does nothing outside session state 2 (+0x2f64) or
  with a forced target. It steps `dir` from the followed slot (0 with
  none), wrapping at `sv_maxclients` (0x836f8a4), back to where it
  started, and takes the first slot whose archived info
  (`SV_GetArchivedClientInfo`, 0x81795e2) is on a team
  `G_ClientCanSpectateTeam` (0x80a479a) allows. VERIFIED (the
  instructions). `StopFollowing` sets the forced target, the killcam
  entity and the target to -1. A follower (`otherFlags` 2) is put 40
  behind and 10 over the followed eye along the view's forward and up
  (-40 at 0x8219944, 10 at 0x820aac8), an 8-unit capsule traced there
  under 0x810011 and stopped short, looking 15 lower (0x8215e54). It
  clears `eFlags` 0x300, the view lock (+0x598 0, +0x59c 1023),
  `pm_flags` 0x10010, `weapFlags` 0x40, `otherFlags` 2 and
  `fWeaponPosFrac`. VERIFIED (the instructions). vcod:
  `Cod4Level::spectator_buttons`, `stop_following`, `follow_cycle`,
  with "playing" standing in for the archived info. INFERRED. The live
  tests `cod4_live_spectator_*` (client `play::cod4`) fly, follow a bot
  and let go on ours at `scr_game_spectatetype` 2. On `war` at 1 the
  team menu's spectator watches no one: `setSpectatePermissions` refuses
  it every team. On `dm` at 1 it follows the first player. VERIFIED
  (`_spectating.gsc`, the tests on ours). Retail does the same: `cod4_lnxded`
  1.7, `war` on mp_crash at `scr_game_spectatetype 1`, an allies
  `cod4_probe` idling and two team-menu spectators, one pressing attack
  every 3 s (`--script follow`), one flying (`--script fly`): for 25 s
  both read `pm_type` 4 at the spectator spot (336, 60, 260) with their
  own `clientNum`, never moving nor following. VERIFIED live
  (a local spectator script).
- The trace is `SV_Trace`'s: the clip map, then each other client's body
  as a temp box model of `CONTENTS_BODY` (`r.mins`, `r.maxs` as its last
  move left them: 70, 50 or 30 high), then each solid brush entity's
  inline model in its own frame (`game::cod4::clip::Cod4Clip`). INFERRED.
  `CM_Trace` (0x811ce5c) sends the temp box model (0xfff, compared at
  0x811d21d) to its own arm at 0x811d29c, which culls on the box grown by
  a unit and builds the end spheres' centres at the sphere offset over and
  under the start and end. VERIFIED (the instructions). So a body is a
  capsule, as CoD2's (cod2-movement.md 20): the touch capture's walker, coming at the target about 26 to
  the side, stopped 15.4 short of its centre in y, on its round side,
  where a box would have stopped it 30.1 short. VERIFIED live. vcod clips a client with
  `movetrace::clip_body_kept`, 1.1d's capsule arithmetic; its plain-float
  twin gives the same rows. VERIFIED (the gate, 15.3). mp_crash's script
  movers carry `solid` 0 (VERIFIED, the walk capture's entities); no
  capture walks into a brush entity yet.
- `ClientImpacts` (`cod4_lnxded` 0x80a501a) runs after the link, per cmd:
  for each entity in the touch list (pm+0x48 count, numbers from pm+0x4c)
  not listed earlier, with the script system up (0x815cfe0), `touch` on
  the client with the entity, then on the entity with the client (the
  `touch` const at 0x836f15a), then the entity's handler touch (handler
  row +0xc, table 0x826cee0) and the client's own. VERIFIED (the
  instructions). The rows with a touch are 1 and 3 (triggers, met through
  `G_TouchTriggers`), 7, 16, 17 and 18 (items, `0x8092a44`) and 22 (a
  vehicle's). VERIFIED (the table's bytes). A waiter wakes once a frame:
  the touch capture's gsc counted one `touch` a frame each way while the
  bodies pressed, through three cmds a frame. VERIFIED live. That each of
  the three met the body is INFERRED.
  vcod: `Cod4Level::after_think` hands the touch list to
  `ScriptRuntime::client_impacts` after `ClientEvents`, and the spawn's
  think does the same.
- `SetClientViewAngle` (0x80a77f6) keeps each axis's turns on the x87
  stack: the cmd's int angle times -360/65536 (0x820deb4) plus the view,
  times the float 1/360 (0x8209630); less the floor of that plus a half
  (`frndint` under a round-down control word), stored as a float; times
  360 (0x8209634), one store into `delta_angles` (ps+0x64). VERIFIED (the
  instructions). With `f32` steps vcod's spectator's delta came out a few
  ulps off, and the free flight's velocity drifted from its first stroke.
  VERIFIED (the gate, 15.3).
- `ClientSpawn` puts `commandTime` 100 ms back (0x80a7e5b) and runs
  `ClientEndFrame` (0x80a68b8) and `ClientThink_real` (0x80a5c1a) on the
  last cmd at `level.time` (0x80a7e50) before it returns. VERIFIED (the
  instructions). It sets `PMF_RESPAWNED`. INFERRED, not yet confirmed on
  1.7.
  The playerstate is the memset's, so `moveSpeedScaleMultiplier` and the
  weapon are still 0 and the think moves nothing whatever the cmd holds:
  the walk capture's walker held forward 127 into its spawn and its spawn
  frame reads velocity 0, the ground (1022) and `holdBreathScale`
  0.519216, a 66 ms and a 34 ms step of the breath's track from 0
  (VERIFIED live). After the cmd, `ClientEvents` shoots off its fire
  events (cod4-combat.md 3).
- `PM_UpdateHoldBreath`, inside `PM_Weapon` after the footsteps: the
  breath's timer runs down by the step's ms, the target is `(gasp_scale -
  1) * timer / (hold + gasp) + 1`, scaled toward 1 by `fWeaponPosFrac`,
  and `holdBreathScale` follows it through `DiffTrack` at
  `player_breath_gasp_lerp` (6) per second. INFERRED; the walk and fly captures' `holdBreathScale` match
  `cod4_moves_ab` on every row (VERIFIED, the gate). Holding the breath
  (the 0x2000 key on a scope) is the weapon half's (cod4-combat.md 2); no
  capture holds it.
- The breath's weapon def is the view weapon's: `PM_UpdateHoldBreath`
  tests `weapFlags` 2 (`PWF_USING_OFFHAND`) at `cod4_lnxded` 0x8062e42
  and indexes the def table (0x828d0c0) by `offHandIndex` (ps+0xe0) when
  it is set, by `weapon` (ps+0xe8, 0x8062f24) when not; the scope test
  then reads the def's +0x438 `overlayReticle` (0x8062eb9). VERIFIED
  (the disassembly). So a sighted scope with a grenade on the view model
  holds no breath; `pmove::tests`'
  `the_breath_reads_the_offhand_while_it_shows` pins it.
- `placeSpawnPoint`: the player's capsule swept 128 up under
  `MASK_PLAYERSOLID`, then 262144 down from where it stopped, each stop
  `Vec3Lerp(start, end, fraction)`. INFERRED
  (`GScr_PlaceSpawnPoint`). 1.7's own (0x80c56c6) runs the same three traces
  with the same box, mask and offsets (cod4-collision.md 5). VERIFIED
  (the disassembly). The walker's spot (-22, 1789, 252) lands at
  z 224.2543 (0x43604119) where retail's reads 224.25427 (0x43604118):
  the drop's fraction off mp_crash's terrain is a bit off, every way of
  rounding the lerp gives ours. VERIFIED (the gate). That is section
  13.2's float model of the trace kernels.
- The snapshot leaves out every entity `hide` hid: `ScrCmd_Hide` sets
  `EF_NODRAW` and every bit of `r.clientMask`, INFERRED. The four
  `com_bomb_objective_d` models `war` hides (105, 109, 120, 364) are in
  every 1.7 capture's baselines with `eFlags` 0x20 and in no snapshot of
  any of them. VERIFIED live.
- No PVS cull. The walker walked 40 s across mp_crash and the spectator,
  the shooter and the snapshot capture's spectator stood elsewhere: every
  one of the four captures sends the same eleven script models in every
  snapshot (77, 78, 79, 92, 93, 152, 153, 154, 212, 366, 367), plus the
  players and the frag. VERIFIED live. With one cluster row on every
  stock map (cod4-collision.md) the snapshot culls nothing by position.
  INFERRED. Ours sends every entity not hidden.
- A LAN client's next message is due the next frame; only a fragment
  waits on the rate (`SV_SendMessageToClient`, INFERRED). The
  loopback walker at `sv_maxRate` 5000 was sent its 359-byte spawn frame
  and its next snapshot 50 ms apart, where the rate would hold it 81 ms.
  VERIFIED live. vcod's LAN test is CoD2's (`engine::link::Lan::Cod2`,
  127/8 among them); 1.7's `Sys_IsLANAddress` is not read.
- A body's entity origin is `(int)` of the playerstate's, toward zero
  (`BG_PlayerToEntitySetTrajectory`, INFERRED): the spectator
  capture reads the walker at x -32 off -32.87, z 224 off 224.79. VERIFIED
  live. Its `movementDir` is the playerstate's (VERIFIED, the gate).

### 15.1 `cod4::wire_ab`'s walk

`cargo test -p vcod-server --lib -- cod4::wire_ab --nocapture` (`COD4_DIR`):
the three probes of section 13's walk capture join ours in process, and
from its spawn (47900) the walker sends the capture's own cmds, each
packet before the frame of the first retail snapshot whose `commandTime`
reaches it. Every retail snapshot to 87300 is compared with ours of the
same server time, word for word, and ours is never resynced:

| what | rows | exact | off only in the float model |
|---|---|---|---|
| the walker's playerstate (section 13's words, `holdBreathScale`, `eFlags`' pmove bits) and its teammate's origin and yaw | 789 | 410 | 379, the largest origin error 0.0083 at 70600 |
| the spectator's own playerstate (`commandTime` aside: its cmds are the probe's) | 789 | 789 | |
| the walker's body as the spectator is sent it | 789 | 789 | |

VERIFIED (the gate's output). The walker's run is `cod4_moves_ab`'s free
run: the same words go wrong at the same rows (z a bit off on the terrain,
the sign of a zero vz, x from the steep step at 70300 on), and every other
word, the jumps, the crouch, the refused prone, the eye, the ground and the
breath, is exact on all 789. The gate's `WALK_GAPS` counts the rows each
wrong word takes. Taking 1.7's trace set-up (cod4-collision.md 2) moved the
walker from 73 exact rows to 410, y and the body's origin to exact on all
789, and the steep step's error from 0.0385 to 0.0083. VERIFIED (the
gate).

### 15.2 Open

- The body's entity: `BG_PlayerToEntityProcessEvents` (`cod4_lnxded`
  0x8052b84..0x8052ca0) copies the playerstate's events since
  `oldEventSequence` (+0xd8), at most the last four and none when the
  sequence went back, each event and parm read as a byte, onto the
  entity's ring (`eventSequence` +0xa0 stepped as a byte), but the
  server-only ones (1.7's list at `cod4_lnxded` 0x820d954, file 0x1c5954:
  31, 20, 19) and the single-client ones (0x820d920, file 0x1c5920: 6, 7,
  8, 34, 13, 14, 32, 33, 34, 37, 42, 43), each list ended by -1, which
  take no slot. VERIFIED (the bytes and the instructions).
  vcod-server copies them at the end frame
  (`level::BodyEvents`), and the walk gate's body rings match retail on
  all 789 rows. VERIFIED (the gate). Its angles go out `(int)`, then through the
  writer's half-short rounding (section 8): the walk's body yaw and the
  idle teammate's match retail on all 789 rows (cod4-player-anims.md 8).
- The triggers' touch pass runs at the head of `G_RunFrame`
  (`Cod4Level::touch_triggers`); 1.7's `ClientThink_real` calls
  0x80d3762 after `ClientImpacts`, which has not been read. The weapon
  half and the shots are cod4-combat.md's.
- No capture walks into a brush entity or a script model.

### 15.3 The touch and free flight capture

`cod4::capture_ab::cod4_touch_and_flight_ab` (fixtures
`play-1.7-touch-{target,walker,flyer}.txt`, the gsc
`crates/cod4/tests/fixtures/gsc/touch_pass`): `cod4_lnxded` 1.7 in
cod4-handshake.md 7's namespace, `+set g_gametype war +set sv_pure 0 +set
scr_game_spectatetype 2 +set fs_game mods/vcodtouch +devmap mp_crash`, the
gsc in the mod's iwd. 35 s on, `target` (allies, `--goto "207 2291 296"
--script idle`), a second later `walker` (axis, `--goto "207 2211 296"
--script push --face 90`) and a second after that `flyer` (`--team
spectator --script fly`). The walker walks into the target, strafes along
it, backs off and walks in again 26 to the side; the gsc prints, each
frame, how many `touch` notifies each player got and from whom. Ours
replays the three on retail's clock, seed and packet order and compares
every playerstate word, every entity and the printed lines, all three
clients, from the last spawn. VERIFIED (the gate):

- Every printed line matches retail's, time and count (one a frame each
  way while the bodies press), and the walker's path, the second stop on
  the target's round side included.
- The flyer's playerstate matches but for `velocity[1]` in 4 rows of its
  turn, a few ulps (section 13.2's float model).
- The walker's `delta_angles[1]` reads 180 off retail's for the 0.55 s
  after its `setviewpos`, and its body's yaw with it (cod4-combat.md 15).
- Before the capture ours had no `ClientImpacts` (no line printed), clipped
  bodies as boxes (the second stop 15 short of retail's) and rounded
  `SetClientViewAngle` in `f32` (the flyer off from its first stroke).

## 16. The weapon half

`pmove::weapon` (`PM_AdjustAimSpreadScale`, `PM_UpdateAimDownSightFlag`,
`PM_Weapon` with the offhand and the melee) and `pmove::anim` (the anim
script) are cod4-combat.md 1, 2 and 10 and cod4-player-anims.md 7.
`Mantle_IsWeaponInactive` is called at both of `PM_Weapon`'s places: the
weapon goes away quick during a mantle and the next change after one is
quick (cod4-combat.md 2); the `mantle` rows replay armed.

## 17. The mantle

`vcod_cod4::pmove::mantle` ports 1.7's mantle. The functions carry no
symbols; each is named by its debug strings (`Mantle_DebugPrint`'s
lines are compiled in: "Mantle available!" 0x820a5ac, "====== Mantle
Debug ======" 0x820a5be, "Mantle Failed: Forward movement is blocked"
0x820a808 and the rest) and by the structure around them. INFERRED.

| function | `cod4_lnxded` |
|---|---|
| `Mantle_RegisterDvars` | 0x8050e70 |
| `Mantle_CreateAnims` | 0x8050fa4 |
| `Mantle_ClearHint` / `Mantle_IsWeaponInactive` | 0x8050d80 / 0x8050d90 |
| `Mantle_CapView` | 0x8050dc6 |
| `Mantle_GetAnimDelta` | 0x80513a6 |
| `Mantle_Move` | 0x80514f2 |
| `Mantle_CheckLedge` (with `Mantle_CalcEndPos`, `Mantle_Start` and `Mantle_FindTransition` inlined) | 0x8051768 |
| `Mantle_Check` (with `Mantle_FindMantleSurface` inlined) | 0x8051f7a |
| `XAnim_CalcDeltaForTime` / `XAnimGetAbsDelta` / `XAnimGetLengthMsec` | 0x81b0930 / 0x81b5e50 / 0x81b726c |

### 17.1 What differs from CoD2's

1.7's is CoD2's (cod2-movement.md 9) but for these, each labelled on
its own:

- The dvars are registered with flags 0x180 (CoD2 0x1180); the defaults
  are section 12's. VERIFIED (`tools/re/cod4_dvars.py`).
- Every box is the player's 15 (rodata 0x820e030), not `maxs.x`: the
  surface probe sweeps (-r, -r, 0)..(r, r, 70) (0x820e02c, 0x820e038)
  from `15 - r` behind the origin to `mantle_check_range + 15 - r` ahead
  under `CONTENTS_MANTLE` 0x1000000; the ledge box is 15 wide and 30
  tall, then 50. The constants are VERIFIED (the rodata). The sweeps'
  shape is INFERRED, not yet confirmed on 1.7 instruction by
  instruction.
- The ledge's floor must be the trace's walkable flag (CoD2 compares its
  normal with 0.7). INFERRED, not yet confirmed on 1.7.
- The traces are `PM_trace` (0x8055f10, the handler's trace, the
  client's number passed, `pm->tracemask`). INFERRED (the calls).
- The binoculars' weapon states no longer refuse a mantle; dead
  (`pm_type` 7 or more), mantling and crouched or prone (`eFlags` 0xc)
  still do. INFERRED, not yet confirmed on 1.7.
- The mantle's `eFlags` bit is 0x8000 (CoD2 0x4000). INFERRED, not yet
  confirmed on 1.7.
- `Mantle_Check` clears the `mantleState.flags` bit 0x10 first, and
  `Mantle_Move` sets it on the frame the mantle ends. INFERRED, not yet
  confirmed on 1.7.
- `Mantle_Start` writes `pmove_t` 0xf0 (the mantle started), 0xf4..0x100
  (`mantleEndPos`) and 0x100 (`mantleDuration`); `Mantle_Move` clears
  0xf0 at the end. INFERRED, not yet confirmed on 1.7.
- `Mantle_CapView` adds the wrapped difference to the float
  `delta_angles[1]` unrounded (CoD2 adds shorts). INFERRED, not yet
  confirmed on 1.7.

The playerstate's `mantleState` is at 0x5e0 (yaw, timer, transIndex,
flags), the four `mantleState.*` netfields. VERIFIED (the netfield
table's offsets, `net::fields`). The default arm of `PmoveSingle` (0x805bd14)
runs, off a mantle, the sight flag (0x8061c52), the sprint (0x80570ae),
the walking flag (inlined), `PM_CheckDuck` (0x80588c6) and
`PM_GroundTrace` (0x805ae58), then `Mantle_Check` (0x805bffc) either
way; with `pm_flags` 4 set it clears the ladder, sets the ground to
1023, clears `pml.walking` and `groundPlane`, runs the sight flag, the
sprint, the walking flag and `PM_CheckDuck` again, then `Mantle_Move`
(0x805c0ae) and `PM_Weapon` (0x8062df0), and returns. INFERRED (the
calls' order and branches). `Mantle_ClearHint` runs every frame after
the view (0x805bf50); `PM_UpdateViewAngles` (0x80573fa) calls
`Mantle_CapView` instead of the ladder, prone and lean steps while
`pm_flags` 4 is set. INFERRED (the branches).

### 17.2 The arithmetic

1.7 keeps the x87 locals here as in section 1. VERIFIED (the
disassembly):

- `Mantle_Move`: each axis' step `d1 - d0` is stored with `fsts` and the
  sum `step + origin` takes the kept difference; the velocity is the
  stored step times `1 / (0.001 * dt)`, the scale kept.
- `Mantle_GetAnimDelta`: the fraction is `fild t / fidivr len` stored to a
  float; the two clips' offsets are summed and stored.
- `VectorAngleMultiply` (0x819558e): `yaw * pi / 180` (the double at
  0x820a288) stored, `sinf` stored, `cosf` kept as returned, each new
  axis summed on the stack.
- `Mantle_FindTransition`: the rise `ledge.z - start.z` kept, each row's
  distance from it stored, the first row on a tie (`fucomi`, `ja`).
- `XAnim_CalcDeltaForTime`, under 256 frames: the frame position `t *
  numframes` kept, its whole part `(int)` of the stored float; the key
  pair around it found in the frame indices; the fraction `(pos - k0) /
  (k1 - k0)` kept; each quantized axis lerped by it and stored; then
  `size * lerp + mins`, the product kept. At `t == 1` the last key is
  scaled the same way, and a one-key track is its `frame0`.
- `XAnimGetLengthMsec`: `numframes / framerate * 1000` kept, stored,
  truncated (`cvttss2si`).
- `Mantle_CapView` calls the out-of-line `AngleDelta` (0x8190590) and
  `AngleNormalize360` (0x8190540), which multiply by the float 1/360
  (0x8209630), keep the turns and return on the stack; `floor` is
  `frndint` under the round-down control word.

`assets::model::DeltaPart` keeps the quantized track (`trans_quant`,
`trans_mins`, `trans_size`) for the lerp in that order.

### 17.3 The A/B

`play-1.7-mantle.txt` (its recipe in its header): a probe teleported by
`setviewpos` to the -x face of mp_crash's low wall at x -16..36, y
-1376 (brush 2242, `MANTLE_OVER`, found by `cargo run -p vcod-cod4
--example mantle_spots`) faces +x, walks into it and taps jump, swinging
its view 120 left over the mantle and back; then faces -x and vaults
back; then leans. Each mantle is row 4 (up 39, over mid), flags 5 (over
and stand), 900 ms: `mantleState.timer` reads 16 (the second time 32)
on the first snapshot and 900 on the last, `eFlags` 0x8002 through it,
`EV_PUTAWAY_WEAPON` at the start, `EV_STANCE_FORCE_STAND` at the end.
VERIFIED live. `cod4_moves_ab` replays it from the first snapshot
after the teleport landed (47100): every mantle row's origin, velocity
and `mantleState` words are exact, and the client's prediction of it
corrects at most 0.0005 units (`mantle_capture_predicts_without_a_correction`).
VERIFIED (the gates).

GAPS:

- The yaw cap, closed: on the 11 rows where `Mantle_CapView` pushed
  `delta_angles[1]` (a negative yaw), ours went out one short under
  retail's, and a constant 0.0031..0.0067 degrees added to the push made
  all 11 exact. That was the wire's half-short rounding (section 8), not
  the cap: compared as the writer sends them, every mantle row's
  `delta_angles` is exact, and the 19 and 6 rows of the origin's x and
  y the gate listed went with it, the free run starting from the fitted
  float. VERIFIED (the gate).
- The walk off over the terrain, a bit of the origin's height (7 cells):
  section 13.2's float model. INFERRED.

### 17.4 The blockage

`G_AddPlayerMantleBlockage` is inlined in `ClientThink_real` after a
pmove whose `mantleStarted` is set (`cod4_lnxded` 0x80a6537..0x80a6618):
`G_Spawn` (0x80cbbe8), `parent` and `r.ownerNum` the client (0x808af6e
twice), `r.contents` and `clipmask` 0x10000 (ent+0x120, +0x190),
`r.svFlags` 0x21 (+0xf6), `eType` 5, `ET_INVISIBLE` (+0x4), handler 0x15
(+0x16e), `r.mins` and `r.maxs` the client's (+0x108, +0x114),
`G_SetOrigin` at `mantleEndPos`, `SV_LinkEntity`, and `nextthink` the
level time plus `mantleDuration` plus `g_mantleBlockTimeBuffer`'s integer
(0x84bc2c0). VERIFIED (the disassembly). The dvar is an int, 500, 0 to
60000, flags 0x80. VERIFIED (`tools/re/cod4_dvars.py`). The client's box
is the one the last cmd left: `ClientThink_real` copies `pm.mins` into
it after the blockage. INFERRED. The handler's think frees the
entity (`ENT_HANDLER_PLAYER_BLOCK`). INFERRED.

vcod-server spawns it off `Pmove::mantle_started`
(`Cod4Level::spawn_mantle_block`): an entity with no classname, which no
snapshot carries, and a box of contents 0x10000 in every other player's
pmove (`Body4` with an owner; `SV_Trace` skips an entity its pass entity
owns), freed in the frame's entity pass once its time comes. It takes an
entity number as retail's does. No gate replays a second player against a
mantle yet.

### 17.5 The anims

- `Mantle_Move` calls `BG_AnimScriptAnimation` (0x80502ea) every frame
  after the two `Mantle_GetAnimDelta` calls (0x80515ef) with state 0, the
  movetype `s_mantleTrans`' up clip slot plus 0x15, or its over clip's
  (0x820aa24) when the up clip's length is under the timer, and force 1.
  `PM_Footsteps`' calls pass force 0 (0x805a570 and the rest). VERIFIED
  (the instructions). Slot plus 21 lands on `MANTLE_UP_57` ..
  `MANTLE_OVER_LOW`: `animMoveTypesStr` (0x826a8e0, 8-byte entries) has
  `MANTLE_ROOT` at 21. VERIFIED (the table). The stock script's blocks
  play `both mp_mantle_up_57` .. `33` and `over_high`/`over_mid`, `legs`
  for `up_27`, `up_21` and `over_low`, one command each. VERIFIED (the
  `common_mp` rawfile).
- At the end, after `pm_flags` 4 and `mantleStarted` clear and before
  the stand test, a mantle with `mantleState.flags` 1 runs
  `BG_AnimScriptEvent` (0x8050194) with event 3, not continuing, forced
  (0x8051716). Event 3 is `JUMP` (`animEventTypesStr`, 0x826aa40).
  VERIFIED (the instructions, the table).
- vcod: `pmove::anim::mantle` and the jump event in `mantle::mantle_move`.
  `cod4_moves_ab` replays the mantle capture with the anim script and
  compares `legsAnim`, `torsoAnim`, `legsTimer` and `torsoTimer` on every
  row: all exact, resynced and free. Before the change 8 `legsAnim` rows
  and 2 rows of each other word were wrong. The capture's up clip shows
  as `legsAnim` 0x23d (61 with the toggle), the over clip 57, then the
  jump's 0x29b (155, `pb_standjump_takeoff`) with `torsoAnim` 0x200 and
  both timers 39, then the land's 154. VERIFIED (the gate). The capture
  is the mantler's own playerstate; the body others are sent copies these
  words, which `cod4::wire_ab`'s walk gates (cod4-combat.md 11). No
  capture of a second client watching a mantle yet. INFERRED.

## 18. The lean

`PM_UpdateLean` is section 8's. The mantle capture leans left from
55600 and right from 57100 in the open: `leanf` steps out by `msec / 350
* 0.5` to -0.5 and back by `msec / 280 * 0.5`, and every lean row of it
replays exactly. VERIFIED (the gate). No capture leans into a wall yet,
so the box that holds a lean off one has no retail gate; `stance::tests`
pins the model's on a box world. The client's eye
(cod4-client-view.md 7) leans as 1.7's does.

## 19. The melee charge

A melee key press whose cmd carries `meleeChargeDist` lunges the body at
a target in reach. `vcod_cod4::pmove::charge` runs it on the server and in
the client's prediction.

- The start. `PM_MeleeChargeStart` (`cod4_lnxded` 0x80561e6) runs on the
  swing's press, from `PM_Weapon_CheckForMelee`'s arm (the call at
  0x8063465): a cmd distance of 0 clears the charge; any other sets
  `pm_flags` 0x20000 and copies the cmd's yaw (`ps` +0x5f0, float) and
  distance (+0x5f4) with the timer (+0x5f8) at 0. VERIFIED (the stores).
- The swing. `PM_Weapon_MeleeInit` (inlined at 0x806346a) takes the charge
  swing when the flag is set and the weapon has a charge anim
  (`meleeChargeAnim`, +0x70, non-empty) and `meleeChargeTime` (+0x384) above
  0: `weaponTime` = `meleeChargeTime`, `weaponDelay` = `meleeChargeDelay`
  (+0x364), weapon anim 9, and on a knife model the anim script event
  0x13 (`KNIFE_MELEE_CHARGE`). Otherwise `meleeTime`, `meleeDelay`,
  anim 8, and `knife_melee` or `meleeattack`. VERIFIED (the branches).
  The stock rifles in `common_mp` carry `viewmodel_M4m203_knife_melee_2`,
  charge delay 159 and time 1159 against the plain swing's 129 and 800.
  VERIFIED (the zone).
- The update. Inlined in `PmoveSingle` after `Mantle_ClearHint`
  (0x805bf55): the charge runs only for `pm_type` 0, `eFlags` & 0x300
  clear (no turret) and `pm_flags` & 0xc clear (not mantling, not on a
  ladder); anything else clears it. On its first frame (timer 0) the flat
  velocity becomes `sqrt(2 * dist * player_meleeChargeFriction)` along
  `YawVectors2D(yaw)` (0x81955e4: `sinf`/`cosf` of the yaw times the
  double `pi / 180` stored to a float) and the timer `(int)(float)(v /
  friction * 1000)`; the inline `fsqrt` keeps its extended result
  (0x805d471). Each frame takes its msec off; at 0 or below the charge
  clears. VERIFIED (0x805c2ef..0x805c354, 0x805d442..0x805d4bd).
- The friction. `PM_Friction` under the flag drops `friction * frametime`
  whatever the ground (0x8056fb8..0x8056fcb), so the lunge stops in
  `dist` units, in `v / friction` seconds. VERIFIED. The flag also forces
  `forwardmove` 127 (section 3).
- `player_meleeChargeFriction`: 1200, 1..5000, cheat (0x8054f1d stores
  the dvar; `cod4_dvars.py`). VERIFIED.

The client's pick (`iw3mp.exe` 0x402b60,
called from the aim assist's mouse step 0x402c50, which `CL_MouseMove`
calls at 0x463821 and whose outputs it stores into the cmd's yaw and
distance at 0x46382e..0x463845): with the melee key (0x4) down and not
prone, the first screen target within `aim_automelee_range` whose box
meets the middle region; at or past `player_meleeRange` squared
(0x402bc6..0x402bef) the cmd gets the yaw from the view origin to the
target's aim point (`vectoyaw`, 0x55fa70) and the distance truncated
(`fistp` under a 0xc00 control word, 0x402c1d..0x402c3e). VERIFIED (the
compares and stores). The dvars' defaults: `aim_automelee_range` 128
(0x401c63), `aim_automelee_region_width` 320 and `..height` 240 out of
640 by 480 (0x401b9f, 0x401c01), `player_meleeRange` 64 (0x40b801),
`aim_target_sentient_radius` 10 (0x4033b3). VERIFIED. What the target
lists hold is INFERRED: enemies (anyone without teams) whose `j_head` the
eye sees under mask 0x803003, at most the sentient radius behind the
view's plane, sorted nearest first; their boxes (the radius round,
origin to `j_helmet`) projected and sorted by the box centre's distance
from the crosshair; the aim point `j_spine4`; the distance the origin's
from the predicted origin, squared. `charge::client_pick` is that. vcod's
client feeds it the newest snapshot's players with the stance's bind
heights for `j_head` and `j_helmet` and the origin for `j_spine4`
(`play::cod4::melee_charge`), at `cg_fov` 65 on 4:3, the world alone in
the sight trace and no smoke test (`FX_GetClientVisibility`). INFERRED:
no capture of `iw3mp.exe`'s own cmds measures the pick.

The A/B. `play-1.7-lunge.txt` is the attacker of a retail run (recipe in
its header): teleported 100 units south of a standing probe, it presses
the melee key with the pick's yaw 90 and distance 100 in the cmd. The
first snapshot after the press reads `pm_flags` 0x20000, weapon state 12,
`weapAnim` 0x209 (anim 9), `weaponDelay` 159, `weaponTime` 1159 and the
charge yaw and distance with the timer still 0 (the update runs before
`PM_Weapon` in the frame); the next 430 units/s along +y (489.9 less one
50 ms frame of friction) and the timer 358; the charge clears at 52700,
the swing having killed the target at 52450, the attacker 133 units on.
VERIFIED live. `cod4_moves_ab`'s `lunge` row replays it armed: 147
snapshots from 51250, origins exact in the free run (largest error 0.000)
and 146 of 147 rows exact resynced; the one row left is the server's
(`EV_MELEE_BLOOD` on the attacker and the ammo off the victim's dropped
rifle, in `GAPS`). VERIFIED (the gate). vcod-server and the client's
prediction run the same module: `cod4_live_the_melee_key_lunges_at_a_bot_in_reach`
lunges at a bot 100 units off through the client's own pick and lands
where the server put it. VERIFIED (the gate).

