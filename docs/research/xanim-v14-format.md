# CoD 1 xanim format (version 14)

The base byte layout of a CoD 1 animation clip, verified against the ten `xanim/viewmodel_kar98mp_*` clips and then against every `xanim/*` entry in the shipped pk3s: all 2957 decode to exactly EOF. The parser is `crates/common/src/xanim.rs`; `crates/common/src/skeleton.rs` applies the sampled keys to a pose. The format has no length prefixes, so landing exactly on the last notetrack byte is the oracle every layout rule below was checked with. This document covers the flag-0 layout; the three flagged variants (loop, delta, both) and the frame-index widening are decoded in `player-model-anim-system.md` under "xanim v14 header flags" and are only summarized here. Everything is VERIFIED unless labelled otherwise. Multi-byte values are little-endian; `cstr` is a NUL-terminated string.

## Layout (flags == 0)

```
u16 version                 // 14
u16 frame_count             // >= 1; 161 is the largest viewmodel clip, 256 the largest u8-indexed clip
u16 bone_count
u8  flags                   // 0 plain, 0x1 loop, 0x2 delta (root motion), 0x3 both
u16 framerate               // 30 for every viewmodel clip
u8[ceil(bone_count / 8)]    // bitset A: role unknown, layout-neutral, skipped
u8[ceil(bone_count / 8)]    // bitset B: bit i set = bone i uses "simple" one-component rotation keys
bone_count x cstr           // track names, e.g. "tag_torso", "kar98_bolt"; bit index order matches
per bone, in name order:
  u16 rot_count             // <= frame_count
  if 1 < rot_count < frame_count: u8 frame_index[rot_count]   // ascending
    // rot_count == frame_count: implicit indices 0..frame_count-1
    // rot_count <= 1: constant or empty track, no index list
  rot_count x key:
    simple bone: i16 z                  // q = (0, 0, z / 32768, w)
    full bone:   i16 x, i16 y, i16 z    // q = (x, y, z) / 32768, w derived
  u16 trans_count           // <= frame_count, same index-list rule
  trans_count x f32[3]      // local bone position offset, see below
u8 note_count               // notetracks ("fire", "reload done", ...)
note_count x { cstr name; u16 frame }
```

- Dequantization matches `xmodelparts`: `q_xyz = v / 32768.0`, `q_w = sqrt(max(0, 1 - x^2 - y^2 - z^2))`. The clamp is required (largest observed `|xyz|^2` is 1.00003).
- Simple bones key a single i16 that is the quaternion's Z component. `skeleton.rs` pins the axis: read as Z, the `viewmodel_kar98mp_idle` finger keys equal the hands' `xmodelparts` bind locals exactly (0.0 degrees apart), while an X reading lands 6 to 114 degrees off. In the 48-bone kar98mp clips the simple bones are the forearms, every finger bone and `kar98_bolt`, all single-axis rotators.
- The parser refuses any flag value above 3; none occurs in the shipped corpus.

## Flag extensions, in brief

Decoded in `player-model-anim-system.md`. Flag `0x1` (loop) stores `frame_count + 1` key positions, the extra one being the loop-closing repeat of frame 0; vcod folds it into `XAnim::frame_count` so key indices stay in `0..frame_count` and `duration()` is the loop period. Flag `0x2` (delta, root motion) inserts one nameless track between the header and the bitsets, shaped like a bone track with yaw-only i16 rotations; vcod keeps it as `XAnim::root`, which only the mounted gunner's body placement reads (`cod11-turrets.md` section 7). Flag `0x3` composes both. Independently of the flags, the sparse frame-index list is u8 while the highest index fits a byte and u16 above that (`frame_count > 256` in the parser, counting the loop key).

## Timing

Duration is `(frame_count - 1) / framerate`, which reproduces the weapon-file timings: `viewmodel_kar98mp_fire` has 11 frames, 0.333 s, matching `fireTime 0.33`; `ADS_up` has 10 frames, 0.3 s, matching `adsTransInTime`. Looping playback wraps over `frame_count - 1`; one-shots clamp to the last frame and hold their end pose.

