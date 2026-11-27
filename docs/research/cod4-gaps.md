# CoD4 1.7: what the client still lacks in a normal match

What a 1.7 player sees or uses in a public match that vcod's CoD4 client
does not draw, play or do, or fakes, ranked. The player-facing summary is
`docs/features/cod4.md`. Sources: the open sections of the `cod4-*.md`
docs, the HUD menus of `common_mp` (`menu_dump`) against what `cod4_hud`
reads, `iw3mp.exe` 1.7 for the menus' owner draws, and the live harness
in `play::cod4` (a client joined to vcod-server's `war` on mp_crash).
First audit 2026-10-26 at 0f20b5ed; rechecked against the code
2026-10-30, edited as items close since. Open items lead; closed ones are
kept at the end. Sizes: S under an hour, M a few hours, L a day or more.

## 1. Open

Ranked by how much a player notices.

- Ragdolls run on the ODE port as 1.7 runs them, gated on two retail
  captures to within a fraction of a unit (cod4-player-anims.md 10.7);
  the base pose they are built in is the model's, not under the corpse's
  entity, and the bones the ragdoll does not set keep the death anim's
  last locals. L.
- Multichannel output: kira mixes stereo, so a 5.1 or 7.1 device gets
  the stereo mix on its front pair (cod4-sound.md 4.2, 5). The 139 alias
  rows whose curve no walked zone holds fall off straight. L.
- `iw3mp.exe` has neither played a vcod demo nor recorded one for vcod:
  no retail client runs here. The framing is gated both ways against
  the binary and a retail capture (cod4-demos.md 8). S once a 1.7 client
  runs under Wine.
- Small HUD fades: the compass's voice fade, drawn off `isTalking` but
  not checked against a retail capture with a talker (cod4-hud.md 15.9).
  S.
- Smaller open items, each in its doc: `winvoice_mic_mute`, `winvoice_save_voice` and the record level meter
  (cod4-voice.md 5).

## 2. Not gaps

- CoD4x servers, by choice for now. Most public servers run CoD4x, which parks a 1.7
  client and hands it to its own update backend; vcod refuses them at the
  challenge and hides them in the browser (cod4-handshake.md 13).
- Retail's `mpdata` is never read: a retail rank does not carry over.
  By choice.
- The stance indicator: 1.7's `stance` menu sits at -1000 -1000, off
  the screen, so retail draws none either. VERIFIED (`menu_dump`).
- Spot light effect elements draw as omni lights; no stock MP zone holds
  one (cod4-effects.md 9).
- Single Player's popup does nothing: vcod is multiplayer only.

## 3. Closed

- Physics FX models (cluster bombs, breakables, the watermelon's
  chunks) drew as plain effect elements. They now spawn as world 1
  bodies off `fx_randomTable`, as 1.7's 0x4a1ff0 spawns them, draw at
  their bodies' poses and free them when their lives end; the watermelon
  take's six chunks spawn to the bit and pass the trajectory gate
  (cod4-effects.md 9.1.1).
- Launched parts and dynamic entities ran on vcod's own impulse solver,
  with no impact sounds. Worlds 0 and 1 now run on the ODE port, 1.7's
  geoms, colliders, contact pass and step, matched step for step on four
  retail takes, and their impact sounds play; the solver and its mp_crash
  pot rule are gone (cod4-gsc.md 3.6, 3.6.7; cod4-world.md 13.7).
- Effect model elements did not draw, and whether CoD4's effect sizes
  are radii was open. They are (`iw3mp.exe` 0x496ef0), so sprites were
  right, but tails drew half 1.7's length; tails now run `2 * size[1]`,
  and model elements load out of the zones and draw at the visual
  state's scale (cod4-effects.md 9), on 1.7's axes (9.1). Their
  physics body (flag 0x8000000) was vcod's own path; it is world 1's
  (above, cod4-effects.md 9.1.1).

- A playerstate sent without origin (bit 0) kept the base's origin and
  velocity. It now takes them, the view, `bobCycle` and `movementDir`
  off the archive row for its `commandTime`, as `iw3mp.exe` 0x507280
  does (cod4-netfields.md 13).

- vcod's moves reported an origin of 1e6 at time 0, so no server left
  origin out for vcod, and vcod-server never left it out for anyone. The
  client now stamps its cmds `cl.serverTime` and sends each packet after
  the frame's prediction with the predicted origin and time, and
  vcod-server leaves origin out on a match as `cod4_lnxded` 0x813f586
  does (cod4-netfields.md 3, 13).

- `timedemo` wrote no csv. It now writes 1.7's
  `demos/timedemo_<map>_mode_<r_mode>.csv`, a row a frame
  (cod4-demos.md 5). 1.7's front end has no demo list to copy
  (cod4-demos.md 7).

- The clip graphic drew nothing for an alt weapon's kind (6) and the
  offhand highlight was not looked at. Kind 6 now draws the `altWeapon`'s
  clip, as 1.7's 0x422a40 does: `gl_m16_mp` shows the rifle's
  magazine. The highlight (owner draws 109 and 110) sits in no stock
  menu, so 1.7 draws none either; the offhand icon now takes the
  equipped offhand first, as 0x45a360 does (cod4-hud.md 9).

- `sv_clientSideBullets` 0 was not read: vcod's client ran its own
  rounds whatever the server said. vcod now reads it off the systeminfo
  and under 0 runs no round of its own, and every server bullet hit event
  rolls its tracer, plays its whizby and pushes the props and launched
  parts along its leg from the event's `lerp.u`, the world's origin on
  1.7, as 1.7 does; the tracer and whizby run on the server's flesh hits
  under 1 too (cod4-effects.md 12.4). Gated on a retail capture under 0.
- A launched part's or dynamic entity's contact pass ran once a step on
  all of its contacts and never cut five or more; its world stepped at
  17 ms whatever the count awake, and the post-step sweep ran after each
  step. It now runs per geom with 0x5a2320's cut, a jitter pushes on the
  contacts the pass keeps, each world steps its bodies together at the
  `msecStep` its count awake picks (0x5a6738), and the sweep runs once a
  frame (cod4-gsc.md 3.6, 3.6.1; cod4-world.md 13.8).

- A body pinched between two facing surfaces kept both sides' contacts.
  It now takes `iw3mp.exe`'s contact pass (0x5a2760, 0x5a25d0): of two
  facing contacts the one whose plane the centre lies in front of stays,
  and each contact the body's point is leaving goes. Picture frames
  wedged between two walls on mp_crash's floor now rest with an end just
  inside the far wall, as the rule lets them (cod4-gsc.md 3.6.1).
- A cylinder geom (stock bottles, cans, pails, tires, hub caps) met the
  world on 16 points round its rims: dropped on mp_crash's steps, 480 of
  540 slept with world solid inside the cylinder. It now meets brushes
  and triangles through `iw3mp.exe`'s cylinder colliders (0x5a1330,
  0x5a17c0, 0x5a0f70) and none does. vcod's centre guard, which cut a
  body's speed on a hit, gave way to 1.7's post-step sweep (0x5a6930),
  which keeps it but on a stop under a fraction of 0.001 (cod4-gsc.md
  3.6.1).
