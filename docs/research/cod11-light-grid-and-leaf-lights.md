# The light grid and leaf lights (lumps 19, 30, 32)

What the "light grid" actually is in CoD 1.1 MP, what lights entities, and
why per-vertex grid sampling is not a retail mechanism. Evidence from
`CoDMP.exe` 1.1 (image base `0x00400000`); addresses virtual, xref sweeps
run against the whole `.text`. The earlier guesses in
`bsp-ibsp59-format.md` ("72-byte lights", "128x128x64x3 grid, not decoded")
are corrected here.

## Lump 32 is a per-cell light table, not an RGB grid

The fixed `0x300000` length decomposes as **262144 cells x 48 bytes**:
262144 = 128x128x64 exactly, so the lattice dims are right but the "3 bytes
per texel" reading was wrong - every cell carries a 48-byte record on every
map (the load only accepts that exact length, `FUN_004b7c00` @ `0x4b7c00`,
gate `param_3 == 0x300000`).

`FUN_004b7c00` walks all 65536 x 4 records (stride `0x30`) and distills
each into a 32-byte ring entry near `DAT_00ca28d4`: it samples a handful of
byte/u16/u32 fields per record, counts the non-zero ones into
`DAT_00ca27b0`, and tracks the high-water ring index in `DAT_00ca27ac`
(`& 0x1f`). The record layout itself stays undecoded - see the negative
result below for why it does not matter.

The compile can be re-run from a 24-byte-record light file through
`FUN_004b7ab0` @ `0x4b7ab0` (it ends by flushing the cvar
`r_vc_compile`), which is where the ring globals are otherwise touched.

## Negative result: nothing in MP samples the grid

An xref sweep over the whole `.text` for the three output globals
(`DAT_00ca27b0`, `DAT_00ca27ac`, `DAT_00ca28d4`) finds only writers inside
the compile path itself - `FUN_004b7b47`/`FUN_004b7c00` and the
`r_vc_compile` recompile. **No code reads the compiled rings.** The grid
machinery is Singleplayer inheritance: MP ships the data and the compiler,
but no consumer. Sampling lump 32 per vertex would therefore be invented
behaviour against data retail ignores - and there is no coordinate mapping
(origin/cell size) to copy, because MP never needs one.

## Lump 30: 48-byte lights, not 72

`R_LoadLights` (`FUN_004db240` @ `0x4db240`) rejects the lump unless the
length divides by `0x30` - the records are **48 bytes**. The census gcd was
72 because every stock length also divides by 144 (lcm of 48 and 72), so
both fit; retail's own check settles it. The loader copies fields [0..5]
and [7] verbatim into a runtime struct and resolves field [6] as
`base + index * 0xc` (a per-light list). Per-light runtime records are 32
bytes (`uVar3 << 5` allocation).

## Lump 19: leaf -> light indices (unchanged)

`u16` indices into lump 30, stored per leaf (`first_light_index` /
`light_index_count`), enforced by "R_LoadNodesAndLeafs: too many lights in
leaf" (`0x54d421`, xref `0x4db533`). This leaf-light wiring is the MP
entity-lighting path the `r_showLeafLights` cvar debugs.

## `lightingPrecalc` is the static-model lighting

The client parses the entity lump itself during LoadMap
(`FUN_004dbf50`, called with the entities lump at `LoadMap`), and the
`lightingPrecalc` string (VA `0x54d2fc`) is read by the entity-var getter
callers at `0x4dbc71` (the light/flare spawn parser `FUN_004dbcd0`) and
`0x4f8e25` (the shader/vertex-program path). Static models are baked world
geometry lit by this compiler-generated per-entity tint - which is exactly
what vcod already applies (`props.rs`), with a white fallback where the key
is absent, matching the getter's default.

## Consequences for vcod

- Per-vertex grid sampling: **rejected** - no retail mechanism implements
  it in MP (the sampler does not exist), and the data it would need from
  the grid (a world mapping) is never computed by retail MP.
- Static props keep `lightingPrecalc`; that is full parity already.
- The one real lighting gap left is **dynamic entities** (player models):
  retail MP can shade them from their leaf's lump-30 lights via lump 19.
  That is a separate, well-scoped follow-up ("entity lighting from leaf
  lights") and needs the 48-byte light record decoded from `FUN_004db240`
  first.
