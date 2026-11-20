# vcod agent notes

vcod is a Rust client, dedicated server and map viewer for four Call of Duty
games. `docs/features.md` and its per-game pages (`docs/features/<game>.md`)
say what it does and doesn't do. This file holds what the code and configs
don't tell you: where the evidence lives, how to measure against retail, and
the rules every change follows.

Retail is the oracle. When vcod and retail disagree, retail is right, and the
answer goes into a research doc with the bytes.

| Game | Version | Protocol | Test env | Detected by |
|---|---|---|---|---|
| Call of Duty | 1.1 | 1 | `COD_DIR` | default |
| Call of Duty | 1.5 | 6 | `COD_DIR` | 1.5's `pak0` checksum |
| United Offensive | 1.51 | 22 | `COD_DIR` (`uo/` beside 1.5's `main/`) | `uo/pakuo*`, `--mod-dir uo` |
| Call of Duty 2 | 1.3 | 118 | `COD2_DIR` | `main/iw_*.iwd`, `CoD2MP_s.exe` |
| Call of Duty 4 | 1.7 only | 6 | `COD4_DIR` | `zone/`, `iw3mp.exe` |

The binaries read only `COD_DIR` (or `--game-dir`), whatever the game;
`COD2_DIR` and `COD4_DIR` point tests and tools at those installs. CoD1 1.5
and CoD4 1.7 both speak protocol 6: a wire is named by its game and version
together (`WireGame::id()`), never the version alone. Both binaries run all
three engines (CoD1, CoD2, CoD4); `--game cod1|cod2|cod4` overrides the
detection. CoD4 work rests on the 1.7 binaries (`objdump`, `tools/re`), live
captures, the research docs and the A/B gates.

## Layout

| Crate | Package | Owns |
|---|---|---|
| `crates/common` | `vcod-common` | what the games share: `mapworld`, CoD1 formats, collision and `clipmap/`, pmove, weapons, `asset_source`, `net/` (the CoD1 wires, `Transport`, `WireGame`, the field codec) |
| `crates/cod2` | `vcod-cod2` | CoD2 1.3 formats, wire, pmove, collision |
| `crates/cod4` | `vcod-cod4` | CoD4 1.7 fastfiles, assets, wire, pmove, collision |
| `crates/gsc` | `vcod-gsc` | the `.gsc` compiler and VM |
| `crates/server` | `vcod-server` | the dedicated server: `engine/` (`Engine<G: GameModule>`), each game's module, `game/` (the script host), bots and nav |
| `crates/client` | `vcod` | window, renderer, HUD, audio, console, front end, `play/`, `session.rs`, `probe.rs` |

The module map per crate is `docs/dev/layout.md`.

Check a file's size before reading it whole. Three sources pass 5k lines
(`main.rs` at 6.1k, `spectate.rs`, `renderer/cod2.rs`;
`git ls-files 'crates/*.rs' | xargs wc -l | sort -rn`): grep them. Research docs run from 14 KB to 630 KB, and thirteen pass 150 KB
(`ls -S docs/research`): jump by section.

Gitignored and local to one machine: `private/` (local reference material,
the retail 1.1d Linux server and its homepaths, old plans) and `tmp/`
(scratch; probe captures land there).

### Dependency rules

- `common` imports no wgpu, winit or kira.
- `cod2` and `cod4` depend on `common` only among vcod's crates, and on no
  wgpu, winit or kira either. `cargo deny check` (`deny.toml`, run in CI)
  enforces the wgpu, winit and kira half of these two rules.
- `gsc` depends on neither `common` nor the client: `cargo tree -p vcod-gsc
  -e normal` lists only `log`. `vcod-common` is a gsc
  dev-dependency for the corpus tests.
- Assets load by kind and name through `vcod_common::asset_source`
  (`load::<T>`). A reader that only reads takes `&dyn AssetSource`, not
  `&Pk3Fs`. Which kinds cache and who owns each: `docs/dev/assets.md`.

## Before you change code

Read the pointer that matches what you are doing:

| When you are about to | Read |
|---|---|
| touch wire, map cycle, movement, collision, combat, items, script, assets, rendering, sound or bots | `docs/dev/traps.md`, All games plus that game's section: the traps already paid for |
| move work inside a server frame, add a `GameModule` hook | `docs/dev/server-tick.md` |
| add an asset kind or change a loader's cache | `docs/dev/assets.md` |
| run an install's gates, an ignored test, a census, soak, perf or bot run | `docs/dev/testing.md` |
| run a retail server, write a probe or a fixture, add an A/B gate | `docs/dev/retail.md` |
| read a retail binary or port disassembly | `docs/dev/reverse-engineering.md` |
| run the binaries, read the F3 overlay, set debug env vars | `docs/dev/running.md` |
| find which module owns something | `docs/dev/layout.md` |
| learn a subsystem's retail behaviour | its research doc, picked from `docs/research/README.md` |

Player-facing docs: `README.md`, `docs/features.md`,
`docs/features/<game>.md` (`cod1`, `uo`, `cod2`, `cod4`: the feature and
limitation lists), `docs/getting-started.md`, `docs/usage.md` (flags),
`docs/bots.md`.

## Workflow

- Feature work happens on a branch in a worktree. Master is merge-only:
  `git fetch`, merge `origin/master` into the branch, then `git merge
  --no-ff` with `merge <branch>: <summary>`. Other sessions push to master
  too.
- Commit messages take conventional prefixes (`feat:`, `fix:`, `docs:`,
  `test:`, `perf:`, `refactor:`, `style:`, `chore:`).
- Before merging a feature or a fix, reread every line of AGENTS.md and the
  player-facing docs above that touches what the branch changed. Commit a
  fix on the branch for each one the work made stale or wrong. The README
  stays a short summary; detail goes in the other docs. The merge waits
  until all of them match the code.
- Comments: one line where the code doesn't explain itself (an invariant, a
  unit or sign convention, a workaround and what it works around). Facts that
  live in a doc stay there; the comment is the pointer. A doc comment moves
  with its item: `crates/common/tests/doc_placement.rs` fails on one left
  glued onto the next item's.