kar98mp clips (frames / tracks): idle 1/48, fire 11/48, lastshot 11/48, rechamber 32/48, reload 78/48, ADS_up 10/1, ADS_down 17/1, pullout 10/48, putaway 11/48. The two ADS clips animate only `tag_torso`, which moves the grafted gun along with the hands (see `xmodel-v14-format.md`).

## The sight layer

The hip position of a viewmodel is not its bind pose. Every `adsUpAnim` clip opens and every `adsDownAnim` closes on the hip placement of `tag_torso`, and `adsUpAnim` closes on the sight placement: for `kar98k_mp` `tag_torso` translates by (5.98, -3.10, -3.39) at `viewmodel_kar98_ADS_up` frame 0 and `viewmodel_kar98_ADS_down` frame 12, and by (3.66, -0.26, -0.51) at `ADS_up` frame 10 (VERIFIED, both clips). No idle, fire, reload or raise clip of a rifle, pistol or submachine gun keys `tag_torso` at all; the stock grenades key it in every clip and ship no ADS clips, and `panzerfaust_mp` keys it in `altDropAnim` only (VERIFIED, every `weapons/mp/*` file's named clips). So a rig that draws only the main clip leaves `tag_torso` at its zeroed bind, which holds the gun 6 units behind the hip and 3 up and left, the receiver in the camera's face.

In `cgame_mp_x86.dll` (1.1) the viewmodel anim tree is created with 21 nodes (`0x30034cf0`, trap `0x84` on the `"VIEWMODEL"` tree). The function at `0x30034240` sets a goal weight (trap `0x8c`) of 1 on node `0x13` and 0 on node `0x14`, or the reverse, and then sets node `0x13`'s time (trap `0x91`) to the value at `0x30207214` and node `0x14`'s to 1 minus it (VERIFIED, the immediates). `0x30207214` is the interpolated playerstate float at `+0xc4`, the one the fov function at `0x30032e20` blends `adsZoomFov` with, so `fWeaponPosFrac` (INFERRED). Its one caller at `0x3003448a` passes `0x13` when `pm_flags` bit `0x20` is set and `0x14` otherwise, with a weaponstate-5 case that forces `0x14` while `weaponTime` exceeds a weapon-def field (INFERRED, branch conditions). Read together, nodes `0x13` and `0x14` are the up and down sight clips, one of them always at full weight with its time slaved to the fraction (INFERRED), which puts the hip at `adsDownAnim`'s last frame. `ViewWeapon::pose_sight` in `crates/client/src/viewmodel.rs` poses that layer before the main clip each frame; it picks the up clip off the fraction's trend rather than the flag.

## Sampling and the translation-key gotcha

`Track::sample` lerps positions and slerps rotations between the bracketing keys, clamps outside the keyed range, and returns nothing for a channel with no keys, in which case the bone holds its previous local. Rotation keys replace the local rotation outright.

Translation keys are offsets from the bind local position, not absolute locals. The shipped player clips prove it: they key `bip01 pelvis` with small values (-2.2 idle, -22.3 crouched, -31.0 prone) against a bind local z of 37.23. Read as absolutes those collapse the whole stance about 39 units into the ground; read as offsets they put `bip01 l/r toe0` within about 1 unit of z = 0 across `pb_stand_alert`, `pb_combatrun_forward_loop`, `pb_crouch_alert`, `pb_crouch_run_forward` and `pb_prone_aim`, four visibly different stances. Every other translation-keyed player bone (spine, clavicles, toe0, fingers) reads the same way, as fractions of the bone's bind length. Viewmodel rigs hide this because their bind local positions are all exactly zero (checked across every shipped `viewmodel_*` model; the viewhands quirk in the xmodel doc is why), so offset and absolute are the same number there, which is how the absolute reading survived the viewmodel work. `PoseBuffer::apply` does `local_pos = bind_pos + key`.

Notetracks are parsed for EOF validation and have no runtime consumer yet.
