# CoD4 1.7: what the client still lacks in a normal match

What a 1.7 player sees or uses in a public match that vcod's CoD4 client
does not draw, play or do, or fakes, ranked. The player-facing summary is
`docs/features/cod4.md`. Sources: the open sections of the `cod4-*.md`
docs, the HUD menus of `common_mp` (`menu_dump`) against what `cod4_hud`
reads, `iw3mp.exe` 1.7 for the menus' owner draws, and the live harness
in `play::cod4` (a client joined to vcod-server's `war` on mp_crash).
First audit 2026-10-26 at 0f20b5ed; rechecked against the code
2026-10-30. Sizes: S under an hour, M a few hours, L a day or more.

## Open

Ranked by how much a player notices.

- CoD4x servers. Most public servers run CoD4x, which parks a 1.7
  client and hands it to its own update backend; vcod refuses them at the
  challenge and hides them in the browser (cod4-handshake.md 13). L.
- Retail's `mpdata` is never read: a retail rank does not carry over.
  By choice.
- Rigid-body physics is vcod's solver, not ODE: a launched part's 26
  box points (corners, edge midpoints, face centres) meet the world as
  points, so a world edge between them can poke in, and `msecStep`
  stays 17 ms (cod4-gsc.md 3.6). A jitter's pushes land on the body's
  touching points, not ODE's contacts, so a flat prop takes more of them
  (cod4-world.md 13.8). The ragdoll is a position-based solver
  (cod4-player-anims.md 10). M.
- `sv_clientSideBullets` 0 is not read: under it 1.7 draws no rounds of
  its own and plays the server's bullet events, which then push the
  props; vcod's client runs its rounds as under 1, which every stock
  server sends (cod4-world.md 13.10). S.
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
- The view weapon's anims still sum through CoD1's layered sampler: the
  tree 1.7 builds for it and the goal weights it blends by are unread
  (cod4-player-anims.md 11). M.
- The drawn bodies' skeleton composes with every result a float, where
  `iw3mp.exe` runs the x87 at double precision: a bone can sit an ulp
  off (cod4-player-anims.md 8, 11). S.

## Fixed

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
  and sets them back on a map restart, as 1.7's client does with nothing
  on the wire (cod4-world.md 13).
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
- Cursor hints (owner draw 72): `cod4_hints` (cod4-hud.md 20).
- Grenade indicators: `cod4_grenades` (cod4-hud.md 21).
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

## Ranked

The 2026-10-26 audit's numbering, kept because other docs cite it.

5. Fixed. "Can't go prone here" is the off-screen `stance` menu's, so
   1.7 draws none.
6. Fixed.
7. Fixed. The XP bar's fill waits on real rank stats (see Open, ranks).
8. Fixed.
9. Closed: every expression evaluates (cod4-hud.md 12.2).
10. Fixed: the tracer's corkscrew. Trails and emitted effects draw
    (cod4-effects.md 15, 16); no stock MP zone holds a spot light. The
    airstrike jets' trails ride their entering snapshot and play on
    neither client as far as `CG_ResetEntity` reads (16).
11. Fixed.
12. Sound: fixed but for multichannel output (see Open).
13. Fixed, turrets included, the own third-person tracer start too
    (cod4-effects.md 12.3).
14. Fixed; the gunner's body is drawn (cod4-client-view.md 13).
15. Fixed.
16. Fixed.
17. Fixed.
18. Voice chat: done (cod4-voice.md), `winvoice_*` included but the
    playback mute and `voice.wav`.

## Not gaps

- The stance indicator: 1.7's `stance` menu sits at -1000 -1000, off
  the screen, so retail draws none either. VERIFIED (`menu_dump`).
- Spot light effect elements draw as omni lights; no stock MP zone holds
  one (cod4-effects.md 9).
- Single Player's popup does nothing: vcod is multiplayer only.