- A launched part met the world on its 26 box points, so a world edge
  between two of them poked into it: on mp_backlot's steps 30 of 36
  dropped hoods and doors slept with world solid inside the box. Its
  boxes now meet the brushes and triangles round them, gathered as
  `iw3mp.exe`'s world collider (0x5a8110) gathers them, and none does
  (cod4-gsc.md 3.6.1). A map's dynamic entities meet it the same way,
  with 1.7's narrow-box pass: on mp_crash's steps 167 of 288 dropped
  props slept with world solid inside the box on the points, none now.
- The view weapon's anims summed through CoD1's layered sampler, the sight
  laid under the main clip. It now runs 1.7's tree, root over 32 leaves,
  with `iw3mp.exe`'s goal weights, blends, restarts and clock, sampled
  through 1.7's walk (cod4-player-anims.md 12).
- The client composed a skeleton with every result a float; `iw3mp.exe`
  runs the x87 at double precision. It now keeps doubles between the
  client's own stores (cod4-player-anims.md 8).
- Drawn players and corpses summed their clips through CoD1's sampler,
  which normalizes each key and the blend. They now take 1.7's walk as
  `iw3mp.exe` runs it, its stores included, the server's own tree
  matching 280 gdb trees word for word through the same function
  (cod4-player-anims.md 11).
