# vcod client test fixtures

The `cod4_*_pins.txt` files are pinned output of vcod's own CoD4 HUD
drawing, not retail recordings. Each is the digest its test built from the
module before a rewrite; the test compares the whole file byte for byte, so
a recipe header would fail it.

| File | Written by | Read by |
|---|---|---|
| `cod4_crosshair_pins.txt` | `CROSSHAIR_PIN_DUMP=<path>` on the test | `src/cod4_crosshair.rs`, `the_crosshair_draws_its_pinned_quads` |
| `cod4_damage_pins.txt` | `DAMAGE_PIN_DUMP=<path>` on the test | `src/cod4_damage.rs`, `a_scripted_life_draws_its_pinned_overlay_and_icons` |
| `cod4_grenades_pins.txt` | the test's digest; it has no dump variable | `src/cod4_hud/grenades.rs`, `the_indicators_draw_their_pinned_quads` |

The `retail-client-*.txt` files are cut from retail client captures
(`docs/dev/retail.md`, "Retail client captures"); each names its capture
in a `#` header.

| File | Capture | Read by |
|---|---|---|
| `retail-client-intermission.txt` | 2026-11-24 `2-intermission` (1.1) | `src/hud/mod.rs`, `the_intermission_capture_draws_no_hudelems` |
