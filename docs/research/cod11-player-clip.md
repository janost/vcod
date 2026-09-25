# CoD 1.1 MP: players clipping players

How a live player blocks another on a stock 1.1 MP server: which trace masks
see a player, what contents a player carries in each state, how the server's
trace clips one capsule against another, when the entity's box is linked and
how it is packed into the wire `solid`, how the client's prediction clips the
same players, and `StuckInClient`, the end-frame push that separates two
players found inside each other. Sections 9 and 10 read the retail captures;
section 11 is what vcod does and section 12 where it differs.

Evidence rules as everywhere in this directory. This document carries no
document-level default: every claim carries its own label. VERIFIED is a byte
read out of a module or a committed capture (an immediate, a relocation, a
string, a store at an offset, a fixture line). INFERRED is anything read off
control flow, which includes the order of two stores and every branch
condition, and anything that says what a field means.

Modules:

- `game.mp.i386.so`, the 1.1d Linux dedicated server's MP game module.
  Addresses are its own VAs as `nm -D`, `objdump` and
  `python3 tools/re/annotate_func.py <elf> <symbol|0xaddr>` print them; a
  Ghidra export at +0x10000 shows each one 0x10000 higher. The module is
  position-independent, so a call or a data reference is readable only with
  its relocation resolved, which `annotate_func.py` does. `PM_Jump` (0x2eb98),
  `PM_Friction` (0x2e460), `PM_WalkMove` (0x2f258) and `PM_DropTimers`
  (0x32a44) carry no dynamic symbol; the names describe what the bytes do.
- `cod_lnxded`, the 1.1d dedicated engine, stripped, absolute addresses.
- `cgame_mp_x86.dll` 1.1, image base 0x30000000.

Offsets used throughout, in the game module. `ent` is a gentity (stride
0x314), `cl = ent+0x158` its client, the playerstate at `cl+0`.

| offset | meaning | evidence |
|---|---|---|
| `ent+0x9c` | `s.solid` | VERIFIED, `SV_LinkEntity`'s stores at `cod_lnxded` 0x80908da, 0x80909f7 and 0x8090a00 |
| `ent+0xf4` | `r.svFlags`; 0x200 the capsule flag | VERIFIED, `ClientConnect` stores 0x200 there (0x4258f); the meaning is INFERRED from `CM_TempBoxModel` (section 2.1) |
| `ent+0xfc` | `r.bmodel` | VERIFIED, `SV_LinkEntity` tests it at 0x80908d1, beside the 0xffffff store; the name is INFERRED |
| `ent+0x108` / `+0x10c` / `+0x114` | `r.mins.z` / `r.maxs.x` / `r.maxs.z` | VERIFIED, the three loads in `SV_LinkEntity` (0x809094c, 0x8090900, 0x809099b) and `ClientThink_real`'s stores (0x40539-0x40569) |
| `ent+0x118` | `r.contents` | VERIFIED, `SV_LinkEntity`'s `testl $0x2000001` at 0x80908f0 and every store in section 1.2 |
| `ent+0x14c` | `r.ownerNum` | VERIFIED, read by the entity clip at `cod_lnxded` 0x8090f18 (section 2.1); the name is INFERRED (`cod11-turrets.md`) |
| `ent+0x190` | `clipmask` | VERIFIED, `ClientSpawn` stores 0x2810011 there (0x42732); the name is INFERRED |
| `ent+0x230` | `health` | VERIFIED, entity field table offset 560 (`cod11-turrets.md`) |
| `cl+0xc` | `ps.pm_flags`; byte `cl+0xd` bit 0x1 is 0x100, byte `cl+0xe` bit 0x4 is 0x40000 | VERIFIED, player netfield offset 12 |
| `cl+0x10` | `ps.pm_time` | VERIFIED, player netfield table |
| `cl+0x44` | `ps.speed` | VERIFIED, player netfield table |
| `cl+0x20d0` | `sessionState` | VERIFIED, `cod11-map-cycle.md`; the name is INFERRED |
| `cl+0x21d8` / `+0x21dc` | noclip / ufo | VERIFIED, `G_SetClientContents` compares both against 0 (0x41504, 0x4150d); the names are INFERRED |

Contents bits: `CONTENTS_BODY` 0x2000000, `CONTENTS_CORPSE` 0x4000000,
`CONTENTS_SOLID` 0x1.

## 1. Contents and masks

### 1.1 Tracemasks

VERIFIED: `ClientThink_real` (0x3fee0) stores 0x2810011 into `pm.tracemask`
at 0x400a4 and 0x810011 at 0x40098. INFERRED, off the `jle` at 0x40096: the
first for `pm_type <= 5`, the second above, so a live player's move is
stopped by bodies and a dead one's is not. VERIFIED: `ClientSpawn` stores
0x2810011 into `ent+0x190` (0x42732).

