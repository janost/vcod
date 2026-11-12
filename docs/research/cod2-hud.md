# CoD2 1.3 player HUD: `hud.menu`'s owner draws, the crosshair, damage icons

What the 1.3 cgame paints over a playing client and what it reads.
Addresses are the Mac 1.3 client (`cod2mp_mac13_i386`, symbols intact)
unless marked lnxded (`cod2_lnxded`). Float constants come from its
`__const` section at the cited addresses. `cg` is the cgame's `cg_t`; the
predicted playerstate sits at `cg + 0x25bc4`, so `cg + 0x25bc4 + n` is
playerstate offset `n` (`PLAYER_STATE_FIELDS`' offsets). VERIFIED
(`CG_DrawPlayerAmmoValue` hands `cg + 0x25bc4` to
`BG_GetTotalAmmoReserve(playerState_s *, int)`).

vcod draws it in `crates/client/src/cod2_player_hud.rs`, called from
`cod2_hud.rs`.

## 1. Where it is drawn

`CG_Draw2D` (0x1d114a), in order. VERIFIED.

- `pm_type` 5: the scoreboard only.
- `pm_type` 4 (spectator): `CGAME_SPECTATOR`, crosshair names, chat and
  the hudelems; no menus.
- Otherwise: the crosshair (for `pm_type` under 6), crosshair names and
  chat, `CG_CheckTimedMenus`, the background hudelems, `Menu_PaintAll`
  (the HUD menus), the foreground hudelems, the scoreboard, a red damage
  flash and the damage direction icons.

`ui_mp/hud.txt` loads `ui_mp/hud.menu` only. VERIFIED (the stock file). Its
visible menus and their owner draws, numbered by `ui/menudefinition.h`.
VERIFIED (the stock menu). `CG_OwnerDraw` (0x18cc30) switches on the
number. VERIFIED.

| menu | owner draws |
|---|---|
| `MantleHint` | 80 |
| `Cursorhints` | 72 |
| `stance` | 20 |
| `weaponinfo` | 6 backdrop, 83 mode icon, 5 ammo (82 commented out) |
| `Health` | 95 back, 79 bar |
| `Low Health` | 109 |
| `Compass` | 85 back and highlight, 84 face, 86 objectives, 89 friendlies |
| `offhandinfo` | 100..105 |
| talk icons | |

## 2. Fades

`CG_FadeColor(start, total, fade)` (0x1a1b0e) is alpha 1 until
`total - fade` ms past `start`, falls to 0 at `total`, and is null when
`start` is 0 or `total` is past. VERIFIED.

The HUD pieces fade on these dvars (seconds, a 700 ms fade; 0 means always
shown). VERIFIED (the `Dvar_Register*` defaults).

| dvar | default |
|---|---|
| `hud_fade_ammodisplay` | 0 |
| `hud_fade_healthbar` | 2 |
| `hud_fade_compass` | 0 |
| `hud_fade_stance` | 1.7 |
| `hud_fade_offhand` | 0 |

The stance's clock is `cg + 0x2c5cc`, which `CG_MenuShowNotify(3)` stamps
when `CG_CheckTimedMenus` (0x1ce0ae) sees the cmd's stance buttons
(`0x2300`) change. VERIFIED. vcod stamps it when `pm_flags & 3` changes and
at a spawn, a stand-in. INFERRED.

## 3. The ammo counter (5, 6)

`CG_DrawPlayerAmmoValue` (0x18c440). VERIFIED.

- Nothing without `ps.weapon`. The weapon is `cg_weaponSelect`
  (`cg + 0x2be50`) when `pm_flags & 0x800000` and the weapon is held, else
  the client's entity's weapon. vcod takes the selection when the
  playerstate holds it, else `ps.weapon`.
- The reserve is `BG_GetTotalAmmoReserve` (0x7a9d0): `ammo[ammo index]`, or
  `ammoclip[clip index]` for a `clipOnly` weapon (shared ammo caps aside;
  no stock weapon has one). A `clipOnly` weapon has no clip part. Both
  clamp to 999.
- The clip prints `%2i` at the rect's left, the reserve `%3i` right-aligned
  to the rect's right, and `|` at `x + (w - width("|")) * 0.5 - 5`
  (0x326190 0.5, 0x32628c 5.0). With one part only, it is centred.
- `CG_CheckPlayerForLowClip` (0x18b932): the clip at `clipSize * 0.33` or
  less (0x3264a4). The clip is redrawn over itself in (0.89, 0.18, 0.01),
  alpha falling from 1 to 0 over 800 ms (0x326514), restarting every
  800 ms.
- `CG_CheckPlayerForLowAmmo` (0x18b852): the reserve at `maxAmmo * 0.2` or
  less (0x3262dc). The reserve and the bar draw in (0.89, 0.18, 0.01).
- The backdrop (6) draws the item's background in the item's colour, or
  that red on low ammo, while `ps.weapon` is set.

Ammo and clip slots: `cod2-weapons.md` 6, "Clip indexes"; slot 0 is the
built-in default def's `""`.

## 4. The weapon name and mode icon (81, 83)

81: `CG_FadeColor(cg + 0x2be54, 1800, 700)`, the selection's clock, over
the item's colour; the weapon as in 3. The text is the translated
`displayName`, or `"%s / %s"` with the translated `modeName` when that is
set, at `x + w - width - 28` (0x3264a8), baseline `y`. VERIFIED.

83: the weapon's `modeIcon` material (`cg_weapons[w] + 0x13c`) over the
item's rect, always (its fade is `hud_fade_ammodisplay`'s 0). VERIFIED
(`CG_RegisterWeapon`'s stores: `+0x138` `hudIcon`, `+0x13c` `modeIcon`).

## 5. The stance icon (20)

`CG_DrawPlayerStance` (0x18bbbc): `stance_prone` with `pm_flags & 1`,
`stance_crouch` with `& 2`, else `stance_stand` (cgs 0x27de0, 0x27ddc,
0x27dd8 in `CG_RegisterGraphics`' order), over the rect shifted by
`(cg_hudCompassSize - 1)` and `cgs + 0x28388 * 0.7`. VERIFIED. With
`pm_flags & 0x10000` it prints `CGAME_PRONE_BLOCKED` pulsing for 1.5 s,
and `cg_hudStanceHintPrints` (default 0) adds key hints and a
`stance_flash`. VERIFIED. vcod draws the icon, the prone-blocked line and
the hints (section 9).

## 6. The compass (84, 85, 86, 89)

- 85: the item's background over the rect grown upward by
  `cg_hudCompassSize - 1`. VERIFIED (`CG_DrawPlayerCompassBack`,
  0x18ba26).
- 84: the face, drawn by `CG_DrawRotatedPic` turned by `cg + 0x2c5b0`.
  `CG_UpdateCompassOrientation` (0x1a1bbc) springs that toward
  `AngleNormalize360(view yaw - northYaw)` in 5 ms steps, snapping after
  500 ms unseen. `northYaw` is configstring 11, the worldspawn's
  `northyaw` or "0" (`CG_NorthDirectionChanged`, `G_InitGame`).
  VERIFIED.
- The spring step, read off the loop (INFERRED): with
  `d = AngleSubtract(angle, target)` and speed `v` (`cg + 0x2c5b4`), a
  step of `s` seconds (5 ms, the last one shorter) snaps to the target
  once `|d| < 0.25` and `|v| < 1`. Else `d = AngleNormalize180(d + s * v)`;
  `v` moves 1000 s against the sign of `d`, loses `2 v s`, loses `3.5 v s`
  more while it carries `d` away from 0, then `s` toward 0 without
  crossing it, clamped to +-30000. The constants (0.25, 3.5, 30000, 500,
  0.001 at 0x3261bc, 0x32653c, 0x326540, 0x32635c, 0x326210) are
  VERIFIED.
- `CG_DrawRotatedPicPhysical` (0x1a232e) places the rect, then turns its
  corners about its centre in real pixels: `(cx + cos * hw * sx - sin * hw
  * sy, cy + sin * hh * sx + cos * hh * sy)` over the `sign` table
  (-1,-1), (1,-1), (1,1), (-1,1). VERIFIED.
- 86, for each of the 16 objectives in state 4 (`"current"`). VERIFIED.
  - Position: its `origin`, or entity `entNum`'s lerped origin when that is
    not 0x3ff.
  - Yaw `vectoyaw(target - eye) - view yaw`; the view's unless
    `cg_hudCompassSpringyPointers` (default 0).
  - Radius `cg_hudCompassSize * 43.75 * f` (DOUBLE 0x340d40), `f` the
    distance from `cg_hudCompassMinRange` 0 to `cg_hudCompassMaxRange`
    1500 clamped to 0..1 (`cg_hudCompassMinRadius` 0).
  - Alpha 1 inside 1500, falling to `cg_hudObjectiveMinAlpha` (default 1)
    at `cg_hudObjectiveMaxRange` 2048.
  - A 16 x 16 icon at `centre - 8 - (sin * r, cos * r)`: the material of
    shader configstring `0x61e + icon` for an icon in 1..127, else
    `objective`.
- 89, `CG_DrawCompassFriendlies` (0x18f13e). VERIFIED.
  - Nothing for team 0 or 3.
  - Each snapshot `ET_PLAYER` (1) not `eFlags & 0x20000` on the local
    client's team is recorded, kept 800 ms, and drawn as a 10 x 10
    `objective_friendly` by the same radius rule, turned by
    `view yaw - his yaw`.
  - A talking one (`eFlags & 0x400000`) starts a 3 s flash once the last
    one is over (`cg + 0x2c5e8 + n * 0x1c`). While it runs and the time
    left modulo 500 is 250 or more he is drawn unturned as
    `objective_friendly_chat` in place of his icon and ping.
  - Compass pings (fire, grenades) add their own icons.
  - vcod draws the current snapshot's teammates only, the chat flash
    included.
- Fire pings: `CG_FireWeapon` (0x1d823c) hands a player entity's
  (`eType` 1) fire to `CG_CompassAddWeaponPingInfo` (0x18f060) with the
  fire sound's play length, for `EV_FIRE_WEAPON`..`_LASTSHOT` (158..160).
  VERIFIED. The ping is kept per client (`cg + 0x2c5d0 + n * 0x1c`); none
  for the local client or a shooter on team 3. The end time is `cg.time`
  plus that length; an enemy (local team 0, or another team) also keeps
  the fire's x and y. VERIFIED. `CG_DrawCompassFriendlies` draws a
  teammate pinging as `compassping_friendlyfiring` turned with him (his
  own icon dropped at full alpha), and every live enemy ping as an
  unturned 10 x 10 `compassping_enemyfiring` at the fire's spot, its
  distance capped at `cg_hudCompassMaxRange`. Alpha is 1 to the end, then
  falls over `cg_hudCompassSoundPingFadeTime` (2 s), at most the HUD's
  fade. VERIFIED (the materials at cgs +0x28360 and +0x28368,
  `CG_RegisterGraphics`). vcod ends a ping at the fire's own time: the HUD
  does not know the sound's length.
- The MG42's ping: `CG_EntityEvent`'s `EV_FIRE_WEAPON_MG42` arm (175,
  0x1e4650) shakes the camera, runs `CG_FireWeapon`, then, when the entity
  the event's parm names is a valid `eType` 1 and the event's entity is
  not the local client, pings that gunner at the event's origin for 50 ms
  (`CG_CompassAddWeaponPingInfo(..., 0x32)`). VERIFIED. vcod pings the
  gunner when the parm names a player in the snapshot.

## 7. Offhand grenades (100..105)

`CG_DrawOffHandIcon`, `-Ammo`, `-Name` (0x1b4f62, 0x1b5332, 0x1b4c6c).
VERIFIED.

- Nothing when `pm_type` is 6 or more, or when no held weapon has the
  class (1 frag, 2 smoke; `offhandClass`).
- The icon is the equipped offhand's `hudIcon` when its class matches,
  else the first held one with rounds (`BG_GetFirstAvailableOffhand`,
  0x7b9f4: ammo plus clip non-zero), else the first held
  (`BG_GetFirstEquippedOffhand`).
- The count is the sum of the clips of every held weapon of the class,
  `%i`, red at 0.
- The name is `offhandStrings[class]`, `WEAPON_FRAGGRENADE` or
  `WEAPON_SMOKEGRENADE` (`_offhandStrings`, `.data` 0x34d100), translated.

## 8. Health (79, 95, 109)

`cg_drawHealth` defaults to 0, so the bar (79) and its back (95) draw
nothing in MP. VERIFIED (`Dvar_RegisterBool("cg_drawHealth", 0)` and the
tests in cases 0x4f and 0x5f). The health fraction is
`stats[0] / stats[2]` clamped to 0..1, 0 when either is 0 or `pm_type` is
6 (`CG_CalcPlayerHealth`, 0x18a5e4). VERIFIED.

109 draws `overlay_low_health` full screen at the alpha of
`CG_PulseLowHealthOverlay` (0x18b2ae). VERIFIED.

- A drop below `hud_healthOverlay_pulseStart` 0.35 restarts the count.
- Each pulse rises to `pulseMags[n]` (1, 0.8, 0.6, 0.3; `.data` 0x33ba50)
  over 150 ms, to 0.7 of it over 320 ms, to 0.6 of it over 400 ms.
- The count steps once `hud_healthOverlay_regenPauseTime` 5000 less three
  cycles has passed since the drop; after the fourth pulse the alpha falls
  to 0 over 700 ms.
- The alpha lerps from the last target over each phase.

`CG_Respawn` (0x1e3c56) runs when the playerstate's word +0x140
(`stats[5]`, the spawn count) or its `clientNum` differs from the last
snapshot's, from the snapshot transition (0x1dee08) and the first snapshot
(`CG_SetInitialSnapshot`, 0x1df69e). VERIFIED. Among its stores it zeroes
the eight damage icons (cg +0x2be74, 0x60 bytes), the damage time
(+0x2bed4) and the two damage kick values (+0x2bf10, +0x2bf14), and ends
in `CG_ResetLowHealthOverlay` (0x18a650): pulse off, target alpha
`hud_healthOverlay_phaseEnd_toAlpha` (0), phase length, phase and pulse
count 0, last health fraction 1. VERIFIED. The overlay's owner draw
returns early at `pm_type` 6 without stepping the pulse (case 0x6d), so a
pulse running at death holds until the respawn resets it. VERIFIED.

## 9. Hints (72, 80)

`CG_DrawCursorhint` (0x18a7a6) at `cg_cursorHints` 4 (no pulse), with
`ps.cursorHint` naming the icon. VERIFIED (cgs 0x27bc4 + 4 * hint, the
`CG_RegisterGraphics` stores).

| hint | icon |
|---|---|
| 1 | nothing |
| 2 | `gfx/icons/hint_usable` |
| 3 | `hint_health` |
| 4 | `hint_friendly` |
| 5..132 | weapon `hint - 4`'s `hudIcon` (twice as wide with `wideListIcon`) |

The text is configstring `0x4fe + cursorHintString` through
`SEH_LocalizeTextMessage`, `&&1` replaced by `+activate`'s key
(`CG_GetUseString`, 0x18a700). `hint_health` without a string reads
`PLATFORM_PICKUPHEALTH`; a weapon not of class 7 reads
`PLATFORM_PICKUPNEWWEAPON` with a free slot and `PLATFORM_SWAPWEAPONS`
without. The text is centred with the icon after it: text at
`-(w + textW) / 2`, baseline `y + textH / 2`; the icon at its end,
`y - h / 2`. VERIFIED.

While the playerstate holds a hint, `CG_DrawCursorhint` latches it, its
string, `cg.time` and `cg_hintFadeTime` (100, flags 0x1001) at
`cg + 0x2bde8..0x2bdf4`, and draws the latched one at
`CG_FadeColor(time, fadeTime, 100)`. A hint outlives the playerstate's by
the fade time, fading over its last 100 ms. VERIFIED. vcod latches and
fades it the same way.

80: `PLATFORM_MANTLE` with `+gostand`'s key and `hint_mantle`, laid out as
the use hint, while `mantleState.flags & 8`. VERIFIED (`CG_DrawMantleHint`,
0x18ae44).

20, the prone-blocked line: `CG_DrawPlayerStance` (0x18bbbc), once past
its stance fade, sets `cg + 0x2be30` to `cg.time + 1500` when
`pm_flags & 0x10000` is set and the last one ran out. While it runs it
draws `CGAME_PRONE_BLOCKED` centred (`x = -width / 2`) at `cg_hudProneY`
(-160), aligned (7, 3), in the item's font and colour at alpha
`|sin((end - time) / 1500 * 540 degrees)|`. VERIFIED (0x32650c, 0x326510,
the dvar default). The stance menu shows on a stance key's change
(`CG_CheckTimedMenus`, 0x1ce0ae, the `buttons` 0x2300 test). VERIFIED.
vcod has no cmd there and restarts the stance fade on the refusal flag's
rising edge instead.

Under `cg_hudStanceHintPrints` (0, flags 0x1001) `CG_DrawPlayerStance`
also prints how to reach the other stances. VERIFIED off the function. A
change of `pm_flags & 3` starts a 3 s clock (`cg + 0x2be34`, `0x2be38`; 0
while the dvar is off). For each of stand, crouch and prone, the first
command with a key in the held stance's table (`C_180` standing at
0x36de80, `C_181` crouched at 0x36de20, `C_182` prone at 0x36ddc0) makes a
line: `PLATFORM_STANCEHINT_STAND`, `_CROUCH` or `_PRONE` with `&&1`
replaced by that command's keys.

| held | stand | crouch | prone |
|---|---|---|---|
| standing | none | `gocrouch`, `togglecrouch`, `lowerstance`, `+movedown` | `goprone`, `+prone` |
| crouched | `+gostand`, `raisestance`, `+moveup` | none | `goprone`, `lowerstance`, `toggleprone`, `+prone` |
| prone | `+gostand`, `toggleprone` | `gocrouch`, `togglecrouch`, `raisestance`, `+movedown`, `+moveup` | none |

The lines draw at the icon's right edge in the item's font, colour and
alignment, 1.5 units plus a text height apart. The first sits at the
rect's middle less 1.5, half a text height lower for one line and a text
height plus 1.5 higher for three. They hold for 2 s, then fade over the
third, never above the icon's own fade. The same clock lights
`stance_flash` in `cg_hudStanceFlash` (white) over the icon for 1 s, from
0.8. VERIFIED (1.5, 1000, 0.8 at 0x3261b8, 0x326180, 0x3263a8). vcod draws
both (`PlayerHud::stance_hints`); with the dvar at 0, as stock, neither
draws.

## 10. The crosshair

`CG_DrawCrosshair` (0x1d0784), VERIFIED unless marked:

- None on a turret (`eFlags & 0x300`; the turret's own instead), in
  `weaponstate` 1, 2, 5, 10 or 11 (raising, dropping, reloading and two
  offhand states), or with `cg_drawCrosshair` 0. The weapon is
  `BG_GetViewmodelWeaponIndex`'s.
- The colour is white, or (0.25, 1, 0.25) with `pm_flags & 0x100000`. The
  alpha multiplies `cg_crosshairAlpha` (1) by what `CG_DrawWeapReticle`
  (0x1d0320) leaves in `xmm0`: 1 with no overlay drawn (0x1d0365), else
  `1 - ` the overlay's alpha (0x1d0545). VERIFIED, the disassembly; that
  the caller reads it as the function's return is INFERRED (Ghidra types
  it `void`). Under 0.01 nothing draws.
- None once `fWeaponPosFrac` (`cg + 0x25ca0`, the predicted
  playerstate's +0xdc) is exactly 1, unless `cg_drawGun` is 0
  (0x1d0860..0x1d0877). VERIFIED. With `cg_drawGun` 0 and a size scale
  under 1 (below), `reticleCenter` draws a second time at
  `reticleCenterSize * (1.5 - scale)` square, centred on
  `CG_CalcCrosshairPosition` without the drop; the crosshair itself draws
  unless the fraction is 1. VERIFIED, the decompile's tests and the
  rodata (0x3261b8 1.5, 0x3261f4 -0.5). So with the gun hidden a second
  dot grows as the crosshair shrinks and stays alone at the top of the
  sight. INFERRED.
- Past the sight's crosshair fraction (`adsCrosshairInFrac` / `-OutFrac`),
  `f = (pos - (1 - frac)) / frac`: the size scale is `1 - f / 2` and the
  centre drops by `480 / fov_y * f * adsAimPitch`.
- `reticleCenter` is `reticleCenterSize * scale` square at the centre.
- `reticleSide` is `reticleSideSize * scale`: four pieces turned 0, 90,
  180 and 270 degrees (top, right, bottom, left) at
  `d = max(tan(spread) * 240 / tan(fov_y / 2), reticleMinOfs) - size *
  hipReticleSidePos`, with `spread = (min + (max - min) * aimSpreadScale /
  255) * scale` off `BG_GetSpreadForWeapon`, alpha times
  `1 - aimSpreadScale / 255`, at least `cg_crosshairAlphaMin` 0.
- vcod takes `fov_y` from an 80 degree `cg_fov` at 4:3. INFERRED.
- vcod: `cod2_player_hud::CrosshairParts`, the second centre at the
  screen's centre.

## 11. Damage direction

`CG_DamageFeedback` (0x1e3e10) runs on a new `damageEvent` with
`damageYaw`, `damagePitch` and `damageCount`. Unless both angles are 255
it takes the oldest of eight slots: start the snapshot's time, length
`cg_hudDamageIconTime` 2000, yaw `damageYaw / 255 * 360` plus a random
-10..10 (0x32624c 20). VERIFIED. `P_DamageFeedback` (0x1a3366) packs
`damageYaw` from `vectoangles(damage_from)`, the hit's direction of
travel. VERIFIED.

`CG_Draw2D` draws each live slot as `hit_direction` on the quad (64, 192),
(-64, 192), (-64, 128), (64, 128) (`cg_hudDamageIcon*`: width 128, height
64, offset 128) about the screen's centre in real pixels, turned by
`view yaw - its yaw`, alpha `min(1, 2 - 2t / length)`; not under a sight
overlay unless `cg_hudDamageIconInScope`. VERIFIED. vcod hides them while
the scope overlay shows (17.4).

The red flash: `CG_DamageFeedback` sets `cg + 0x2bf0c` to `cg.time + 500`
and the two kick values (17.1). While `cg.time` is under it, `CG_Draw2D`
fills (-10, -10) to (width + 10, height + 10) in real pixels with
(0.2, 0, 0) at alpha `0.7 * min(f / 5, 1)`,
`f = |(end - time) * kickPitch| / 500`, after the hudelems and before the
damage icons. VERIFIED (0x32635c 500, 0x32628c 5, 0x326264 0.7). A hit
from the side kicks the roll, not the pitch, so it flashes faintly.
INFERRED.

## 12. What vcod leaves out

- The health bar (off by default).
- On the board (13): the mute list (`CL_MutePlayer`, the `muteplayer`
  script menu's `FEEDER_MUTELIST`), so the voice column stays empty, as
  retail's does while nobody talks or is muted; and style 3's exact shadow
  (13).
- Pending: pixel checks against retail. The headless frames come from
  `VCOD_UI_SHOT` (`renderer/hud_shot.rs`).

## 13. The scoreboard

`CG_DrawScoreboard` (0x1c6e30) is code, not a menu: no stock `ui_mp` file
describes it. VERIFIED (no `scoreboard*.menu` in the iwds; `ui_mp/hud.txt`
loads `hud.menu` only). Its shape is 1.1's board
(`cod11-hud-protocol.md` 3.1); vcod draws it with `hud::scoreboard` in the
`scoreboard::COD2` style. What differs from 1.1, each VERIFIED from the
cited function and the `__const` floats:

- The frame: `white` in black at alpha 0.8 (0x3263a8) over (120, 22, 400,
  430), and the six rules of `CalcBorderLines` (0x1c50a8; 1.1's six rects)
  in white at 0.1 (0x326388). Under a held key the alpha is 1; after
  `-scores` it is `CG_FadeColor(release, 100, 100)`. `score` goes out when
  `cg.time` is past the last request plus 2000.
- The title: `UI_GetGameTypeDisplayName` (0x14f58c), the first token of
  `maps/mp/gametypes/<g_gametype>.txt` (`UI_GetGameTypesList`, 0x15040e),
  translated, at 0.41 from x 129. The map at the right is
  `UI_GetMapDisplayName` (0x14f4a4) of the map's base name: the `.arena`
  `longname` (`mp/CoD2maps.arena` spells "Toujane, Tunisia" plain), else
  the name without `maps/` and `mp/`. The pair shrinks by 0.025 (0x3265b0)
  from 0.41, both measured at the shrinking scale, until it fits 386
  (0x3265ac), at most 14 steps; the map ends at 511 less 4.
- The footer: `sv_hostname` and the server's address
  (`CGAME_LISTENSERVER` for `0.0.0.0:0`), from 0.2 less 0.01 a step, 13
  steps.
- The objective: `cg_objectiveText`, as `CG_SetClientDvarFromServer`
  (0x1e2676) keeps it at `cg + 0x2a9fc`, through
  `CG_TranslateHudElemMessage`; font 0 at 0.24 from x 129, baselines 10.08
  (0x326584) under each 12-unit line from y 56, wrapped past 374
  (0x326594), then 2 down a 390-wide rule at 0.1 alpha.
- The columns, `columnInfo` (0x36e6e0, `{type, share, header, align}`):
  voice 0.05, status icon 0.05, name 0.47, score 0.13 (`CGAME_SB_SCORE`),
  deaths 0.13 (`CGAME_SB_DEATHS`), ping 0.17 (`CGAME_SB_PING`), the last
  three right-aligned. The share is of 382 (0x326598), or 374 when the
  board does not fit and a scroll bar takes the side. Headers at 0.3,
  baseline 11.76 (0x32659c) under the objective plus 4; the list starts 14
  (0x3265a0) lower.
- The fit: 10, plus `2 * cg_scoreboardBannerHeight + 8` with any axis or
  allies row, plus the banner height plus 4 for each of the none and
  spectator blocks, plus 12 a row, against 432 (0x326578) less the list's
  top.
- A banner (`CG_DrawScoreboard_ListBanner`, 0x1c5728) draws when
  `cg_scoreboardBannerHeight` (24) fits above 432. Its picture is the
  team's `g_ScoresBanner_*` over (129, y, width, 24 / 0.75 = 32). The text
  is `CGAME_SB_PLAYER(S)` with the count, for a team `"%s (%s)"` with the
  translated `g_TeamName_*` (`CGAME_SPECTATORS` for team 3), at 0.32 from
  x 133, baseline `24 * 0.84` (0x326588) down, in the team's colour. Axis
  and allies add the team score and the average ping in their columns. The
  rows start 32 under the banner's top (the function returns
  `y + height`); the next team's banner 4 under the last row. The leading
  team draws first (on a tie the last leader, `lastLeadTeam`), then the
  other, then team 0, then spectators.
- A row (`CG_DrawClientScore`, 0x1c51b2) is 12 high at x 129. The viewer's
  own (`cg + 4`) sits under `white` in the team colour at 0.2 (0x3262dc)
  of the alpha, every other row under `black` at 0.15 (0x32631c). The
  status icon is 13 x 13 at its column's left. Each text cell starts at
  0.22 (0x326580) and loses 0.02 (0x326568) until it fits its column;
  baseline 10.08 down, white, shadowed while still at 0.16 (0x326398) or
  more. Score and deaths are blank for team 3; the ping always prints.
- The scroll bar: `black` at 0.5 over (505, top, 8, 432 - top - 1), the
  thumb `white` at 0.25 at 506, the `hudscoreboardscroll_*` arrows and
  keys at 521 (1.1's layout).
- The scroll: while the board is up, `CG_KeyInterceptEvent` (0x18b258)
  takes keys 0xce and 0xa4 as `CG_ScrollScoreboardUp` and 0xcd and 0xa3 as
  `CG_ScrollScoreboardDown`, and `weapnext` and `weapprev` scroll up and
  down instead of cycling (`CG_NextWeapon_f`, 0x1dc352, `CG_PrevWeapon_f`,
  0x1dc2d8). Up moves `cg_scoreboardScrollStep` (3, 1..8) items back, not
  below 0. Down moves only a board whose last draw ran out of room, to at
  most the last row (0x1c4ec6, 0x1c4efc). VERIFIED. vcod's wheel, page
  keys and both commands scroll it this way.
- The voice column (`CG_DrawClientScore`): a 13 x 13 `voice_off` for a
  client `CL_IsPlayerMuted` names, `voice_on` for one `CL_IsPlayerTalking`
  (`Voice_IsClientTalking`) names, else nothing. VERIFIED. vcod has
  neither voice nor a mute list and draws nothing there.
- Text style: `UI_DrawText`'s styles 3 and 6 (`RB_DrawTextWithCursor`,
  0xd9b64) draw each glyph's shadow first, 1 (style 3) or 2 (style 6)
  pixels right and down, twice a quarter pixel apart, then the glyph twice
  a quarter pixel apart. VERIFIED (1.0, 2.0 and 0.25 at 0x326188,
  0x3261e4, 0x3261bc). The shadow's colour is the packed colour's low byte
  only. VERIFIED. That this is black at the text's alpha is INFERRED. The
  board's title, headers, banners and objective are style 3; a row's cell
  only while its scale is 0.16 or more, else 0 (`(scale >= 0.16) * 3`).
  VERIFIED. vcod keeps its 1-pixel shadow at 0.8 of the alpha for the
  shadowed text and drops it from a cell shrunk below 0.16.

Team names, colours and banners are server dvars (flag 0x100) that
`G_InitGame` (0x1addca) registers. VERIFIED.

| dvar | default |
|---|---|
| `g_ScoresBanner_Allies` | `mpflag_american` |
| `g_ScoresBanner_Axis` | `mpflag_german` |
| `g_ScoresBanner_None` | `mpflag_none` |
| `g_ScoresBanner_Spectators` | `mpflag_spectator` |
| `g_TeamName_Allies` | `GAME_ALLIES` |
| `g_TeamName_Axis` | `GAME_AXIS` |
| `g_TeamColor_Allies` | (0.5, 0.5, 1, 1) |
| `g_TeamColor_Axis` | (1, 0.5, 0.5, 1) |

`maps/mp/gametypes/_teams.gsc` sets them per `game["allies"]`. VERIFIED
(the stock script). They reach the client through the configstring mirror
(cod2-protocol.md 15.4). Retail's mp_toujane tdm sent `mpflag_british`,
`0.25098 0.25098 0.74902 0` and `MPUI_BRITISH` for allies,
`mpflag_german`, `0.6 0.6 0.6 0` and `MPUI_GERMAN` for axis. VERIFIED (a
net probe join of retail `cod2_lnxded`, configstrings 157..164 and
253..260). `CG_DrawScoreboard_GetTeamColor` (0x1c514c) unpacks the colour
and keeps the caller's alpha; any team but 1 and 2 is white. VERIFIED.
vcod reads them off the mirror (`cod2_hud::server_dvar`) and falls back to
the `G_InitGame` defaults. INFERRED, the fallback: a server that sends no
mirror row leaves the client's dvar as the cgame finds it.

## 14. Chat, the say field and the game messages

`CG_DrawChatMessages` (0x1cf0d0). VERIFIED.

- Nothing while `cg_chatHeight` (8) is 0. The ring is 1.1's
  (cod2-protocol.md 3.4.1); the oldest line goes once `cg_chatTime`
  (12000) has passed since it came, one a frame.
- Line `i` from the newest (`i` 0) has its top at `cg_hudChatPosition`'s y
  (150) less `10 * (i + 1)`.
- First a `hudcolorbar` strip at x 0, the line's width plus 24 wide and 10
  high, in the colour of the line's leading `^N` (white without one) times
  0.25 (0x3261bc), at alpha 0.6 (0x326500) times the fade. Then the line,
  codes and all, in white at x `(int)` 5, baseline the top plus 9, font 0
  at 0.20833. Both are left- and top-aligned (`UI_DrawHandlePic`'s 1, 1).
- The fade is 1 until the last 200 ms (0x3262ec), then falls to 0.
- `cg_hudChatPosition` is a vec2 (5, 150) and `cg_hudSayPosition` (5,
  126), both registered in `CG_Init` (0x145214, flags 0x1001, 0..640). The
  strip's x is always 0, the text's x `(int)` the position's x, the lines'
  y `(int)` its y.

vcod reads `cg_chatHeight`, `cg_chatTime` and both positions each frame
(`cod2_hud::HudDvars`).

`Con_DrawSay` (0x15d074), called last in `CG_Draw2D` with
`cg_hudSayPosition`'s y plus 24: while the chat field is open, `"%s: "` of
`EXE_SAY` or `EXE_SAYTEAM` (by `chat_team`) at `cg_hudSayPosition`'s x,
top 150, font 0 at 0.20833, white, left and top aligned, the field after
it. VERIFIED.

The game message window (kills, `iPrintLn`): `CG_Draw2D` calls
`Con_DrawNotify(6, y, alpha, 2)` (0x15dcb6), which draws window 0 with a
12-unit line, horizontal align 1 (left) and vertical align 3 (bottom).
VERIFIED. `y` is `floor(compass y - (cg_hudCompassSize - 1) * compass h +
12 + 0.5)` off the `Compass` menu's rect (`CG_LoadHudMenu`, 0x1440da):
`rect -20 -140 160 160` bottom-aligned in `hud.menu`, so -128. VERIFIED.
Mode 2 of `Con_DrawMessageWindow` (0x15d6f0) is 1.1's game window: lines
pushed down while one slides in over the scroll time, then newest first,
each 12 higher. VERIFIED. `Con_OneTimeInit` gives each window 8 lines, 3
forced into their fade, 250 ms scroll and fade-in, 500 ms fade-out (1.1's
numbers). VERIFIED. The window draws with alpha 1 while the board is down;
with it up, at `CG_FadeColor` of the board's last release over 100 ms,
which is past for a held board, so a held board hides it. VERIFIED. vcod
places the newest baseline at (6, 352) of the 640 x 480 grid.

The kill line (`CL_DeathMessagePrint`, 0x16058c). VERIFIED.

- The attacker's name and a space in his colour, the icon, a space and the
  victim's name in his. Each name is the client info's, 31 bytes and
  `^7`, its colour codes dropped as the line is written.
- The icon's width code is `(int)(32 * width)`, 1.4 or 2.8
  (`wideKillIcon`; 0x32664c, 0x326650), its height `(int)(32 * 1.4)`.
  `flipKillIcon` marks it 0x1300 instead of 0x1000; `R_DrawConsoleText`
  (0xece9a) draws a marked icon mirrored, its u running 1 to 0.
- The names' colours are `CG_DrawScoreboard_GetTeamColor`'s (13).
  VERIFIED (`CG_Obituary`).
- Every stock MP weapon sets `flipKillIcon 1`. VERIFIED (`weapons/mp/*` in
  `iw_13.iwd`).

## 15. The centre print

`CG_PriorityCenterPrint` (0x1cd9a6). VERIFIED.

- A print replaces the current one when none is up or its priority is not
  lower.
- The text goes through `CG_TranslateHudElemMessage`; a space past 75
  visible characters becomes a line break.
- The start is `cg.time + 2000`, the character width `(int)` of the
  call's (the obituary passes 9.6), the line count 1 plus each `\n`.
- `CG_Obituary` prints with priority 1.

`CG_Draw2D` draws it after the scoreboard whether or not the board is up.
VERIFIED. Alpha `CG_FadeColor(start, cg_centertime * 1000, 100)`
(`cg_centertime` 5); scale the character width times 0.03125 (0x326430);
the first baseline at `cg_centerPrintY` (-60) plus
`-1.2 * text height * lines` (0x3264e8), each line `1.2 * text height`
lower (0x326614); at most 75 characters a line; x `-width / 2`;
horizontal align 7 (the safe area's centre), vertical align 3 (bottom).
vcod draws it 480 - 60 up from the 640 x 480 grid's bottom, centred on
320.

## 16. Shellshock

Configstrings 0x48e + i (i 1..15) name the level's shocks.
`CG_LoadShellShockDvars` (0x1d6d88) loads them at the gamestate from
`shock/<name>.shock` (falling back to `shock/default.shock`), and
`CG_SetShellShockParmsFromDvars` (0x1d6f2a) builds them; record 0 is the
console's. VERIFIED.

`Com_LoadDvarsFromBuffer` (0x54806) resets the 29 dvars of
`_cg_shock_dvar_names` (`.data` 0x34d680) to their defaults, reads a name
and the next token on its line, warns on an unknown name, and fails a file
that leaves one out. VERIFIED. The warning is `^3WARNING: unknown dvar '%s'
in file '%s'`; the value is skipped with the rest of its line
(`Com_SkipRestOfLine`). VERIFIED.

Every stock file has `cg_shock_viewKickFadeTime`, which no 1.3 module
registers: `CG_Init` (0x145214) registers `viewKickPeriod` and
`viewKickRadius` and no fade, the name is in no string table of the Mac
client, and `CG_SetShellShockParmsFromDvars` reads only the period and
the radius for the kick. VERIFIED (the registrations, `strings`). Retail
ignores the line and warns on each load (`hold_breath.shock` in
`CG_RegisterGraphics`, 0x14374a); vcod's `parse_named` does the same with
the same warning. A value outside the dvar's range keeps the default
(`Dvar_SetVariant`'s range test). INFERRED.

Against 1.1's 27 cvars (cod11-shellshock.md 3, 4), VERIFIED:

- CoD2 adds `soundDryLevel` (1) and `volume_auto2d` (0.1).
- Times are `floorf(x * 1000 + 0.5)` ms with the dvars' 0.001 floors.
- The kick rate is `0.001 / period`, or 1 when that is under 0.001.
- The record's eleven volumes (+0x38..+0x60) go to `SND_SetChannelVolumes`
  in the order `auto, auto2d, menu, body, item, weapon, voice, local,
  music, announcer, shellshock` against the channels `auto, auto2d, menu,
  weapon, voice, item, body, ...`, so 1.1's crossing of the four 3D
  channels stands (the stores and the loop). That element i is channel i
  is INFERRED off `SND_SetChannelVolumes` (0x4a2de).

`CG_DrawActiveFrame` (0x1d4a50) picks the record of `ps.shellshockIndex`
(+0x5e4) with `shellshockTime` and `shellshockDuration` off `cg.snap` and
runs `CG_UpdateShellShock` (0x1d7528). VERIFIED. Its formulas are 1.1's
(cod11-shellshock.md 5..9): the sound fades, loop blend and end cue; the
mouse scale and fade; `CL_SetUserCmdInShellshock` (button 0x800 in
`CL_CmdButtons`, 0x188ee2, while `elapsed < duration`); and the kick off
`_cg_perturbations` (`.data` 0x33c1a0), byte for byte 1.1's 131-point
table, faded over 3000 ms. VERIFIED. `CG_PerturbCamera` (0x1d6c10) turns
the view axes by the kick; the screen blend is
`CG_DrawShellShockSavedScreenBlend` (0x1d6e64), 1.1's capture and blend.
VERIFIED. Stock MP shocks a player for 1 to 4 s of `default` off explosive
damage over 10 (`maps/mp/gametypes/_shellshock.gsc`). VERIFIED.

### 16.1 The turn caps and the console's shock

- `CG_UpdateShellShock` hands the mouse caps to `CL_CapTurnRate`
  (0x17b078, into `cl + 0x24788` pitch and `0x2478c` yaw). VERIFIED (the
  calls and their operands). The caps are 0 and 0 without a shock, past
  its end, or with `cg_shock_mouse` 0. Inside the last `mouse_fadeTime`
  they are the record's `maxpitchspeed` and `maxyawspeed` over
  `f = left / fadeTime`, the sensitivity `(scale - 1) * f + 1`; else the
  record's own. That this is 1.1's rule (cod11-shellshock.md 7) is
  INFERRED.
- `CL_MouseMove` (0x18908e) multiplies the frame's mouse by the
  sensitivity and `m_yaw` / `m_pitch`, then, with a cap set, holds the yaw
  and the pitch each to `frame_msec * cap * 0.001` (0x326210) either way.
  VERIFIED (the branches). vcod: `play::cod2_shock::TurnCap`, the frame's
  moves summed and the sum held, set each frame off the shock's
  `ShockMouse`.
- `cg_shellshock <duration> [file]` (`CG_ShellShock_f`, 0x1d2e3c): with a
  file, `CG_LoadShellShockDvars` loads it and a failure starts nothing.
  Then record 0 (`cgs + 0x22a44`) is built from the dvars as they stand,
  its start `cg.time` (`cg + 0x2ccf8`) and its duration
  `floorf(s * 1000 + 0.5)` (`cg + 0x2ccfc`, 0x326180 1000, 0x326190 0.5).
  Other argument counts print `USAGE: cg_shellshock <duration>
  <filename?>`. VERIFIED. `CG_DrawActiveFrame` plays record 0 with those
  two times while `ps.shellshockIndex` is 0 (0x1d4a50, the `+0x5e4` test).
  VERIFIED. vcod: `ViewFx::shock_command`, from the dvars' last file or
  their registered defaults (`cod2_shock::defaults`).

### 16.2 The hold-breath mix

`CG_RegisterGraphics` (0x14374a) loads `hold_breath.shock` into
`cgs + 0x23284`; a missing file is a `Com_Error`. VERIFIED.

`CG_AddPlayerWeapon` (0x1d9c8e) runs each frame for the view weapon,
drawn or not. VERIFIED (the branches, the aliases at
`cgs + 0x28340..0x2834c`, the dvars' defaults 4.5, 2 and 1 in
`BG_RegisterDvars`, 0x694d4).

- `cg + 0x2cd08` (the breath delay) loses `cg.frametime` while positive.
- With `pm_flags & 0x4000` (breath held) clear: a running hold
  (`cg + 0x2cd00` at 0 or more) adds the frame. Past
  `player_breath_hold_time * 1000` it plays `weap_sniper_breathgasp`;
  else, with the delay under 1, `weap_sniper_breathout`, and the delay
  goes to `player_breath_snd_delay * 1000`. The hold goes to -1,
  `cg + 0x2cd04` and the mix fraction (`cg + 0x2cd0c`) to 0.
- With the flag set: the fraction `DiffTrack`s toward 1 at
  `player_breath_snd_lerp` (2). A hold starting (at -1) goes to 0 and,
  with the delay under 1, plays `weap_sniper_breathin`, keeps its length
  (`CG_PlayClientSoundAlias`' return, `SND_PlaySoundAlias`' ms) in
  `0x2cd04` and sets the delay; else it zeroes `0x2cd04`. A running hold
  past that length plays `weap_sniper_heartbeat`. Then the hold adds the
  frame.

A non-zero fraction sets priority 1's channel volumes to the record's
eleven as `(v - 1) * f + 1`; a zero one deactivates it. VERIFIED.
`SND_SetChannelVolumes` (0x4a2de) keeps the highest active priority in
force, so a shellshock's 3 wins over it. INFERRED (the priority walk).
`weap_sniper_heartbeat` is a looping alias (`iw_06.iwd` `iw_sound2.csv`)
asked for every frame; vcod holds it while asked and stops it after
(`AudioSystem::hold`). INFERRED. `CG_HoldBreathInit` (0x1d7ddc) sets the
hold to -1 and the rest to 0. VERIFIED. vcod: `play::cod2_shock::HoldBreath`.

## 17. The view

`CG_CalcViewValues` (0x1d3b28) on a first-person body off a turret adds
`BG_CalculateViewAngles` (0x8033a) to the predicted angles, then runs
`CG_ShakeCamera`, `AnglesToAxis`, `CG_PerturbCamera` and `CG_CalcFov`.
INFERRED (the call order).

The eye, in order: the view height, the vertical bob, the horizontal bob
along the view's right, the landing dip, `cg + 0x284d0` over 100 ms off
`0x284d4`, `AddLeanToPosition` (16, 20), and the floor 8 over the origin.
VERIFIED (the stores in order).

The landing dip: `dt = cg.time - landTime` (`cg + 0x284dc`) as a float. A
negative one moves `landTime` to `cg.time - 450`, and that frame still
dips by it. Under 150 the height takes `dt / 150 * landChange`
(`cg + 0x284d8`), under 450 `((dt - 150) / -300 + 1) * landChange`.
VERIFIED (0x3261a0 0, 0x3264f8 150, 0x326618 450, 0x32661c -300, 0x326188
1). `CG_EntityEvent` (0x1e4650) sets the change and the time (`cg.time`)
on a landing of `cg.predictedPlayerState.clientNum`. VERIFIED (0x3262f0
0.01, 0x326354 12, 0x326358 26, 0x3261c0 4, 0x326470 24).

- `EV_LANDING_*` (93..115): `-parm`.
- `EV_LANDING_PAIN_*` (116..138): `-n` for
  `n = (int)((h - 12) / 26 * 4 + 4)` capped at 24, nothing at `h` 12 or
  under or `n` under 1, with
  `h = parm * 0.01 * (bg_fallDamageMaxHeight - MinHeight) + MinHeight`.

vcod: `play::cod2_view::Land`, the eye through `first_person_eye`. The
`0x284d0` term is not in vcod.

### 17.1 The damage kick

`CG_DamageFeedback` (0x1e3e10): `k = damageCount * 0.2`, held to 5..90.
With `damageYaw` and `damagePitch` both 255 the pitch kick is `-k` and the
roll 0; else the direction
`AngleVectors(pitch / 255 * 360, yaw / 255 * 360, 0)` gives pitch
`dot(dir, viewaxis[0]) * k` and roll `dot(dir, viewaxis[1]) * -k`. The
damage time is `cg.snap->serverTime`. VERIFIED. `BG_CalculateViewAngles`
adds `g * f * kick` to pitch and roll: `f = 1 - pos / 2`, times
`1 + pos / 2` on an `adsOverlayReticle` weapon; `g` a `GetLeanFraction`
rise over 100 ms and an eased fall over 400. VERIFIED (0x326350 100,
0x326290 -400).

### 17.2 The shake

`CG_ShakeCamera` (0x1cea88) and `CG_StartShakeCamera` (0x1d0138) are
1.1's four-slot quake (cod11-combat.md, "The earthquake shake"): the
600 ms clock, the 18, 16 and 10 degree gains and the 8, 15 and 12 pi
frequencies. VERIFIED (0x3263bc, 0x326280, 0x326260, 0x32626c,
0x340d58..0x340d68). `CG_EntityEvent` starts one on `EV_FIRE_WEAPON_MG42`
(175: 0.05 for 100 ms within 100) and `EV_EARTHQUAKE` (196: `angles2[0]`,
`time`, `angles2[1]`, as `GScr_Earthquake`, lnxded 0x81161ba, writes
them). VERIFIED. The floor at `cg + 0x2c5a0` is only ever zeroed
(`CG_AddPacketEntities`). VERIFIED.

### 17.3 The scope's sway and bob

On a weapon with `adsOverlayReticle`, `BG_CalculateViewAngles` adds the
idle sway. VERIFIED.

- Amount and speed are `hipIdle*` (an amount of 0 reads 80 at speed 1),
  or on an ADS weapon blended to `adsIdle*` by `fWeaponPosFrac`.
- The stance factor (`idleCrouchFactor`, `idleProneFactor`, off `eFlags` 4
  and 8) eases 0.5 a second on a copy of `cg + 0x284a4`.
- `k = amount * factor * pos * holdBreathScale`; the idle clock
  `cg + 0x2beec` advances `speed * holdBreathScale * ms`.
- Yaw `+= sin(t * 0.0007) * k * 0.01`, pitch `+= sin(t * 0.001) * k *
  0.01`.
- Then the gun's walk bob (section 18) times `pos`. Off a turret, with
  `adsViewBobMult`, 1.1's sight bob (amplitude capped at 45).

### 17.4 The zoom and the overlay

`CG_GetViewFov` (0x1d33ca): 90 at intermission; else `cg_fov` (80), on an
ADS weapon `adsZoomFov` at `pos` 1, else `cg_fov - (cg_fov - adsZoomFov) *
z` for `z = (pos - (1 - frac)) / frac` past 0; 55 on a turret; times
`cg_fovScale` (1), at least `cg_fovMin` (10). `frac` is `adsZoomInFrac`
while `cg + 0x28494` is set and `adsZoomOutFrac` while it is clear;
`CG_AddViewWeapon` sets it at `pos` 0 and clears it at 1. VERIFIED.
`CG_GetWeapReticleZoom` is the same `z` for a weapon with an
`adsOverlayShader` or reticle, true past 0.01, and `CG_AddViewWeapon` then
draws no gun. VERIFIED.

`CG_DrawWeapReticle` (0x1d0320), called from `CG_DrawCrosshair` before the
hip crosshair. VERIFIED.

- The overlay material at alpha `min(z, 1)`, centred on
  `CG_CalcCrosshairPosition` (the screen centre for a scope, whose gun
  angles are the view's).
- Wider than 320 or taller than 240: one `adsOverlayWidth` x
  `adsOverlayHeight` picture; else four mirrored quarters of it.
- `CG_DrawFrameOverlay` (0x1cdbf2) fills each screen edge outside it with
  the texture's first column or row.

The stock scoped Kar98k draws `scope_overlay_german` at 480 x 480 and
zooms to 15 past 0.42 in. VERIFIED (`weapons/mp/kar98k_sniper_mp`).

## 18. The view weapon's place

`CG_AddViewWeapon` (0x1da2f8) and `BG_CalculateWeaponAngles` (0x7f226);
`play::cod2_gun` ports them, the recoil spring aside (another module's,
in through `GunInput::recoil`).

- The turn sway, `BG_CalculateWeaponPosition_Sway` (0x7b5fe), on a frame
  with ms. VERIFIED.
  - None down a scope's sight.
  - The six `sway*` keys, blended to `adsSway*` by `pos` on an ADS weapon.
  - The view's pitch and yaw turn since the last frame, divided by
    `ms * 0.06`, held to the max angle.
  - The offset's left and up `DiffTrack` (0x2c220) toward `yaw * horiz` and
    `pitch * vert`; the lag's pitch and yaw `DiffTrackAngle` (0x2ce40: the
    short way, truncated to 65536ths) toward `pitch * pitchScale` and
    `yaw * yawScale`. Each target is times the shellshock's
    `swayShellShockScale` smoothstep, tracked at `lerpSpeed * ms / 1000`
    of the gap, snapping within 0.001.
- The offset. VERIFIED.
  - The stance's `*Move{F,R,U}` (stance off `eFlags` 8 and 4) times
    `(speed - *MoveMinSpeed) / (ps.speed - min)` in 0..1 past the min
    speed, none while reloading.
  - Plus `duckedOfs` or `proneOfs` at a `viewHeightTarget` of 40 or 11.
  - Eased at `posMoveRate` (`posProneMoveRate` at a current height of 11),
    at least 0.1 a second; faded by `1 - 2 pos`, gone at half sight.
  - The lean adds `(1 - pos) * lean * 1.6` of `AngleVectors(0, 0,
    -2 lean)`'s right; the sway's offset comes off the left and onto the
    up; the `cg_gun_*` dvars are 0.
  - In the world, `F * forward - L * right + U * up` off the eye.
- The angles, in order. VERIFIED (0x32638c -0.1, 0x326388 0.1, 0x326374
  11, 0x3261d0 0.0005, 0x326394 0.0007, 0x32627c -0.75, 0x3262e8 400,
  0x326638 1.6, 0x326380 60).
  - `-2 lean` on the roll; `pos * adsAimPitch` on an ADS weapon.
  - The movement tilt (`*Rot{P,Y,R}`, `*RotMinSpeed`, `posRotRate`,
    `posProneRotRate`; the target times `1 - pos`, the result faded as
    the offset is).
  - The idle drift: 17.3's amounts, the factor eased for keeps unless a
    scope is sighted, times `1 - pos` on a scope, the clock at
    `speed * ms` with no breath scale, sines of 0.0005, 0.0007 and 0.001
    on roll, yaw and pitch, by 0.01.
  - The walk bob: `0.16 * speed * bg_bobAmplitude*` capped at 10, pitch
    and yaw a quarter turn past the eye's cycle, the roll 1.5 times and
    only ever down, toward `adsBobFactor` by `pos`, times `1 - pos` on a
    scope.
  - The damage kick: `f = 0.5 + pos / 2`, up over `100 f` ms and down over
    `400 f`, times `1 - 0.75 pos` on a sighted scope; pitch
    `+= g * kickPitch / 2`, yaw `-= g * kickRoll`, roll
    `+= g * kickRoll / 2`.
  - On an ADS weapon the recoil spring's angles; then the sway's lag off
    pitch and yaw.
  - The model's axes are `AnglesToAxis(angles)` times the view's.

### 18.1 The sight between snapshots

- The client predicts the weapon. `CG_PredictPlayerState` (0x1ddafa) steps
  every cmd through `Pmove(&cg_pmove)` (0x75931), whose `PmoveSingle` arm
  calls `PM_AdjustAimSpreadScale`, `PM_UpdateAimDownSightFlag` and, after
  the move, `PM_Weapon`. VERIFIED (the calls). `fWeaponPosFrac`,
  `weaponstate`, `weapAnim`, the sight flag and the weapon timers move
  every frame, and the zoom (17.4), the gun's place (18) and the sight
  clips (cod2-weapons.md 6) read them from `cg.predictedPlayerState`.
  vcod runs the same step in its predictor
  (`vcod_cod2::pmove::weapon::armed_step`) and plays the weapon events off
  the replay (`play::cod2_predict::predicted_event`). VERIFIED against the
  retail movement and combat captures: every row's sight, state, timers
  and anim land on the next snapshot's bit for bit
  (`retail_captures_predict_bit_for_bit`).
- A body the cgame does not predict (a followed player, a demo) gets
  `CG_InterpolatePlayerState` (0x1dd8b2): the newer snapshot's playerstate
  with the origin, velocity, `bobCycle`, `viewHeightCurrent` (snap+0x104),
  `leanf` (snap+0x58) and `fWeaponPosFrac` (snap+0xe8; ps at snap+0xc)
  lerped from the older by `frameInterpolation`. VERIFIED (the stores).
  `cod2_viewmodel::pos_frac` takes the lerp unpredicted and the frame's
  own value predicted.
- The view weapon is drawn at the scene's fov, zoom included.
  `R_SetViewParmsForScene` (0xc5a9a) builds one projection from the
  refdef's fov and stores `r_znear_depthhack` (0.1) beside it;
  `RB_SetDepthHackNearClip` (0xcd55e) swaps only the near plane for the
  depth-hacked surfaces. VERIFIED (the stores). 1.3 reads no
  `adsZoomGunFov` (cod2-weapons.md 5). Down the Kar98k's iron sight
  (`adsZoomFov` 50) the gun is magnified with the world, as 1.1's is
  (xmodel-v14-format.md, "The view fov"). INFERRED (one projection for
  every scene surface). vcod draws the gun at the zoomed fov; drawing it
  at the unzoomed 80 is the trap.
- `CG_AddViewWeapon` also drops the gun on a landing:
  `z += 0.25 * landChange * dt / 150` for `dt = cg.time - landTime` under
  150 ms, `0.25 * landChange * (450 - dt) / 300` under 450 (cg+0x284d8,
  cg+0x284dc). VERIFIED (the branch on 0x96 and 0x1c2; 0x3261bc 0.25,
  0x3264f8 150, 0x32663c 300). The eye has already reset a landing ahead
  of the clock (17), so the gun's `dt` is never negative. INFERRED (the
  call order). vcod: `Land::gun`, along the drawn view's axes onto
  `play::cod2_gun`'s offset.

## 19. The sprites in the world and the spectator's lines

### 19.1 Waypoints (`CG_Draw3dHudElems`, 0x1e1ade)

- `HECmd_SetWaypoint` (lnxded 0x81044c0) sets `type` 13 and stores its
  argument as a float at `+0x70`, the `value` word (netfield offset 112).
  VERIFIED (the stores). `_objpoints.gsc` and `_deathicons.gsc` call
  `setWaypoint(true)`; ctf's flag points, sd's `objpoint_a`/`_b`/`_star`,
  hq's radio point and the death icons are such elements. VERIFIED (the
  stock scripts in `iw_07.iwd`). vcod-server writes the flag into `value`;
  keeping it in a field of its own sent `value` 0.
- A death icon is `addDeathIcon(body, ...)` on the entity `cloneplayer`
  returned: one team element for the victim's team and one for
  `spectator`, at `body.origin` plus 54 on z. VERIFIED (`_deathicons.gsc`
  in `iw_07.iwd`). `PlayerCmd_ClonePlayer` (0x1c3574) calls
  `G_SetOrigin(clone, ps->origin)`, so the body's `origin` is the player's
  where he died. VERIFIED (the call). On retail mp_toujane tdm a test
  client's `suicide()` logged `body.origin`, `self.origin` and the icon at
  (1489.38, 2269.57, 80.125), (1489.38, 2269.57, 80.125) and (1489.38,
  2269.57, 134.125). VERIFIED (a `logPrint` probe on `cod2_lnxded`).
  vcod-server reads a body-queue entity's `origin` from the corpse's
  `r.currentOrigin` (`BodyQueue::cod2_origin`, test
  `cod2_gaps::a_death_icon_sits_over_the_corpse`); an engine slot nothing
  writes put every icon at (0, 0, 54).
- The pass. VERIFIED.
  - Walks the two 31-element blocks (ps+0x1728, then ps+0x7a8), each up to
    its first free one, and sorts them with `compare_hudelems`.
  - For each type 13 element: its text (`CG_DrawHudElemString`), then,
    when its colour's alpha byte and its material are set,
    `CL_DrawSprite(material, white with that alpha, (x, y, z), size, 0,
    flags)`.
  - `size` is `height`, scaled from `fromHeight` over `scaleTime` past
    `scaleStartTime`; nothing when it is 0. With `value > 0` the size is
    times 0.0043 (0x326648) and the flags 0x3000.
  - The fade (`fadeOverTime`) is not read here: the alpha is the raw byte.
  - vcod draws no waypoint text: no stock script sets one.
- `RB_TessEntity`'s sprite case (0xff3cc, entity type 4, which
  `RB_DrawSpriteCmd`, 0xd6a32, fills from the command). VERIFIED.
  - Without flag 0x2000 the radius is in world units. When
    `minScreenRadius` is above 0, a sprite whose projected half height
    (NDC y times 0.5) is less is grown to it. Behind the eye nothing is
    drawn.
  - With 0x2000 the radius is `2 * size * w` (`w` the point's clip w)
    times the dot of the view's up with row 1 of the matrix at
    `viewParms + 0x108` (the arithmetic).
  - `R_SetViewParmsForScene` (0xc5a9a) builds that matrix as
    `MatrixInverse44` of `viewParms + 0xc8`, the product of
    `MatrixForViewer`'s view (+0x48) and `InfinitePerspectiveMatrix`'s
    projection (+0x88) (the calls). The view matrix's column 1 is the
    view's up (`MatrixForViewer`, 0x2b87e); the projection has only its y
    scale in row 1, `cot(fovY / 2) * 0.9995` (0x2b7a6), with clip w the
    depth (the stores).
  - Flag 0x1000 raises the sprite's centre by its own world radius
    (`RB_BuildSprite`, 0xfe78a). The quad faces the view (the view axes'
    left and up).
- So row 1 of the inverse is that scale's reciprocal times the up, the
  radius is the world length whose NDC half height is `2 * size`, and the
  radius is `size` of the screen's height at any distance, the 0.9995
  cancelling. An objpoint (`level.objpoint_scale` 7) is a 2 x 0.0301 of
  the screen's height square, sitting on its point, at any distance.
  INFERRED (the algebra over the stores above). vcod measures the same NDC
  height off its own projection (`cod2_world_hud::WorldView::sprite`).
- `objpoint_*`, `headicon*` and the death icons are `effect` materials
  whose state bits turn the depth test off (word 1 is 2, `mtlDepthTest`
  "Disable"). VERIFIED (the material files). They show through walls, so
  vcod draws them as projected 2D quads under the HUD
  (`crates/client/src/cod2_world_hud.rs`).

### 19.2 Head icons (`CG_DrawPlayerSprites`, 0x1cef3e)

- For each snapshot entity of `eType` 1, `CG_PlayerSprites` (0x1e662c).
  VERIFIED.
  - Nothing unless both its client and the viewed client (`ps.clientNum`)
    have client info.
  - The head icon, `iHeadIcon` (es+0x94) as configstring
    `0x1e + iHeadIcon`, draws when `iHeadIcon` is set and `iHeadIconTeam`
    (es+0x98) is 0, or the viewer's team is 3, or it equals the viewer's
    team.
  - A second sprite stacks `16 + cg_scriptIconSize` over it (0x326260):
    `headiconyouinkillcam` for the local client's own body in the
    killcam; else `headicondisconnected` for `eFlags & 0x80`; else, for a
    teammate or a spectator viewer only, the voice chat icon while its
    timer runs, else `headicontalkballoon` for `eFlags & 0x200000`, 5
    lower (0x32628c). The materials are cgs +0x27bb8, +0x27bb4, +0x27bb0
    (`CG_RegisterGraphics`).
- `CG_PlayerFloatSprite` (0x1e64c6). VERIFIED.
  - Skipped for the viewed body in the first person.
  - Placed at `j_head` (`scr_const` +0x96, `GScr_LoadConsts`) plus
    `21 + offset` (0x326658), or at the lerped origin plus `82 + offset`
    (0x326654) without a head.
  - Radius the icon's size dvar plus 10 (0x32626c), white,
    `cg_headIconMinScreenRadius` (0.02) as the minimum; with
    `cg_constantSizeHeadIcons` (default 0; the killcam icon always) the
    0.0043 scale and flags 0x3000.
  - The size dvars (`CG_RegisterDvars`): `cg_scriptIconSize`,
    `cg_voiceIconSize`, `cg_connectionIconSize` 0, `cg_youInKillCamSize`
    6.
- The size each `CG_PlayerFloatSprite` call takes in xmm0, which the
  decompile drops: `cg_scriptIconSize` for the head icon,
  `cg_youInKillCamSize` for the killcam icon, `cg_connectionIconSize` for
  the disconnected icon, `cg_voiceIconSize` for the voice chat and talk
  icons. VERIFIED (the `movss 0x8(%eax), %xmm0` loads at 0x1e6779 and
  after, off the dvar pointers 0x1acd9f1, 0x1acd9f5, 0x1acd9f9 and
  0x1acd9e1, which the symbol table names). The killcam icon is
  `(6 + 10) * 0.0043` of the screen's height in radius. INFERRED (that
  arithmetic). The killcam test compares the entity's number
  (`cent + 0xf0`) against `cg + 4`, the local client, not `ps.clientNum`.
  VERIFIED (0x1e66c9).
- The voice chat icon draws `cent + 0x214` while `cg.time` is under
  `cent + 0x218`. VERIFIED (0x1e66f8). No function in the Mac decompile
  stores either centity field, so the timer stays 0 and the icon never
  shows; `headiconvoicechat` is registered at cgs +0x27bac and read by
  nothing. INFERRED (a search of every store at those offsets).
- vcod draws the head icon, the killcam icon, the disconnected icon and
  the talk balloon. It has no voice, and retail's voice icon is dead code,
  so that one is not drawn.

### 19.3 The spectator's lines

- `CG_Draw2D` (0x1d114a) with `pm_type` 4 and the HUD menus shown:
  `CGAME_SPECTATOR` in font 4 at 0.5, x `-width / 2`, y -27, aligned
  (7, 3), style 3, then the crosshair names, the chat and the hudelems.
  VERIFIED. A follower's playerstate is his target's (`pm_type` 0), so
  only free flight shows it.
- After the board, when it is not up: `CG_DrawSpectatorMessage`
  (0x1cfeba), with `cg_descriptiveText` (default 1), no UI key catcher and
  `ps.pm_flags & 0x3000000`. Font 0 at 0.20833, x 240, y from
  `UI_TextHeight * -2 + 436` (0x3261e0, 0x3265e8), a line down each,
  aligned (0, 0). With 0x1000000: `PLATFORM_FOLLOWNEXTPLAYER` on
  `+attack` and `PLATFORM_FOLLOWPREVIOUSPLAYER` on `toggleads` (else
  `+speed`). With 0x2000000: `PLATFORM_FOLLOWSTOP` on `+melee` (else
  `+melee_breath`). An unbound key reads `KEY_UNBOUND`. VERIFIED.
- Then `CG_DrawFollow` (0x1ced8a): with `pm_flags & 0x400000` outside the
  killcam, `CGAME_FOLLOWING` and the followed client's name (`?` without
  client info), font 0 at 1/3, centred at y 40 and 65, aligned (7, 1),
  style 3. VERIFIED. The killcam flag (`cg + 0x2cd14`) is `ps.deltaTime`
  non-zero (`CG_SetNextSnap`, 0x1dee08). VERIFIED.
- vcod draws all three without the style 3 shadow.

### 19.4 Objectives on vcod-server

`G_RunFrame`'s objective pass (lnxded 0x8109ce0) copies the level's
16-record table (0x864c3a4, 0x1c a record) into each connected client's
playerstate (`ps + 0x5e4`). A record whose state is 0, or whose team is set
and is not the client's session team (`gclient + 0x274c`), gets its state
0 alone, the rest of the copy left from before. VERIFIED; 1.1's rule
(cod11-gsc-object-model.md 23.3). vcod-server's CoD2 playerstate carries
the objectives; without them the compass shows no sd bomb sites, ctf flags
or hq radio.

### 19.5 What else the stock game types draw

The 2D elements of the stock gametypes (`newHudElem`, `newTeamHudElem`,
`newClientHudElem` in `iw_07.iwd`) go through the 2D hudelem path
(cod2-fonts-menus.md 3). VERIFIED (the calls).

- Every game type's `level.clock` `setTimer`.
- `_hud_teamscore`/`_hud_playerscore`'s icons and scores.
- The respawn text and timer.
- ctf's carried-flag icon and its flash.
- hq's radio icon and its capture and neutralize bars
  (`setShader("white", w, h)` over a black back).
- sd's bomb stopwatch (`setClock(timeleft, level.bombtimer,
  "hudStopwatch", 48, 48)`) and its plant and defuse bars
  (`setShader("white", 0, 8)` then `scaleOverTime(planttime, barsize,
  8)`).
- The killcam's bars and texts.

`CG_Draw2dHudElems` (0x1e11b8). VERIFIED.

- A material element (type 6) and a clock (11 counting down to `time`, 12
  up from it) take `width` and `height`, each scaled from
  `fromWidth`/`fromHeight` over `scaleTime` past `scaleStartTime`.
- A size of 0 reads as the element's font height (`CG_GetHudElemInfo`'s
  `UI_TextHeight`, its +0x224), so sd's plant bar grows from 12, not 0.
- A clock draws its material, then `<material>needle`
  (`CL_RegisterMaterialNoMip`) turned by `CG_DrawRotatedPicPhysical` about
  the same rect's centre: `AngleNormalize360(t * 360 / duration)`, or
  `t * 0.006` (0x326644, a turn a minute) with no `duration`; `t` is the
  ms left (11) or gone (12), at least 0.

vcod draws the clocks and the scale (`hud::cod2elem`).

### 19.6 Crosshair names (`CG_DrawCrosshairNames`, 0x1cddb4)

- With `cg_drawCrosshairNames` (default 1, flags 0x1001) and not in the
  third person (`cg + 0x25bc0`): a capsule trace of no size from the
  refdef's view origin (`cg + 0x28588`) 8192 units (0x3265c8) along its
  forward axis (`cg + 0x28594`), passing `ps.clientNum`, mask 0x2000001.
  VERIFIED. An entity number under 64 there stores the client
  (`cg + 0x2bdcc`) and `cg.time` (`cg + 0x2bdd0`). VERIFIED.
- `CG_TraceCapsule` (0x1de958) is `CM_BoxTrace` on model 0, then, unless
  it stopped at 0, `CG_ClipMoveToEntities` (0x1de5b8) over
  `CG_BuildSolidList`'s entities: those with `solid` set, not `eType` 3
  (the trigger list), a brush model (`solid` 0xffffff) skipped with
  `eFlags & 1`. VERIFIED. A packed `solid` there is the box
  `mins = (1 - x, 1 - x, 2 - zd)`, `maxs = (x - 1, x - 1, zu - 33)`, with
  `x`, `zd`, `zu` its three bytes from the low one: the server's box a
  unit smaller each side. VERIFIED (the arithmetic). Its contents are
  0x2000000 for `eType` 1, else 1. VERIFIED.
- `CG_FadeColor(time, 150, 100)` (0x1a1b0e): nothing 150 ms past the last
  store, else alpha 1, falling linearly over the last 100. VERIFIED.
  Nothing either unless the viewer and the found client have client info,
  the viewer's team is not 0 and is 3 or the found client's, and his name
  is not empty. VERIFIED. The name is `I_CleanStr`ed and drawn in font 0
  at 1/3 at `(cg_drawCrosshairNamesPosX, PosY + 16)` (defaults 300 and
  180, flags 0x1000), aligned (0, 0), style 3. VERIFIED.
- The colour: when the found client is `ps.stats[3]`,
  `h = stats[4] / 100` clamped to 0..1 gives `(1, 2h, 0)` up to 0.5 and
  `(2 - 2h, 1, 0)` above it (0x326350 100, 0x326190 0.5); else white. The
  alpha is 0.6 (0x326500) of the fade's. VERIFIED. `CG_SetNextSnap`
  (0x1dee08) copies `snap + 0x144` and `+0x148`, `ps.stats[3]` and
  `stats[4]` (ps at `snap + 0xc`, `stats` at ps+0x12c), to `cg + 0x2bddc`
  and `+0x2bde0`. VERIFIED.
- `CG_Draw2D` (0x1d114a) calls it with the HUD menus shown, after the
  crosshair for `pm_type` under 6, after `CGAME_SPECTATOR` for 4; not for
  the dead or at intermission. VERIFIED.
- The server's half, 0x811c148, runs from `G_RunFrame` after every
  `ClientEndFrame` and before `CheckVote` (0x811c382: once a new
  `level.time`, for each connected client without `pm_flags` 0x400000).
  VERIFIED. A playing client (`sessionState` 0) in team 0 gets `stats[3]`
  -1 and `stats[4]` 0. VERIFIED. Otherwise: from `G_GetPlayerViewOrigin`,
  raised by `8 - viewHeightCurrent` for a client not playing whose
  `viewHeightCurrent` is under 8 (rodata 0x815eac8), a `G_TraceCapsule`
  8192 along `G_GetPlayerViewDirection`'s forward, mask 0x2000001, passing
  the client. VERIFIED. A hit entity under 64 with a client, while the
  viewer is not playing or shares its session team (`gclient + 0x274c`),
  stores its number and its health (entity +0x194); anything else stores
  -1 and 0. VERIFIED. `stats[3]`'s six bits send -1 as 63, so a lone
  spectator reads 63 (cod2-protocol.md 9). INFERRED (the netfield width).
- vcod-server runs the pass after its end frames (`game::cod2_identify`),
  with the bodies as their playerstate boxes and the world through the
  collision world; script models' xmodel collision does not stop it.
  vcod's client traces the prediction's collision world and the
  snapshot's packed boxes (`cod2_names`), leaving brush models out, and
  draws without the style 3 shadow.