- A round did not push a flying car part, and the props missed the
  knife's swing, `physicsJitter`, and every leg of a piercing round past
  the first; a round into the open sky pushed what it passed. vcod now
  pushes the part off the round's first contact (0x48df10), runs the
  swing's five rays (0x48e3a0) and the jitter (0x48eb50), and hands the
  props the legs 1.7's walk does (cod4-gsc.md 3.6, cod4-world.md
  13.4, 13.8, 13.9). The server's bullet events push nothing under
  `sv_clientSideBullets` 1, on 1.7 too (13.10).

- A map's dynamic entities (mp_crash holds 516) were neither drawn nor
  simulated. vcod now draws them, pushes them with blasts and rounds on
  its rigid-body solver, breaks the destructible ones into their effect,
  and sets them back on a map restart and on a `fast_restart`'s `n` or
  `B`, as 1.7's client does with nothing more on the wire (cod4-world.md
  13).
- The crosshair name and the friendly names over heads were not drawn.
  vcod now draws both as 1.7 does: over the posed `j_head`, sized by
  distance, in the relative team colour with the rank's icon and level,
  under the crosshair and friendly fades (cod4-hud.md 17.1).
- The crosshair never took the team colours live: 1.7's client sets
  `weapFlags` 8 and 0x10 off its own scan each frame, which vcod now runs,
  with the `cg_*NameFadeIn` wait before the colour (cod4-hud.md 17).
- The `winvoice_*` dvars were not registered and the voice page's record
  level slider sat greyed; they register at 1.7's defaults and the
  capture applies the level and the scaler (cod4-voice.md 4, 5).
- The CD key menu opened with greyed fields and no `getCDKey` or
  `verifyCDKey`; it now shows, checks and keeps the key as 1.7's does,
  minus the registry (cod4-front-end.md 7).
- The prediction clipped other players as boxes where vcod-server clips
  capsules, so walking into a player at an angle corrected by up to 20
  units. It now clips 1.7's client capsule, a unit inside the server's
  (cod4-client-view.md 5.1, 5.3).
- A dropped weapon's camo: vcod-server's item sends `weapon + 128 *
  model` off the dropper's `weaponmodels[]`, a pickup gives `index >> 7`,
  and the client draws `worldModel[index >> 7]` (cod4-client-view.md
  4.7).
- Other players' camo: vcod-server sets the entity's `weaponModel` off
  `weaponmodels[weapon]` and the client arms the body with
  `worldModel[weaponModel]` (cod4-client-view.md 4.7).
- The Create a Class editor: `statsetusingtable`, `statClearBitMask`,
  `statClearPerkNew`, `statset`'s item check, `setFromLocString` and
  `uploadstats` run as 1.7's do, the picks reach `vcod_mpdata` and a
  join's `custom1`..`5` spawn with them (`play::cod4_menus::cac`,
  cod4-front-end.md 4.1).
- Demo framing: a retail capture written as `iw3mp.exe` writes a demo
  plays in vcod, vcod's recorder writes the same bytes from the same
  messages, and a slot goes out as `MSG_WriteBigString` writes it; the
  `RECORDING` line draws at 1.7's place in `consoleFont`
  (cod4-demos.md 8, 9).