VERIFIED: the cgame's pmove setup stores 0x2810011 and 0x810011 into
`pm+0x34` (0x30029569-0x30029590) and masks 0x2000000 and 0x10000 out of it
at 0x3002959c. INFERRED: the first for `pm_type < 6`, the second above, and
the clear when `cg.snap`'s `pm_type` is 4, the spectator, so a predicting
spectator passes through players and playerclip.

VERIFIED: `BG_CheckProneValid` (0x2d428) traces with 0x810011 and 0x820011
(0x2d4c2, 0x2d4d2), neither holding 0x2000000; `PM_UpdateLean` (0x32ac8)
traces with 0x2810011 (0x32d07). INFERRED: the prone fit ignores players and
a lean is cut short by one.

Of the bits above 0x20000 only 0x800000 and 0x2000000 are in a mask, and no
stock material carries either (`bsp-ibsp59-format.md`); so against the map
the live and dead masks clip the same brushes. INFERRED, from that census.

### 1.2 `r.contents` by state

VERIFIED, each a store to `ent+0x118`:

| value | where | state, INFERRED from the branch that reaches it |
|---|---|---|
| 0x2000000 | `ClientEndFrame` 0x4100b | playing |
| 0 | `ClientEndFrame` 0x40fc9 / 0x40fe1 | noclip / ufo |
| 0 | `ClientEndFrame` 0x40ffc | `sessionState` 1, dead |
| 0 | `ClientEndFrame` 0x40ee0 | intermission |
| 0 | `SpectatorClientEndFrame` 0x4078f | spectator |
| 0x4000000 | `player_die` 0x49c28 | the death edge |
| 0x4000000 | `ClientEndFrame` 0x411b8 | stuck (section 6) |

VERIFIED: `G_SetClientContents` (0x414f8) compares noclip, ufo and
`sessionState` against 0, 0 and 1 (0x41504, 0x4150d, 0x41516) and stores 0
(0x4151f) or 0x2000000 (0x41530); `ClientSpawn` calls it (reloc 0x4274f).
INFERRED: it is the end frame's rule applied once at spawn.

INFERRED: `player_die`'s CORPSE is overwritten with 0 by the dead player's
next end frame, since the dead arm at 0x40ffc stores unconditionally. So a
freshly dead player is outside both masks for the rest of the frame it died
in, and outside every mask after it. A body-queue clone carries contents 0
(`cod11-combat.md` 5.2).

## 2. The server's trace

### 2.1 Entities in `SV_Trace`

VERIFIED: trap 35 (`trap_TraceCapsule`, the case at `cod_lnxded`
0x8088234) calls `SV_Trace` (0x80916f4) with its capsule argument 1
(`cod11-mantle.md`, "The player is a capsule"). VERIFIED: a non-point trace
walks the entities through 0x805a6d8 into the clip at 0x8090f18, which
compares `mask & contents`, the pass entity and `ent+0x14c` against the pass
entity and its owner, and holds no team compare. INFERRED: an entity is
skipped when the mask misses its contents, when it is the mover, or when it
is owned by the mover or by the mover's owner; teammates block each other.

VERIFIED: `CM_TempBoxModel` (0x804baf8) tests `svFlags & 0x200` and holds
the handle 0x1fe, and `ClientConnect` stores 0x200 into every client's
`svFlags` (0x4258f). INFERRED: the flag selects the capsule handle, and
0x8056310 routes 0x1fe to the capsule-through-capsule clip at 0x8055c04 when
the trace's capsule flag is set, so a player is traced as a capsule.

INFERRED, off `SV_Trace`'s branch on the world trace's fraction: the entity
pass is skipped when the world trace comes back at fraction 0, so a mover
already stopped by the map is never reported as hitting a player.

### 2.2 Capsule through capsule

The Minkowski sum of two upright capsules is an upright capsule with the
summed radius and the summed half heights, so the sweep is the mover's
centre against that capsule: a cylinder over the straight span and a sphere
at each end. INFERRED, the geometry of Q3's `CM_TraceCapsuleThroughCapsule`,
which the retail primitives below fill in.

The sphere primitive is `cod_lnxded` 0x8055794, the cylinder 0x8055980, the
dispatcher 0x8055c04. VERIFIED: the floats at 0x80cd424 and 0x80cd428 read
0.125 and the one at 0x80cd42c reads 0.5. INFERRED, from the instruction
sequence, for both primitives (the cylinder in the plane, the sphere in
three dimensions), with `rel` the start less the centre and `R` the summed
radius:

- a start with `|rel|^2 - R^2 <= 0` is startsolid, fraction 0, with the
  normal `rel`;
- a move with `rel . delta >= 0` (not closing) or a negative discriminant
  is not clipped at all;
- the hit fraction is the entry root less a 0.125 backoff along the start
  normal, `f = (-b - sqrt(b^2 - a c)) / a + 0.125 |rel| / b`, clamped to 0
  after the `< fraction` test;