- Pixel and by-ear checks (aim sign, lean direction, shading, sound) belong
  to the human. Report them as pending.

### What CI runs

Before a commit, run what CI runs:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins --tests
cargo deny check   # after a dependency change
```

`--lib --bins --tests` leaves out the examples, about a GB of links that
clippy's `--all-targets` already compiles. `tools/gate.sh [area...]` runs
the first three once per install the environment names (`COD_DIR`,
`COD_UO_DIR`, `COD2_DIR`, `COD4_DIR`) and then the ignored gates of each
area; its header lists them.

- Set `COD_DIR` (absolute) for the suite, and `COD2_DIR` and `COD4_DIR` for
  their games. A test that needs an install passes vacuously when its
  variable is unset and panics when the path is relative or holds no
  install. `VCOD_REQUIRE_INSTALL` turns the skip into a failure.
- The suite is green under each install: `COD_DIR` at 1.1 runs the 1.1
  tests, at UO (`uo/` beside 1.5's `main/`) the UO ones.
- `ci.yml` runs on ubuntu with no game data. A new test needing paks returns
  early through `game_fs()` or uses `Pk3Fs::empty()`.
- Each crate's integration tests are one binary named after the crate
  (`--test common`, `--test server`; `tests/main.rs`): a new file needs its
  `mod` line. The client's tests live in its binary: `cargo test -p vcod
  --bins`.
- Some gates are `#[ignore]`d for their cost. Run the one for your area when
  you touch it (`tools/gate.sh nav`, `bots`, `render`, `cod2`, `cod4`).

Which fixture helper a test takes, which gates need which install, the
ignored gates with their commands and costs, and the nightly build:
`docs/dev/testing.md`.

## Evidence labels

Every claim in a research doc names the module, the virtual address and the
string or table it rests on, and carries its own label: VERIFIED when read
out of a binary, an asset or a live capture, which includes a branch
condition or instruction order read at a named address range ("VERIFIED
(the instructions)"); INFERRED for identity, intent, flow read without
addresses, and another game's or engine's source shape. No section-level
blanket. The full rule, document defaults and what
`crates/common/tests/evidence_labels.rs` checks: `docs/research/README.md`.

Research docs are cited by section number from code comments and other docs
(`combat doc 12.4`, `coduo-diff.md 16`). Keep numbered sections and file
names stable; extend a doc with what you learn instead of leaving the fact
in a commit message. Split a long section by adding numbered subsections
(12.4.1) under it, never by renumbering. `crates/common/tests/doc_citations.rs`
checks that every `<doc>.md N` citation names a heading and every quoted
paragraph name (`<doc>.md N, "The name"`) a heading, bold lead or
paragraph lead in that section, and that no doc gives two headings one
section number.

## Research docs

`docs/research/*.md` hold verified engine facts with binary addresses, and
`docs/research/README.md` indexes them by game and subsystem. File names
start with the game (`cod11-`, `cod15-`, `coduo-`, `cod2-`, `cod4-`,
`bot-`); CoD1's formats have their own (`bsp-ibsp59-format.md`,
`xmodel-v14-format.md`, `xanim-v14-format.md`,
`player-model-anim-system.md`, `clientstate-wire-format.md`,
`efx-grammar.md`). The 1.1 wire is `docs/protocol-1.1.md`; check it before
assuming Q3 semantics. Read the subsystem's doc before touching it.
