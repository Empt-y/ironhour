# Zero Hour in Rust: port plan

## Context

EA released the C&C Generals Zero Hour source (GPL v3 plus extra trademark terms) in 2025. The goal is a new Rust engine with a new renderer that plays Zero Hour from the original game data, runs existing mods unchanged, and gives modders more room than the 2003 engine did.

Decisions already made with you:

| Topic | Decision |
|---|---|
| Fidelity | Data-compatible. Same INI, maps, models, scripts and mods; deterministic against itself only. No retail replay or retail multiplayer compatibility. |
| Strategy | Fresh Rust workspace, built bottom-up, with the EA C++ as the behavioural reference. No C++ is compiled into the game. |
| Renderer | `wgpu`, faithful to the original look first; visual upgrades are opt-in. |
| First complete version | Skirmish vs AI on Windows, Linux and macOS. Campaign, Generals Challenge and multiplayer come after. |

What the survey found (sizes are estimates from the repo tree):

- About 0.75M lines of game-relevant C++. The simulation core that must be ported is roughly 300k lines: 205 gameplay modules, 19 draw modules, pathfinding (335 KB), AI states (430 KB), script engine (880 KB, 344 actions and 109 conditions).
- The renderer is Direct3D 8, mostly fixed-function, so it is replaced, not ported.
- The repo contains no game assets. Your install supplies them:
  - Zero Hour: `C:\Program Files (x86)\EA Games\Command & Conquer Generals Zero Hour\` (original 1.04 archives intact)
  - Generals: `C:\Program Files (x86)\EA Games\Command and Conquer Generals\`
  - Contra X Beta 2 is installed there as `.ctr` archives and becomes the first mod-compatibility target.
- Miles (audio), Bink (video), GameSpy and SafeDisc are proprietary and absent. Each needs an open replacement or is dropped.

This is a very large project. The plan is ordered so every milestone ends in something you can see or run, and so the mod-facing layers are proven on real data before the simulation is built on them.

## Ground rules

- **Licence:** the Rust code is a translation of GPL v3 code, so the project is GPL v3 with EA's additional terms carried over. No EA trademarks in the project name, and no game assets in the repo.
- **Working name and location:** `ironhour` at `C:\Users\theas\ironhour` (a placeholder; rename freely before the first commit).
- **Installs are read-only inputs.** The engine reads the two game folders in place (found through the registry `InstallPath`, overridable in a config file) and writes only to its own user-data folder.
- **Reference sources** are cloned next to the project under `reference/` and never built into it:
  - `electronicarts/CnC_Generals_Zero_Hour` (behaviour baseline, `GeneralsMD/` tree only)
  - `TheSuperHackers/GeneralsGameCode` (modern-compiler fork; source of known bug fixes)
  - OpenSAGE docs for format notes
- **Every ported Rust file names the C++ file it came from** in a header comment, so behaviour questions can be traced.
- **Verification is headless by default** (offscreen render to PNG, headless simulation runs), so tests do not steal focus while you are gaming. You do the in-game reviews at milestone ends.

## Architecture

One Cargo workspace. The simulation never depends on the renderer.

| Crate | Responsibility | Replaces (C++) |
|---|---|---|
| `ih-vfs` | Loose files, `.big` archives, mount priority, mod profiles | `Win32BIGFileSystem.cpp`, `ArchiveFileSystem.cpp`, `FileSystem.cpp` |
| `ih-ini` | INI lexer, block registry, field tables (derive macro), override layers | `Common/INI/*.cpp`, `INI.h` |
| `ih-formats` | W3D, map DataChunk, CSF/STR, WND, TGA/DDS, WAK, SCB, RefPack | `w3d_file.h`, `DataChunk.cpp`, `GameText.cpp`, `GameWindowManagerScript.cpp` |
| `ih-math` | `f32` vectors/matrices and software trig (`libm`), identical on every platform | WWMath, `Trig.cpp` |
| `ih-sim` | Game logic: objects, modules, scheduler, partition, pathfinding, AI, weapons, players | `GameLogic/`, most of `Common/` |
| `ih-script` | Map script engine and skirmish AI scripts | `ScriptEngine/` |
| `ih-render` | `wgpu` renderer: W3D materials, terrain, water, shadows, particles, shroud, 2D | WW3D2, `W3DDevice/` |
| `ih-client` | Drawables, model condition states, camera, input translation, in-game UI | `GameClient/` |
| `ih-ui` | WND window system, gadgets, control bar, menus | `GameClient/GUI/` |
| `ih-audio` | Sound, speech, music (`kira` + `symphonia`) | `MilesAudioManager.cpp` |
| `ih-modapi` | Mod manifest, extended data features, Lua module host | new |
| `ih-game` | The game binary (`winit` window, main loop) | `WinMain.cpp`, `GameEngine.cpp` |
| `ih-tools` | CLI: archive listing, INI lint, coverage report, model/map viewer, headless sim runner | new |

Key design choices:

- **Simulation shaped like the original.** Objects own an ordered list of behaviour modules created from INI by name, a 30 Hz logic tick, and the same "sleepy update" scheduler with its four phases. This is a direct translation target (not an ECS redesign), because gameplay feel depends on update order.
- **Logic and rendering decoupled.** The original advances logic at most once per rendered frame. Here the simulation ticks at a fixed 30 Hz and the client interpolates, so frame rate is unlimited.
- **Self-determinism from day one.** `f32` only, software transcendentals, no iteration over hash maps in the simulation, one seeded logic RNG (ported exactly from `RandomValue.cpp`). A state hash every N frames makes desyncs testable, which later multiplayer and replays depend on.
- **Simulation reads assets without the renderer.** Projectile launch points, dock and exit positions come from W3D skeleton bones, and ground height comes from the map heightmap. Both are loaded through `ih-formats` so headless runs work.
- **Open registries instead of compiled enums.** KindOf, model conditions, damage types, weapon-bonus conditions, upgrades and module names live in registries seeded with the original names in the original order. Stock data parses identically, and the old caps disappear (see "Modding expansion").

## Mod compatibility surface

These rules are reproduced exactly, with tests:

- **File priority:** loose files, then `-mod` archive or folder, then Zero Hour archives in case-insensitive alphabetical order (first one wins, which is why `!`-prefixed archives override), then base Generals archives.
- **INI:** all 62 block types; case-sensitive field names; the same unit conversions (milliseconds to frames at 30 Hz, angles, velocities); `Default\*.ini` then main file then folder load order; `AddModule` / `RemoveModule` / `ReplaceModule`; `ObjectReskin`; per-map `map.ini`, `solo.ini` and `map.str` overrides.
- **Modules:** every one of the 205 behaviour and 19 draw module names resolves. Until a module is implemented it loads as a logged stub, never a parse failure.
- **Maps:** DataChunk container, all compression variants, height and blend tiles, objects, waypoints, polygon triggers, sides, teams, scripts.
- **Scripts:** actions and conditions matched by internal name as the original does; unknown ones become no-ops.
- **GUI:** `.wnd` layouts, with callbacks resolved through a Rust table keyed by the original C++ function names.
- **Mod profiles:** a launcher-level description of which archives to mount (covers Contra's `.ctr` scheme without renaming files in your install).

A **coverage tool** (`ih-tools coverage`) scans whatever is mounted and reports which block types, fields, modules and script actions the data uses versus what is implemented. It decides porting order and is the measure of mod compatibility.

## Milestones

Each ends with a check you can run or look at.

**M0. Foundations**
- Create the workspace, licence files, CI (Windows, Linux, macOS), reference clones, install locator.
- `ih-vfs` with BIG reading and the priority rules; `ih-tools ls` and `extract`.
- Check: lists every file in both installs; the resolved winner for a sample of overridden paths matches the rules above.

**M1. Data layer**
- `ih-ini` with all 62 block types parsed into typed data; CSF/STR strings; mapped images; coverage tool.
- Check: stock Zero Hour and Contra X data parse with zero errors; coverage report generated for both.

**M2. Model viewer**
- `ih-render` core, W3D meshes, hierarchies, animations (raw and compressed), HLOD, textures, fixed-function material emulation in WGSL, house colour.
- Check: offscreen renders of a fixed list of units and buildings; you compare a handful against the real game.

**M3. Map viewer**
- Map loading, terrain with blend tiles and cliffs, roads, bridges, water, trees and props, placed objects, lighting, tactical camera.
- Check: stock skirmish maps render; fly-through in a window.

**M4. Simulation core**
- Object and module framework, scheduler, partition manager, terrain logic, pathfinding, locomotors, physics, AI state machine, command pipeline, selection and move orders. Drawables with condition-state animation.
- Headless runner and determinism harness.
- Check: units path across a map on command; the same command log produces the same state hashes on all three platforms.

**M5. Combat and economy**
- Weapons, armour, bodies, death, projectiles, object creation lists, particles and FX.
- Construction, production, supply gathering, power, upgrades, sciences, special powers.
- Garrisons, transports, tunnels; stealth and detection; shroud and fog; radar; veterancy; crates.
- Check: scripted headless battles finish deterministically; a hand-played sandbox match with no AI.

**M6. Interface and audio**
- WND system, control bar, hotkeys from `CommandMap.ini`, cursors, EVA, sound, speech, music.
- Menus needed for skirmish: main menu, skirmish setup, options, load screen, score screen.
- Check: start a skirmish from the menu and play it with full UI and sound.

**M7. Script engine and skirmish AI (first playable)**
- SCB loading, script actions and conditions in the order the coverage tool reports for `SkirmishScripts.scb` and the stock maps, AI player build lists and teams, victory conditions.
- Check: complete skirmish games against every AI general at each difficulty.

**M8. Mod hardening and release**
- Drive coverage to 100% for stock data, then Contra X; add ShockWave and Rise of the Reds if you install them.
- Own-format save games and replays; packaging for the three platforms.
- Check: a full Contra X skirmish; replay of a recorded match reproduces its state hashes.

**M9. Modding expansion** (designed in from M1, switched on here)
- See next section.

**After the first complete version:** campaign and Generals Challenge (needs the remaining script actions and Bink playback through an optional FFmpeg feature), multiplayer (lockstep on the existing determinism, with a new lobby and relay), and a map editor.

## Modding expansion

Old mods run unchanged. New features are opt-in through a `mod.toml` manifest (name, version, dependencies, load order, feature level), so nothing changes for data that does not ask for it.

- **Caps lifted:** weapon-bonus conditions (32-bit limit), upgrades (128), build cost (65,535), command-set size (18), terrain texture classes (256), script counters and flags (256).
- **Data-defined enums:** mods can declare new KindOf values, damage types, model conditions, armour and weapon set conditions.
- **INI quality of life:** `#include`, `#define`, template inheritance, precise error messages with file and line, and a strict lint mode.
- **New asset formats alongside the old:** glTF models, PNG and KTX2 textures, OGG audio, bundled TTF fonts.
- **Scripted modules:** new behaviour modules, script actions and UI callbacks written in sandboxed Lua with a deterministic standard library.
- **Renderer options:** shadow maps, PBR materials for assets that supply them, UI scaling for modern resolutions.
- **Hot reload** of INI, textures and models in a running game.

## Main risks

| Risk | Mitigation |
|---|---|
| Sheer volume of gameplay code | Coverage-driven order; stubs keep data loading; each milestone is playable in some form |
| Gameplay "feel" drifting (pathing, AI, timing) | Keep the original structure and update order; port file by file with source references; your side-by-side reviews at milestones |
| Fixed-function material look | Start M2 with a test list covering every blend mode and texture mapper used by stock data |
| Skirmish AI depends on hundreds of script actions | Coverage tool limits M7 to what skirmish scripts use |
| Video and fonts | Skirmish needs no video; fonts use `GenArial.ttf` from your install or a bundled open fallback |

## Verification

- `cargo test` everywhere. Tests needing game data read `IH_ZH_INSTALL` / `IH_GENERALS_INSTALL` and are skipped when unset, so CI stays asset-free.
- `ih-tools coverage` against stock and Contra X data after every milestone; the numbers go in `PORTING.md`.
- Golden offscreen renders for models and maps (M2, M3), compared by image diff.
- Headless simulation runs from recorded command logs, with state hashes compared across runs and across platforms (M4 onward).
- Your in-game review list at the end of each milestone.

## First steps on approval (M0)

1. Create `C:\Users\theas\ironhour`, `git init`, add GPL v3 plus EA's additional terms, and the workspace skeleton.
2. Clone the two reference repos into `reference/` (git-ignored).
3. Implement `ih-vfs` and `ih-tools ls` / `extract`, tested against your install.
4. Start `ih-ini` with a generic block parser so the coverage report exists early.