- the reported normal is `rel` normalized at the start point, not at the
  contact;
- the cylinder is allsolid when the end lies inside its z-span, whatever
  its xy, and a hit counts only when the raw root's z lies inside that span.

That is not Q3's shape. Q3 pads the quadratic's radius by `RADIUS_EPSILON`
(1.0), skips a move whose segment clears `radius + SURFACE_CLIP_EPSILON`,
and takes the normal at the contact; its cylinder runs only on a move with
an xy delta. INFERRED, from Quake III Arena `code/qcommon/cm_trace.c`. The
bump capture decides between the two: section 9.1's 30.125 stop and section
9.2's 30.02 to 30.08 glances fit retail's backoff, and Q3's pad held a
glancing walker at 30.10 to 30.18 (section 11).

INFERRED: the 0.5 at 0x80cd42c is the dispatcher's half height. The
dispatcher picks which of the two spheres and the cylinder to trace from the
z relation of the two capsules; its branch conditions were not read in full.

### 2.3 A step does not climb a player

VERIFIED: `PM_StepSlideMove` (0x34fbc) compares the down trace's
`entityNum` (the u16 at trace+0x28, the 48-byte layout in `cod11-combat.md`
3.2) against 0x3f at 0x3533a. INFERRED, off the branch: when the down pass
lands on an entity below 64, a client, the move restores the plain slide's
origin and velocity and returns, with no revert test, no ground snap and no
step event. So a walker pressed into a prone player is not stepped onto its
top sphere. Section 9.1's prone head-on rows are the capture of it.

## 3. Bounds and the link

VERIFIED: `G_SetPlayerSize` (0x42fd4) reads `g_bounds_width` "30" and
`g_bounds_height_standing` "70". VERIFIED: `ClientSpawn` copies the player
bounds into `r.mins`/`r.maxs` (0x42775-0x427b7), and `ClientThink_real`
copies `pm.mins`/`pm.maxs` into them (0x40539-0x40569) and calls
`trap_LinkEntity` (0x40595). INFERRED: both follow its `Pmove`.
VERIFIED: the stance constants at 0x316f4 read a crouched `maxs.z` of 50
(0x70be0) and a prone one of 30 (0x70bdc).

INFERRED: the box starts at the feet (`mins.z` 0) and its top follows the
stance, and every cmd relinks the player once, after its whole `Pmove` chop
and ahead of `G_TouchTriggers` (0x405b3).

VERIFIED: `ClientSpawn` calls `ClientThink_real` and `ClientEndFrame`
(reloc 0x42a76). INFERRED: a spawn links the new body through that
`ClientThink_real`, and `StuckInClient` can run at spawn through that
`ClientEndFrame`.

## 4. The wire `solid`

### 4.1 Packing

VERIFIED, `SV_LinkEntity` (`cod_lnxded` 0x80908b0):

- it tests `r.bmodel` (0x80908d1) and stores 0xffffff into `s.solid`
  (0x80908da);
- it tests `r.contents` against 0x2000001 (0x80908f0) and stores 0 into
  `s.solid` (0x8090a00);
- it stores `zu << 16 | zd << 8 | x` (0x80909ed-0x80909f7), from `maxs.x`,
  `-mins.z` and `maxs.z + 32` (the float at 0x80d6648 reads 32.0), each
  converted by `fistp` under a control word with 0xc00 set (round toward
  zero) and compared against 1 and 255.

INFERRED, the branches: a brush model packs 0xffffff, an entity whose
contents hold neither 0x2000000 nor 0x1 packs 0, and anything else packs
the three bytes, each clamped to `[1, 255]`.

So a player's feet box, `mins.z` 0, packs `zd` 1: the clamp, not the box,
puts the byte there. A client decoding `mins.z = -zd` puts the box's floor
one unit below the feet (section 5).

VERIFIED, the bump capture (`mp_carentan-dm-bump-walker.txt`, first at
lines 27, 2644 and 4815): the target's `solid` reads 6684943 standing,
5374223 crouched and 4063503 prone. Those are `(maxs.z + 32) << 16 | 1 << 8
| 15` for `maxs.z` 70, 50 and 30.

### 4.2 When it is written

VERIFIED: `SV_LinkEntity` has two callers, the link trap and the
brush-model setter; `ClientEndFrame` makes no link call; `G_RunFrame` makes
one direct link (0x5082f), at a lower address than its end-frame loop
(0x50ab0-0x50ad7). INFERRED: that link runs ahead of the loop, so a player's
`solid` is packed at the link after each of its cmds and at spawn, never at
end frame, from the contents the last end frame left.

INFERRED consequences:

- a player `StuckInClient` marks CORPSE at end frame keeps its packed
  `solid` on that frame's snapshot and reads 0 from its next cmd's link;
  section 10.1 shows 0 on the snapshot after each push;
- a freshly dead player's `solid` goes 0 at its next cmd, since CORPSE and
  then 0 are both outside 0x2000001;