- The view model: the camo's `gunModel[]` slot off `weaponmodels[]`
  (vcod-server's `giveWeapon` sets it), the RPG's rocket at `tag_clip`
  while loaded, Sleight of Hand and Double Tap dividing the clips' rates;
  1.7 MP draws no goggles model (`cod4_viewmodel`, cod4-client-view.md
  4.7).
- Distortion (heat haze, muzzle flash rings) bends a copy of the frame:
  `fs_distort`, `scene_copy` (cod4-effects.md 17).
- Spot light shadow maps, up to `sm_maxLights` with retail's fade
  history: `cod4_spotshadow` (cod4-world.md 3.5).
- Client-side bullets: the own shot's seeded spread, rounds stopping on
  other players' boxes (the flesh hit left to the server's event) and on
  script models' collision, penetration with exit impacts, another
  player's spread off his legs anim's stance and sight: `cod4_fx::client_bullets`,
  `play::cod4` (cod4-effects.md 12).
- Low-health overlay (`low health`, owner draw 112) and the
  `hit_direction` icons: `cod4_damage` (cod4-hud.md 19).
- Cursor hints (owner draw 72): `cod4_hud::hints` (cod4-hud.md 20).
- Grenade indicators: `cod4_hud::grenades` (cod4-hud.md 21).
- View recoil (through the cmd's angles), damage kick and landing dip,
  and the view model's recoil spring, damage kick and landing dip:
  `play::cod4_kick` (cod4-client-view.md 14).
- Hudelem text glow, the connect screen's glowing titles, the compass's
  tickertape with its objective icons (cod4-hud.md 22); a shellshock's
  blend hiding the hit direction icons (19); the connect screen's
  download rate and time left, a refusal's arguments (18,
  cod4-front-end.md 7); the connect screen's yellow lines (18).
- Gaps 10 and 11: the tracer's corkscrew beam (cod4-effects.md 12); the
  world's distortion glass and the world and static model `zfeather`
  sets, drawn past the frame's split over its copies (`cod4_worldfx`,
  cod4-world.md 9.5); the vision sets' glow held on the live path (11.4).
  Cost: cod4-client-perf.md 13.
- Gaps 5 to 9 (cod4-hud.md 23): the mantle, hold breath and invalid
  command hints; the `scoreboard` menu's header (game type, team scores
  and icons, time left off configstring 11); the XP bar and score bars;
  the menus' `play` sounds (the stock menus fade nothing: their one fade
  command names a missing item). Gap 9 closed with e071fbef (every
  expression of the 288 menus evaluates).
- Gaps 6, 8, 14 and 15: the scoreboard's scrolling, scroll bar, ping
  graph (`cg_scoreboardPingGraph`) and headers on the first banner's line
  (cod4-hud.md 7.1); the Options rows' state (`sv_running`, the never
  registered `ui_allow_graphic_change`, `developer`; 12.6); a turret's
  own crosshair and `cg_crosshairDynamic`'s gun angles (17); the
  compass's 1.5 s friendly memory, `pingPlayer`'s flash (vcod-server
  too) and a shot ping held for its fire sound (15.7, 15.9).
- Sound (cod4-sound.md 2, 4): speaker maps' stereo levels, a brush
  model's `loopSound` at its bounds' middle, `EV_STOPSOUNDS`,
  `_AS_MASTER`, the fire sound at `tag_flash`, our own ring on our own
  entity, voice offsets turning with their entity (4.1).
- Turrets' client-side rounds and tracer rolls (cod4-effects.md 12.3).
- Voice chat (cod4-voice.md): the relay, `isTalking`, push to talk
  through libspeex, the talker list, the scoreboard icon, Eavesdrop, the
  compass's talker ping (cod4-hud.md 15.9).
- Ragdolls, the hardpoint vehicles' look, the killcam, culling cost and
  the bots, each listed in an earlier audit as another stream's, have
  landed: cod4-player-anims.md 10, cod4-client-view.md 11, cod4-hud.md
  11, cod4-client-perf.md, docs/bots.md "CoD4".
