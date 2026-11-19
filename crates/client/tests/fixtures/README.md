# vcod client test fixtures

Pinned output of vcod's own CoD4 HUD drawing, not retail recordings. Each
file is the digest its test built from the module before a rewrite; the
test compares the whole file byte for byte, so a recipe header would fail it.

| File | Written by | Read by |
|---|---|---|
| `cod4_crosshair_pins.txt` | `CROSSHAIR_PIN_DUMP=<path>` on the test | `src/cod4_crosshair.rs`, `the_crosshair_draws_its_pinned_quads` |
| `cod4_damage_pins.txt` | `DAMAGE_PIN_DUMP=<path>` on the test | `src/cod4_damage.rs`, `a_scripted_life_draws_its_pinned_overlay_and_icons` |
| `cod4_grenades_pins.txt` | the test's digest; it has no dump variable | `src/cod4_hud/grenades.rs`, `the_indicators_draw_their_pinned_quads` |