- a client that sends no cmds is never relinked, so its `solid` stays what
  its last link wrote.

## 5. The client's clip

VERIFIED, the cgame: `CG_BuildSolidList` (0x30028d50) tests
`nextState.solid` and `eType` 3, has one caller, `CG_SetNextSnap`, and walks
`cg.nextSnap`. INFERRED: the solid list is every entity of the newest
snapshot with a non-zero `solid`, items excepted.

VERIFIED: `CG_ClipMoveToEntities` (0x30028df0) decodes the box as `mins =
(-x, -x, -zd)`, `maxs = (x, x, zu - 32)` (the `- 32` at 0x30028eaa), holds
the contents 0x2000000 and 0x1 beside an `eType` 1 test (0x30028eda), and
tests `eFlags & 0x10` (0x30028ee7). VERIFIED: the pmove trace slot
(0x300290b0) passes the capsule argument 1. VERIFIED: retail's player
entities carry `eFlags` 16 (`docs/protocol-1.1.md`, "What a player entity
looks like"). INFERRED: `eType` 1 clips as BODY and every other type as
SOLID, the flag selects a capsule model, an entity whose contents the mask
misses is skipped, and a player clips as a capsule one unit deeper than the
server's.

VERIFIED: the entity's origin comes from `centity+0x1f8`. INFERRED: that is
`lerpOrigin`. VERIFIED, the call sites in `CG_DrawActiveFrame`:
`CG_ProcessSnapshots`, `CG_PredictPlayerState`, `CG_AddPacketEntities`, whose
`CG_CalcEntityLerpPositions` recomputes `lerpOrigin`. INFERRED, from that
order: prediction clips each player where the previous render frame drew it.

INFERRED: the `pm_type` 4 clear in section 1.1 is the only place the cgame
drops players from its own mask.

## 6. StuckInClient

### 6.1 Where it runs

VERIFIED: `G_KillBox` (0x66bf8) has no caller in the module. VERIFIED:
`ClientEndFrame` (0x40e98) compares `ent+0x230` against 0 (0x4119c), calls
`StuckInClient` (0x40934) at 0x411a9, and stores 0x4000000 into
`r.contents` at 0x411b8. INFERRED: for a player with health above 0, a true
return marks that player, and only that one, CORPSE. VERIFIED: `G_RunFrame`
calls `ClientEndFrame` at 0x50abe inside a loop whose bound is `level+0x1e0`
(0x50ab0-0x50ad7). INFERRED: that bound is `maxclients` and the end frames
run in slot order.

### 6.2 The guard and the scan

VERIFIED: `pm_flags` 0x40000 is set in one place, `ClientEndFrame` 0x40fb0,
and cleared at 0x40f11 (the intermission arm), in `IntermissionClientEndFrame`
0x416ae and in `SpectatorClientEndFrame` 0x407b7 and 0x408e0; bg pmove never
writes it. INFERRED, from the branches around 0x40fb0: it is set for a
connected client in `sessionState` 0 or 1, playing or dead, so it reads "this
client owns its view", and it is clear on a spectator, an intermission
client and a client still connecting. VERIFIED: every `!snap` line of the three
bump walker fixtures carries 0x40000 in `pm_flags` (sections 9 and 10).

`StuckInClient` (0x40934). VERIFIED: it tests byte `cl+0xe` bit 0x4 on self
(0x40949) and compares contents against 0x4000000 (0x4096d); in the loop it
tests the same bit on the other slot (0x409b3) and branches to the function's
exit at 0x409b7. INFERRED, the control flow:

- self must carry 0x40000, `sessionState` 0 and contents exactly BODY or
  exactly CORPSE;
- the scan walks every slot `0..maxclients` in order, lower and higher alike,
  and skips a slot not in use;
- a slot in use without 0x40000 returns 0 for the whole scan;
- it skips self, a slot with `sessionState` not 0, health at or below 0, or
  contents neither BODY nor CORPSE.

INFERRED: so one spectator, one intermission client or one client still
connecting, in any slot, turns `StuckInClient` off for every player on the
server. Section 10.3 is the capture.

### 6.3 Overlap

INFERRED, from the compares: an inclusive overlap of the two absolute boxes
on x, y and z, then, when both are capsules, `dx^2 + dy^2 <= (self.maxs.x +
other.maxs.x)^2`. So the test is a vertical cylinder of radius 30 over the
two boxes' z spans, with no rounding at the ends, and two players exactly 30
apart, or one standing exactly on another's box top, count as stuck.

### 6.4 The push

VERIFIED: the float at 0x72d30 reads -2^-31 and the one at 0x72d34 reads
1e-4. INFERRED, from 0x40cc3-0x40e58:

- `dir = (other.xy - self.xy) + (j1, j2)`, each `j = -1 - rand() / 2^30`,
  which puts both in (-3, -1]; the constant reads as an overflowed
  `RAND_MAX + 1`. Then a 2D normalize (`VectorNormalize2D`, 0x40bc3);
- each player's speed is `ps.speed` when its horizontal velocity is non-zero
  and 0 otherwise; when both come out below 1e-4, both take `ps.speed`;
- `other.velocity.xy = dir * s_other` (0x40dfb, 0x40e0b) and
  `self.velocity.xy = -dir * s_self`, z untouched;
- both take `pm_time` 300 and `pm_flags |= 0x100`;
- it returns 1 at the first overlap found.

VERIFIED: `ClientEndFrame` stores `ps.speed` at 0x41100. INFERRED: the
store runs ahead of the `StuckInClient` call and writes `(int)g_speed`.
VERIFIED: every `!snap` line of the captures here reads `speed=190`.

INFERRED: both players of a pair run the test in the same end frame, the
lower slot first. The lower slot marks itself CORPSE and pushes; the higher
slot then finds the lower one, which CORPSE still admits, and pushes again,
so the higher slot's write is the one left standing. Section 10.1 reads
that off the wire.

## 7. The knockback timer

VERIFIED, in retail's pmove: `PM_WalkMove` tests `pm_flags` 0x100 and
scales the acceleration by 0.25 (0x2f4d8); `PM_Friction` tests the same bit
and multiplies the ground term's control by 0.3 (0x2e51c); `PM_DropTimers`
(0x32a44) masks 0x100, 0x200 and 0x2000 out of `pm_flags`, stores 0 into
`pm_time` and subtracts `msec` from it. INFERRED, the branches: both scales
apply while the bit is set, and the drop clears the bits and the timer once
`msec >= pm_time` and otherwise counts it down.

INFERRED: `PM_DropTimers` runs from `PmoveSingle` for every `pm_type`, so a
pushed player that is linked, mounted or dead still counts the timer down.
VERIFIED, the still overlap (`mp_carentan-dm-bump-overlap-script.txt` lines
355-362, the target's own lines): `pm_time` reads 300, 300, 250, 200, 150,
99, 49 and 0 on consecutive snapshots, with `pm_flags` 262400 (0x40100)
until the 0.

## 8. The landing stun, open

VERIFIED: 0x300af stores into `ps+0x10` (`pm_time`) and 0x300b4 sets bit 0x1
of byte `ps+0xd` (`pm_flags` 0x100), and the string
`landing stun time: %i speed mult: %.2f` sits at 0x709e0. INFERRED: a hard
landing starts the same knockback timer as the push. Not read further and
not modelled (section 12).

## 9. What the bump capture measured

The capture is two committed files from one run on 2026-09-25:
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-bump-walker.txt`
(the `--save-bump` walker's side, "walker line N" below) and
`mp_carentan-dm-bump-script.txt` beside it (the server's `PROBE` lines). The
recipe is in the header and in `client-probes/README.md`, `probe_bump`.
Retail names the gametype `probe_bump`; the files are named `dm` because
the probe runs `dm::main`. Client 0 is the `--probe-bump-target` axis
player on a flat brush floor at (1132, -376, -151.875); client 1 the walker,
placed 200 units behind it along -x, both facing +x (`PROBE place` lines).
The target stands, crouches and lies prone in turn; the walker runs the same
script against each stance: a head-on walk, a glance 20 units right of the
line, and a jump from rest 40 units from the target's centre, forward and up
held until it lands. The jump runs in all three stances because a lowered
target is the only one a jump could clear.

Distances below are the walker's origin to the target's on the floor plane.

### 9.1 Head-on

VERIFIED, the last row of each head-on phase:

| target | distance | walker line |
|---|---|---|
| stand | 30.1255 | 357 |
| crouch | 30.1257 | 2889 |
| prone | 30.1256 | 5083 |

VERIFIED: the crouch and prone walks drifted a few units sideways while
pressing in (y -380.05 and -370.45 on those rows), at the same distance.
INFERRED: the stop is the summed radius 30 plus the 0.125 backoff of
section 2.2 in every stance, and stance changes nothing in xy.

VERIFIED: the prone head-on phase reads `groundEntityNum` 1022 and z
-151.875 on every row (walker lines 4921-5085). INFERRED: a walk into a
prone player's 30-unit capsule does not step onto it, which is section 2.3.

### 9.2 Glance

VERIFIED, the closest row of each glance phase, 20 units off the line:

| target | closest | walker line |
|---|---|---|
| stand | 30.0275 | 650 |
| crouch | 30.0763 | 3190 |
| prone | 30.0182 | 5380 |

VERIFIED: no glance row carries a non-zero `pm_time`. INFERRED: a glancing
walker slides round the target inside the 0.125 band without a push,
because the backoff runs along the start normal rather than padding the
radius.

### 9.3 Jumps: no landing on a head

VERIFIED: no row of any jump or land phase reads the target's entity number
as `groundEntityNum`; each lands on 1022, and the probe wrote a `# BROKEN
<stance>/jump landed on 1022, not on the target` note per stance.

- Standing target, VERIFIED: the walker rose beside it and came down at
  30.1254 (walker line 1125).
- Crouched and prone targets, VERIFIED: at `commandTime` 70633 (walker line
  3649, 29.06 from the target, 32.9 above its feet) and 97483 (line 5839,
  26.67 from it), `pm_time` reads 300 and the velocity (-189.90, 6.31) and
  (-189.50, 13.84), 190 in the plane, with `pm_flags` 262408 (0x40108); the
  target's `solid` reads 0 on the next snapshot (lines 3653, 5843). The
  walker landed 64.2 and 53.3 from the target (lines 3665, 5852).

INFERRED: once the walker's rounded bottom passed over the lowered target's
rounded top at under 30 in the plane, `StuckInClient`'s cylinder test of
section 6.3, which ignores the rounding, read the two as overlapping and
threw the walker off before it could land. A player at rest on a head would
be inside that cylinder too, since the z test is inclusive, so retail
pushes it off rather than letting it stand there.

### 9.4 The replay against the capture

`crates/server/tests/bump_ab.rs` replays the walker's cmds through our
pmove with the target as a body, free-running per phase and rebased on
retail's state at every snapshot, the `playerstate_slope_ab.rs` shape.
`BUMP_REPORT=1` prints every row and `BUMP_TRACE=<ct>` prints ours cmd by
cmd into that clock.

VERIFIED, as run on 2026-09-25: ours stops at 30.1255, 30.1257 and 30.1256
head-on and passes at 30.0276, 30.0764 and 30.0183 on the glance, and 1484
rebased rows outside the gate's `GAPS` match retail to 0.000 in both z and
xy with no ground disagreement.

`GAPS` holds nine rows, all from a jump, each with the most it may miss by.
VERIFIED, the measured size of each:

| kind | rows (phase, `commandTime`) | measured |
|---|---|---|
| `TAKEOFF` | stand/jump 39083, crouch/jump 70232, prone/jump 97033 | dz 0.554, 0.537, 0.554 |
| `AIR_STEP` | stand/jump 39483, 39533 | dxy 0.093, 0.103 |
| `REJUMP` | stand/jump 39682, stand/land 39732, crouch/land 70882, prone/land 97666 | dz 3.754, 10.664, 10.667 (dxy 4.12), 7.473 (dxy 2.78) |

What each is, and why none is a clipping difference, is section 12.

## 10. What the overlap captures measured

Two more runs of the same pair, with `+set probe_overlap 1`: the gsc
`setorigin`s the target onto the walker 12 s and 30 s after both are placed.
The walker stands still through the first and walks +x through the second.
The files are `mp_carentan-dm-bump-overlap-walker.txt` and
`-overlap-script.txt`, and for the third run, with a lone spectator
connected first, `-overlap-spectator-walker.txt` and
`-overlap-spectator-script.txt`. The target's own `BUMPT !snap` lines, its
side of each push, are appended to the script files as comments.

### 10.1 Both still

VERIFIED (`PROBE overlap 34900 0 1`, overlap-script line 65; overlap-walker
line 995; overlap-script line 355): on the first snapshot after the overlap
the walker reads velocity (126.06, 142.16) and the target (-126.06,
-142.16), both 190, both `pm_time` 300 and `pm_flags` 262400 (0x40100). The
`setorigin`'d target reads z -150.875, one unit above the floor.

VERIFIED: the target's `solid` reads 0 on the next two snapshots
(overlap-walker lines 999 and 1003) and 6684943 otherwise.

INFERRED: the walker's direction (0.663, 0.748) is the negated jitter of
section 6.4 over a zero separation, so the walker, slot 1, wrote last as
"self", and the target, slot 0, took `dir * s` as "other". The walker's own
CORPSE mark is not on this wire: no probe logged the walker's entity.

### 10.2 One walking

VERIFIED (`PROBE overlap 52900 0 1`, overlap-script line 228;
overlap-walker line 2455; overlap-script lines 366-374): the walking walker
reads 190 along (79.06, 172.77), the still target velocity (-0, -0) with
`pm_time` 300 and 0x100. `pm_time` holds at 300 on four snapshots and the
target's `solid` reads 0 on four (overlap-walker lines 2459-2471), then the
pair comes apart.

INFERRED: a player with no horizontal speed takes 0 unless both have none,
and `dir * 0` with a negative `dir` is the `-0` on the wire. The push fired
on four consecutive end frames while the still target stayed inside.

### 10.3 A spectator in slot 0

VERIFIED: the spectator run's `PROBE place` lines number the target 1 and
the walker 2 (overlap-spectator-script lines 22-23); slot 0 is the plain
`--net-probe`, which never answers the team menu. VERIFIED: across all 681
snapshots of `-overlap-spectator-walker.txt`, `pm_time` reads 0,
`target_solid` reads
6684943, and the script file carries no `BUMPT !snap` line. VERIFIED: the
walker's origin reads 932,-376,-151.875 on 679 of them, the target's
`pos.trBase` 932,-376,-150 on 440, forward cmds from 29.2 s included.

INFERRED: slot 0 held no 0x40000, so every `StuckInClient` scan returned 0
at it (section 6.2). The pair stayed inside each other for the rest of the
run and neither could walk out: a move that starts inside a body and ends
inside its z-span is allsolid (section 2.2), so nothing but the push
separates two players.

## 11. As implemented

The shared layer is `crates/common/src/movetrace.rs`. `Body` is a player
box at its feet with its contents; `Body::pack_solid` and `Body::from_solid`
are section 4.1's packing and section 5's decode. `MoveWorld` is the map
plus a body list and a pass entity: `box_trace` runs the world trace,
returns it at fraction 0, and otherwise clips every body whose contents the
mask admits and that is not the pass entity, keeping the nearest hit and
reporting the body's entity number, so `groundEntityNum` can name a player.
The mask filters bodies only; the world half clips `MASK_PLAYERSOLID`'s
brushes whatever the mask, which section 1.1's census makes the same thing.
`point_contents` stays map-only. `MoveWorld::bare` has no bodies and is what
every caller without players uses.

The body clip is section 2.2's two primitives (`trace_cylinder`,
`trace_sphere`, `backed_off_root`) with `BODY_RADIUS_EPS` 0.125 as the
backoff. It runs the cylinder and both end spheres on every move and keeps
the nearest, rather than the dispatcher's pick. The first port was Q3's
shape: its radius pad held a glancing walker at 30.14, 30.18 and 30.10 and
its xy gate skipped the cylinder on a vertical move, which let a falling
walker sink into a standing body; and before section 2.3's rule the step
put a walker on a prone target's top sphere, 12.7 high. `bump_ab.rs` found
all three.

pmove (`crates/common/src/pmove.rs`) takes the mask at each call site:
0x2810011 (`MASK_PLAYERSOLID`) for a live move and a lean, 0x810011
(`MASK_DEADSOLID`) for the dead move and the prone fit. `step_slide_move`
keeps the plain slide when its down pass meets an entity below
`MAX_CLIENTS`, section 2.3. The knockback timer is `PlayerState.knockback_ms`
with `PMF_TIME_KNOCKBACK`, section 7's two scales and its drop on every path;
it travels as `pm_time` with `pm_flags` 0x100 both ways.

The server (`crates/server/src/spectate.rs`, `server.rs`,
`game/stuck.rs`):

- `ClientSim.contents` is `r.contents`: BODY at a spawn into play, 0 at any
  other spawn, rewritten by `update_contents` at each end frame (BODY while
  playing and alive, else 0), and CORPSE at the death edge (`die`, reached
  from `take_damage`'s fatal arm and `mirror_vitals`).
- `ClientSim.linked_solid` is packed by `relink`, which runs at every spawn
  and at the end of each `step` (the pmove, dead and spectator paths), never
  at end frame; `to_entity` writes it as `solid`.
- `replay_moves` builds the body list from every sim before its loop, hands
  each chopped step `MoveWorld::new(world, &bodies, slot)`, and rewrites the
  mover's own entry after each step, so a later slot moves against an
  earlier slot's new position within the tick. The list is carried into the
  use key's second round.
- The end-frame loop walks the slots in order. Per slot it runs
  `update_contents` and then, for a live sim, `stuck_in_client` over views
  rebuilt at that slot, so a partner marked CORPSE earlier in the loop is
  seen with it. A push writes both velocities and `knockback_ms` 300 on both
  and marks self CORPSE; nothing relinks there. The jitter's `rand()` is the
  top 31 bits of the server's xorshift.

The client (`crates/client/src/play/predict.rs`, `main.rs`): `solid_bodies`
builds the body list from the newest snapshot, skipping the own client,
`solid` 0, `solid` 0xffffff (a brush model, already in the map) and `eType`
3, and logging and skipping a solid entity without `eFlags` 0x10. Contents
are BODY for `eType` 1 and 0x1 otherwise. The origin is where the renderer
drew the entity on the previous frame (`LivePhase.drawn_pos`), else its
`trBase`. The predictor keeps its last replay only while both the snapshot
playerstate and the body list are unchanged; any body that moved costs a
full replay that frame.

The gates:

- `movetrace.rs`'s unit tests pin the packing, the decode, the 30.125 stop,
  the bare-radius pass, the startsolid and allsolid rules and the fraction-0
  world rule;
- `crates/server/tests/player_clip.rs` runs two clients on one server: the
  30-unit stop, dead and spectating players not blocking, the three stance
  `solid`s, the stuck `solid` reading 0 one cmd late, a pushed pair coming
  apart, and a spectator in slot 0 disabling the push;
- `crates/server/tests/bump_ab.rs`, section 9.4;
- `crates/server/tests/stuck_ab.rs` reads both overlap captures and holds
  retail and ours to the same properties: `pm_time` in (250, 300] and 0x100
  on both on the first frame, 190 on a pushed player, the direction inside
  the jitter band with the higher slot writing last, the target's `solid` 0
  exactly on the snapshots after a push, the push-frame counts (2 still, 4
  walking), and no push beside a spectator. It also runs `stuck_in_client`
  at every snapshot of the bump capture and it fires at exactly
  `commandTime` 70633 and 97483, retail's two. `STUCK_REPORT=1` prints the
  first frames of each overlap;
- `crates/server/tests/predict_ab.rs`'s
  `server_and_predictor_agree_beside_a_body` steps the server's sim and the
  predictor side by side beside a standing body.

## 12. Divergences and not modelled

- **The jump port.** The three kinds of row in `GAPS` (section 9.4) are
  pmove differences a jump into the target exposed, none a clipping one. One
  follow-up closes all three: port `PM_Jump` with `PM_StepSlideMove`'s
  jump-step allowance.
  - `TAKEOFF`, dz 0.54 to 0.55 on each jump's first row. VERIFIED: the
    retail walker's first airborne row (walker line 1077) has risen 7.808
    from the floor with `pm_flags` 262152 (0x40008). INFERRED: that fits a
    takeoff at 249.8, `sqrt(g * 78)`, which is `PM_Jump` (0x2eb98) and its
    39/78 pair (`cod11-mantle.md`, "Jumps (there are two)"). Ours takes the
    stance-path jump at 233.2 with no 0x8.
  - `AIR_STEP`, dxy 0.093 and 0.103 at stand/jump 39483 and 39533. Ours
    steps the blocked airborne move over the standing target's shoulder and
    comes down 30.01 from it; retail holds the side at 30.1248. INFERRED:
    retail steps an airborne move only while below `fJumpOriginZ`, by at
    most `fJumpOriginZ - z` (the prologue at 0x350ec). The dispatcher's
    primitive choice (next item) is an alternative cause the capture does
    not rule out.
  - `REJUMP`, dz 3.754 to 10.667. VERIFIED: at stand/jump `commandTime`
    39682 (walker line 1125) the retail walker is on the ground with
    `pm_flags` 0x40008 and every cmd up to that clock holds up 127, and the
    next row (line 1131) is still on the ground. INFERRED: `PM_Jump` refuses
    while 0x8 is set. Ours jumps again off the landing.
- **The dispatcher's primitive choice** (`cod_lnxded` 0x8055c04) is not
  ported: ours traces the cylinder and both spheres and keeps the nearest.
  The two agree on every row of the bump capture; no capture covers a
  vertical landing on a body, since retail never let one happen (section
  9.3), and no gate covers geometry where the two could differ.
- **Arrival order.** VERIFIED: `SV_UserMove`'s per-cmd loop calls
  0x8092158 at 0x80872cb with the pointer at 0x80e30c4, 7 and the client's
  slot index. INFERRED: that is the game VM's `ClientThink`, run per cmd as
  each client's packet is parsed, so clients' moves interleave in packet
  arrival order and a later packet moves against an earlier one's new
  position. `replay_moves` runs each client's queued cmds in slot order
  instead. Who moved first decides
  who stops against whom when two players close on each other inside one
  frame. `docs/protocol-1.1.md` lists it under "What vcod's server does that
  retail does not".
- **A `trigger_hurt` kill during the move pass.** It runs the damage
  callback inside that cmd's touch pass, but the sim's `dead`, and with it
  the CORPSE contents, lands only at `mirror_vitals` after the script frame,
  so the victim still blocks the higher slots' moves in that tick.
  VERIFIED: retail's `player_die` writes CORPSE at 0x49c28. INFERRED: it
  does so at the kill, so a retail victim stops blocking within the frame.
  A stock map's only `trigger_hurt` is the kill volume under the floor.
- **The landing stun** (section 8) is not modelled. A follow-up with the
  jump port.
- **A client that sends no cmds** keeps the `solid` of its last link. That
  is retail's rule too (section 4.2), not a divergence; the same holds for
  a client at intermission, whose contents go 0 without a relink.
- **The cgame's clear at `pm_type` 4** (section 1.1) is not coded. It is
  moot here: a vcod spectator noclips, runs no trace and never predicts.
- **The packing clamp.** VERIFIED: retail clamps all three bytes to `[1,
  255]` (section 4.1). `Body::pack_solid` clamps `zd` to `[1, 255]` and `zu`
  to `[0, 255]` and passes `x` through. No player box reaches the
  difference.
- **Noclip and ufo** have no contents arm: vcod-server has no such player
  state.
- **Other solid entities.** Clipping a player against anything but a capsule
  player is not modelled: the predictor logs and skips a solid entity
  without `eFlags` 0x10, and no mover pushes a player.
