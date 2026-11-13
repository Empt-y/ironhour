# Porting progress

Milestones are defined in the project plan. Numbers below come from `ih-tools`
runs against a local install. No game data is stored in this repository.

## M0. Foundations (done)

- Workspace, licence, CI for Windows, Linux and macOS.
- `ih-vfs`: `.big` reader and the original's file priority rules, ported from
  `Win32BIGFileSystem.cpp`, `ArchiveFileSystem.cpp`, `ArchiveFile.cpp` and `FileSystem.cpp`:
  - loose files beat archives;
  - archives mount per folder, recursively, in case-insensitive path order;
  - first archive wins, except `-mod` archives, which overwrite (so within a mod
    folder the last one wins);
  - archive directory listings always recurse;
  - extension-less files inside archives cannot be opened.
- Mod profiles (`profiles/*.toml`), covering Contra's selectively enabled `.ctr` packs.
- `ih-ini` (started early): exact port of `INI::readLine`, `strtok` tokenising, and a
  structural parser with the nested-block table (`crates/ih-ini/src/structure.rs`).
- `ih-tools`: `installs`, `archives`, `ls`, `which`, `extract`, `cat`, `scan`.

### Results

| Data | Archives | Files | INI files | INI lines | Top-level blocks | Module classes used | Structure problems |
|---|---|---|---|---|---|---|---|
| Stock Zero Hour + Generals | 38 | 25,303 | 222 | 569,871 | 14,017 | 205 | 0 |
| Contra X Beta 2 | 52 | 37,888 | 460 | 1,178,304 | 31,626 | 203 | 0 |

Mounting takes about 0.1 s and a full INI scan about 0.35 s (release build).

### Findings to carry forward

- `Data\INI\Default\ButtonSets.ini` ships in both games' archives but no code loads it
  (there is no `ButtonSet` block). `scan` skips it until the load-order manifest exists.
- Both installs contain an empty `launcher\MapsZH.big`. The original's recursive search
  mounts it ahead of the root `MapsZH.big`; harmless because it is empty.
- Duplicate `CommandSet` names are a fatal error in the original (`ControlBar.cpp`),
  while duplicate `Object`s parse on top of each other. To be decided in M1 whether to
  keep the error or downgrade it to a warning. (Still open: decided when definitions are
  built into templates in M4.)

## M1. Data layer (done)

- **Schema from the source.** `ih-schemagen` reads the original C++ and generates
  `crates/ih-ini/src/schema/generated.rs`: 285 field tables, 3,093 field definitions,
  all 223 module names resolved through class inheritance, and 121 name lists (including
  those in the WWVegas libraries). Regenerate with
  `cargo run -p ih-schemagen -- reference/ea-zh/GeneralsMD/Code crates/ih-ini/src/schema/generated.rs`.
- **Field validation.** Every field resolves to its exact C++ definition, following
  `initFromINIMulti` (tables in order, first match wins, catch-all entries).
- **Values** (`crates/ih-ini/src/values.rs`). Ports of the generic `INI::` parsers and about
  60 specialised ones, with the original's tokenising (`strtok` separator sets, quoted
  strings), `sscanf`-style numbers, and unit conversions (milliseconds to whole frames,
  degrees to radians, per-second to per-frame). File-local `static` parsers are matched
  by source file, because several files reuse names like `parseOCL`. Original quirks are
  kept, e.g. `parseDebrisObjectNames` skips every second name.
- **Load order** (`crates/ih-ini/src/load_order.rs`). The exact startup sequence from
  `GameEngine::init` and each subsystem's `init()`, including that GameData loads before
  `-mod` archives are mounted, files loaded on demand, and per-map `map.ini`/`solo.ini`.
  `ih-tools load-order` prints it with the archive each file comes from.
- **Strings** (`crates/ih-formats/src/strings.rs`). CSF and text STR tables, with label
  lookup, `map.str`, speech names and escape handling. `ih-tools strings` looks up labels.
- **Coverage** (`ih-tools coverage`). Reports are in `docs/coverage/`.

### Results

| Data | INI files loaded | Fields | Unknown fields | Values parsed | Invalid values | Undefined references |
|---|---|---|---|---|---|---|
| Stock Zero Hour | 215 | 386,620 | 0 | 99.7% | 0 | 60 |
| Contra X Beta 2 | 453 | 847,388 | 0 | 99.9% | 0 | 196 |

The remaining values use 34 to 36 rarely used parsers (under 1,000 fields), listed in the
coverage reports. Script actions and conditions live in map files and are covered in M7.

### Findings to carry forward

- Files that ship but are never loaded: `ButtonSets.ini` and `ControlBarResizer.ini`
  (the code that loads it is commented out), `Default\Webpages.ini`, and the
  debug/demo-only `GameDataDebug`, `CommandMapDebug`, `CommandMapDemo`.
- Contra's `Data\INI\DefenseTowers.ini` is never loaded, so `AmericaBuildingDefenseLaser`
  and `AmericaBuildingDefenseGunHigh` do not exist in the game.
- Contra uses a text `Data\Generals.str`, which the engine prefers over the CSF.
- Stock data has real dangling references the original tolerates as null, e.g. an
  `InstantDeathBehavior` with `FX = FINAL ...` (a phase word copied from `SlowDeath`),
  `OCL = INITIAL OCL_`, and a missing `GC_Nuke_ChinaCarpetBomb` payload.
- Under Contra, stock campaign `map.ini` files name command buttons Contra renamed. The
  original treats an unknown button in a `CommandSet` as a fatal error, so those
  campaign maps would not load with Contra in either engine.

## M2. Model viewer (next)

W3D meshes, hierarchies and animations, textures, and the `wgpu` renderer core, checked
with offscreen renders of a fixed list of units and buildings.
