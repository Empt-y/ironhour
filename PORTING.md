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
  keep the error or downgrade it to a warning.

## M1. Data layer (in progress)

Done:
- `ih-schemagen` extracts the schema from the C++ source into
  `crates/ih-ini/src/schema/generated.rs`: 285 field tables, 3,093 field definitions,
  223 module names resolved through class inheritance, 104 name lists, 12 lookup lists.
  Regenerate with `cargo run -p ih-schemagen -- reference/ea-zh/GeneralsMD/Code crates/ih-ini/src/schema/generated.rs`.
- Schema validation in `ih-tools scan`: every field in stock (387,233) and Contra X
  (848,055) data resolves to its exact C++ definition, with no unknown fields or modules.
- `ih-tools scan --parsers` lists the value parsers the data uses (about 190).

Remaining:

- Load-order manifest: exactly which files `GameEngine::init` and each subsystem load,
  in order (Default, then main file, then folder), plus `map.ini` overrides.
- Typed field tables for all 62 block types and the field parsers with their unit
  conversions.
- CSF/STR string tables, mapped images.
- `ih-tools coverage`: blocks, fields, modules and script actions used versus implemented.
