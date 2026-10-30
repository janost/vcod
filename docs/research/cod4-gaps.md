# CoD4 1.7: what the client still lacks in a normal match

A ranked list of what a 1.7 player sees or uses in a public match that
vcod's CoD4 client does not draw, play or do, or fakes. Sources: the
"not yet" lines of `docs/features.md`, the open sections of the
`cod4-*.md` docs, the HUD menus of `common_mp` (`menu_dump`) against what
`cod4_hud` reads, `iw3mp.exe` 1.7 for the menus' owner draws, and the
live harness in `play::cod4` (a client joined to vcod-server's `war` on
mp_crash). Audited 2026-10-26 at 0f20b5ed. Each line: what, where it
would live, size (S under an hour, M a few hours, L a day or more).
Items another work stream owns are marked and left to it.

## Fixed

- Distortion effects (heat haze, muzzle flash rings) bend a copy of the
  frame: `fs_distort`, `scene_copy`, cod4-effects.md 17.
- The spot lights' shadow maps, up to `sm_maxLights` with retail's fade
  history: `cod4_spotshadow`, cod4-world.md 3.5.
- Client-side bullets: the own shot's seeded spread (the server's disc
  point), rounds stopping on the other players' boxes with the flesh
  hit left to the server's event and on script models' collision,
  penetration with exit impacts, and
  another player's spread off his legs anim's stance and sight:
  `cod4_fx::client_bullets`, `play::cod4`, cod4-effects.md 12.

- Low-health overlay (`low health` menu, owner draw 112) and the
  `hit_direction` icons: `cod4_damage`, cod4-hud.md 19.
- Cursor hints (owner draw 72): `cod4_hints`, cod4-hud.md 20.
- Grenade indicators: `cod4_grenades`, cod4-hud.md 21.
- The view's recoil (through the cmd's angles), damage kick and landing
  dip, and the view model's recoil spring, damage kick and landing dip:
  `play::cod4_kick`, cod4-client-view.md 14. The view model's camo
  stays open.
- Hudelem text glow and the connect screen's glowing titles, the
  compass's tickertape with its objective icons (cod4-hud.md 22); a
  shellshock's screen blend hiding the hit direction icons (19); the
  connect screen's download rate and time left, and a refusal's
  arguments in its text (18, cod4-front-end.md 7).
- The connect screen's yellow lines, a server's `print` and the CD key
  wait (cod4-hud.md 18).

- Gaps 10 and 11 (C4-98): the tracer's corkscrew beam (cod4-effects.md
  12); the world's distortion glass and the world and static model
  `zfeather` sets, drawn past the frame's split over its copies
  (`cod4_worldfx`, cod4-world.md 9.5); the vision sets' glow, already
  drawn, now held on the live path (11.4). Cost: cod4-client-perf.md 13.

- Gaps 5 to 9 (cod4-hud.md 23): the mantle, hold breath and invalid
  command hints; the `scoreboard` menu's header (game type, team scores
  and icons, time left off configstring 11); the XP bar and score bars;
  the menus' `play` sounds (the stock menus fade nothing: their only
  fade command names a missing item). Gap 9 was already closed by
  e071fbef (every expression of the 288 menus evaluates).

- Gaps 6, 8, 14 and 15 (C4-84): the scoreboard's scrolling, scroll
  bar, ping graph (`cg_scoreboardPingGraph`), and its headers on the
  first banner's line at the top (cod4-hud.md 7.1; the talking icons
  were already drawn); the Options rows' state (`sv_running`, the never
  registered `ui_allow_graphic_change`, `developer`; 12.6); a turret's
  own crosshair and `cg_crosshairDynamic`'s gun angles (17); the
  compass's 1.5 s friendly memory, `pingPlayer`'s flash (on vcod-server
  too) and a shot ping's hold for its fire sound (15.7, 15.9). Left: the
  compass's voice fade is drawn off `isTalking` but no retail capture
  holds a talker; the name fade-in's wait on the crosshair colour.

## Ranked

5. Fixed (above). "Can't go prone here" is the off-screen `stance`
   menu's, so 1.7 draws none.
6. Fixed (above).
7. Fixed (above); the XP bar's fill waits on real rank stats.
8. Fixed (above).
9. Closed: every expression evaluates (cod4-hud.md 12.2).
10. Fixed (above): the tracer's corkscrew. Trails and emitted effects
    draw (cod4-effects.md 15, 16); no stock MP zone holds a spot light.
    The airstrike jets' trails ride their entering snapshot and play on
    neither client as far as `CG_ResetEntity` reads (16).
11. Fixed (above).
12. Sound: fixed (cod4-sound.md 2, 4): the speaker maps' stereo
    levels, a brush model's `loopSound` at its bounds' middle,
    `EV_STOPSOUNDS`, `_AS_MASTER`, the fire sound at `tag_flash`, our own
    ring on our own entity, voice offsets turning with their entity
    (4.1). Left: a multichannel output, which kira's stereo mixer
    cannot give (cod4-sound.md 4.2, 5).
13. (fixed, above), turrets' client-side rounds and the tracer rolls
    too (cod4-effects.md 12.3). Left: the own third-person tracer start,
    which waits on a third-person view (vcod draws none).
14. Fixed (above); the gunner's body is S-C4-25's.
15. Fixed (above).
16. (fixed, above)
17. (fixed, above)
18. Voice chat: done (cod4-voice.md): the relay, `isTalking`, push to
    talk through libspeex, the talker list, the scoreboard icon,
    Eavesdrop, the compass's talker ping (cod4-hud.md 15.9). Left:
    `winvoice_*`.

## Owned elsewhere

- Bots and server smalls: S-C4-29. Ragdoll: C4-66. Culling and effect
  cost: C4-65. Hardpoints and their sounds: C4-55. Browser, menus,
  killcam, HQ bar: C4-56. GL grenade, battle chatter, spectate type 1:
  S-C4-24. CoD4x servers: parked.

## Not gaps

- The stance indicator: 1.7's `stance` menu sits at -1000 -1000, off
  the screen, so retail draws none either. VERIFIED (`menu_dump`).
