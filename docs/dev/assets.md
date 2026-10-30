# Loading assets

Every asset loads by kind and name through `vcod_common::asset_source`:
`load::<T>`, one `Asset` impl per type. A reader that only reads takes
`&dyn AssetSource`, not `&Pk3Fs`. Scripts, `default_mp.cfg`, `.stype`
templates and other text read as `RawFile`.

Kinds so far: weapons, localized strings, sound aliases, raw files, images,
CoD1's shader library, CoD2's compiled materials, xmodels, xanims, effects,
impact tables, menu files, maps. CoD4's source is `vcod_cod4::source`
(`Cod4Source`): zones over the iwds.

## Caching and the referenced flag

A load is cached until `Pk3Fs::clear_references` (or an `overlay`) drops the
cache, so within a level only its first load marks the pak it read.

| Kind | Cached | Who owns it |
|---|---|---|
| `Image` (by path, extension included) | no, builds through `Asset::find` | the client's texture caches. `assets::DecodedImages` keeps `&Pk3Fs` for its unflagged prefetch and `mark_read`. |
| `ShaderLib::load` | yes, one parse per source; a `.shader` read marks no pak | |
| `vcod_cod2::materials::load_lib` (`Compiled`) | no | the renderer's `MaterialLib` |
| `Model` (v14 or v20, by its `xmodel/` name; `model::load` is `Model::find`) | no | the client's model caches, the server's hit rigs. The renderer's `upload_dynamic_model` and the multicore prop load keep `&Pk3Fs`. |
| `XAnim` (CoD1's or CoD2's, by its `xanim/` name; `xanim::load` is `XAnim::find`) | no | every reader keeps its own clip cache (`xanim::load_cached`, `PlayerAnimSet`, `HitRigs::clip`, the view weapons) |
| `fx::efx::Effect` (by `fx/` path; a CoD2 source parses through `vcod_cod2::efx::Effect`, kind `Cod2Effect`), both games' impact tables | no | `FxSystem`, the impact table static, `Cod2Fx` |
| `ui_menu::MenuFile` (by path, parsed in the source's game's grammar, `#include`s read) | no | the UI and both HUDs |
| CoD1 map `MapWorld` (`mapworld::find`: `maps/` then `maps/mp/`, as `Pk3Fs::resolve_map`) | no | the level |
| CoD2 map `vcod_cod2::map::MapFile` (kind `Cod2Map`, `maps/mp/<name>.d3dbsp`; `LoadedMap::find` builds both halves) | no | the level |

Exceptions that keep `&Pk3Fs`:

- `ui_menu::game_types`, for `file_list`'s search-path order.
- The client's CoD2 window load (`load_cod2_map`) reads the bytes itself for
  `Com_LoadBsp`'s error and the parse's panic guard.

A CoD2 `.d3dbsp` is parsed once (`MapFile`) into the `MapWorld` and the
`ClipMap`, built side by side (`LoadedMap`). `ClipMap::build` reads
`ClipLumps`, never the `D3dBsp`. The server's collision and combat and the
client's prediction share one `Arc<ClipMap>` per load.

The read-ahead and referenced-flag rule for CoD2 loads is in
[traps.md](traps.md#assets).
