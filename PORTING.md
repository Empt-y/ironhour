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
- **Values** (`crates/ih-ini/src/values.rs`). Ports of every value parser in the schema (the
  generic `INI::` ones and about 100 specialised module, draw and UI parsers), checked by a
  test that fails if a regenerated schema adds one that is not ported. They keep the
  original's tokenising (`strtok` separator sets, quoted strings), `sscanf`-style numbers, and unit conversions (milliseconds to whole frames,
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
| Stock Zero Hour | 215 | 386,620 | 0 | 100% | 0 | 69 |
| Contra X Beta 2 | 453 | 847,388 | 0 | 100% | 0 | 207 |

Script actions and conditions live in map files and are covered in M7.

### Findings to carry forward

- Files that ship but are never loaded: `ButtonSets.ini` and `ControlBarResizer.ini`
  (the code that loads it is commented out), `Default\Webpages.ini`, and the
  debug/demo-only `GameDataDebug`, `CommandMapDebug`, `CommandMapDemo`.
- Contra's `Data\INI\DefenseTowers.ini` is never loaded, so `AmericaBuildingDefenseLaser`
  and `AmericaBuildingDefenseGunHigh` do not exist in the game.
- Contra uses a text `Data\Generals.str`, which the engine prefers over the CSF.
- `TransitionDamageFX`'s OCL parser passes its arguments in the wrong order, so the OCL
  pointer overwrites the effect's location and the OCL itself stays null: transition OCLs
  never fire in the original. The value is parsed normally; the simulation must keep it inert.
- Stock data has real dangling references the original tolerates as null, e.g. an
  `InstantDeathBehavior` with `FX = FINAL ...` (a phase word copied from `SlowDeath`),
  `OCL = INITIAL OCL_`, and a missing `GC_Nuke_ChinaCarpetBomb` payload.
- Under Contra, stock campaign `map.ini` files name command buttons Contra renamed. The
  original treats an unknown button in a `CommandSet` as a fatal error, so those
  campaign maps would not load with Contra in either engine.

## M2. Model viewer (done)

- **W3D reader** (`crates/ih-formats/src/w3d.rs`): meshes (all prelit material variants,
  per-pass vertex materials, shaders, per-vertex colours, texture stages), hierarchies,
  raw and compressed animations (time-coded and adaptive-delta, decoded as
  `motchan.cpp` does), HLODs, boxes. Malformed objects are skipped and reported, the rest
  of the file still loads.
- **Textures** (`crates/ih-formats/src/texture.rs`): Targa (true-colour, grey,
  colour-mapped, RLE) and DDS (DXT1/3/5 and uncompressed). A `.tga` name loads the
  `.dds` of the same name first, searching `Data\<language>\Art\Textures\` then
  `Art\Textures\`.
- **Renderer** (`crates/ih-render`, wgpu): fixed-function emulation of WW3D2 on
  Direct3D 8:
  - Direct3D-style per-vertex lighting with GameData's object lights;
  - colour sources from `meshmatdesc.cpp`: vertex colours pre-multiplied, prelit meshes
    unlit;
  - texture stage 0 primary gradient and stage 1 detail colour/alpha functions;
  - alpha test (0x60) and the blend and depth states from `shader.cpp`;
  - two-sided meshes, opaque-then-translucent ordering, 4× MSAA.
- **Texture mappers**: environment (camera and world space), cheap environment, linear
  offset (scrolling), scale, grid (flipbook) and rotate.
- **Poses and animation** (`pose.rs`): hierarchy poses as `htree.cpp` builds them,
  rigid and skinned meshes, interpolation as `hrawanim.cpp`/`hcanim.cpp`.
- **House colour**: `HOUSECOLOR` meshes take the colour as material ambient and diffuse.
  `ZHCA` textures get the original's HSV hue shift on pixels whose alpha is below 255;
  `ZHCD` textures get the 16-step palette.
- **Tools**: `ih-tools models` (describe or scan all), `textures` (decode, export colour
  and alpha), `render` (one model, optional animation frame), `sheet` (a grid of models).

### Results

| Data | Model files | Meshes | HLODs | Animations | Failed objects | Textures decoded |
|---|---|---|---|---|---|---|
| Stock Zero Hour | 8,897 | 70,585 | 6,505 | 4,090 | 10 | 6,774 of 6,774 |
| Contra X Beta 2 | 12,312 | 110,633 | 9,733 | 5,312 | 10 | 10,188 of 10,188 |

Of 70,136 HLOD sub-object references in stock data, 45 name objects no file contains
(helper boxes and the like the original also skips).

### Findings to carry forward

- **Targa row order.** WWLib's `Targa::Load` stores every image bottom row first and
  uploads it as is, so for Targas v = 0 is the image's bottom row. The DDS files were
  converted to match. Decoding Targas top-down puts colours on the wrong parts.
- The five `uisabotr_*.w3d` UI animations are malformed: a channel's chunk size is 3
  bytes short. The original reads garbage from that point on.
- `GENERALSKEL`, the hierarchy for six General portrait animations, is not in the data.
- Only time-coded compressed animations occur (21 stock, 72 Contra). The adaptive-delta
  decoder is checked by a unit test only.

### Not yet done (later milestones)

- Which sub-objects show depends on model condition state (muzzle flashes, afterburners,
  upgrade parts, damage states). That comes with drawables in M4; `--hide` stands in.
- Bump-environment mapping (Contra only, 1,885 materials) draws with the original's
  no-bump-support fallback. Sine, step and zig-zag offset, screen, random and edge
  mappers are not used by stock or Contra data and draw with plain UVs.
- LOD switching by screen size (the highest-detail LOD is always used), camera-aligned
  meshes (none in stock data), specular (none in stock data), shadows, fog.
- Lighting matches GameData's afternoon object lights; per-map lighting comes with M3.

To compare against the game, see [docs/reference-models.md](docs/reference-models.md).

## M3. Map viewer (next)

Map loading, terrain with blend tiles and cliffs, roads, bridges, water, trees and props,
placed objects, map lighting and a tactical camera, in a window.
