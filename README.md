# ironhour

A new engine, written in Rust, that plays Command & Conquer Generals Zero Hour from your own copy of the game's data, runs existing mods, and extends what mods can do.

ironhour is an unofficial project. It is not affiliated with or endorsed by Electronic Arts. "Command & Conquer" and related names are trademarks of Electronic Arts Inc.; they are used here only to say which game's data this engine reads.

## Status

Milestones M0 to M2 are done. M3's map viewer is implemented and ready for visual review: terrain, road junctions and curves, bridges, flowing rivers, standing water, placed objects and map lighting render offscreen or in a native window with a tactical camera. Stock Zero Hour and Contra X are checked against local game data. See [PORTING.md](PORTING.md) for progress, [the viewer guide](docs/map-viewer.md) to try it, and [the project plan](docs/plan.md) for the remaining milestones.

M4 is well under way: a renderer-independent 30 Hz simulation with the original
logic RNG and sleepy scheduler, hierarchical ground and bridge paths, the original
locomotors and physics (ground vehicles, infantry and helicopters), collisions,
blocked traffic, moving out of the way and crushing, movement AI orders and timed
disabled status, the map's own objects with obstacle cells under its structures,
and drawables that follow the original model condition states.
Command logs replay with identical per-tick state hashes. The
[movement demo](docs/m4-movement.md) supports selection and move orders and has a
review checklist.

M5 now has deterministic combat and economy logic slices (with gameplay and
visual gaps recorded in PORTING.md). M6's interface/audio slice is implemented:
original WND menus start a sandbox skirmish with loading and score screens,
faction control bars, command buttons, hotkeys, cursors and saved options.
UI clicks, positional sounds, unit responses, EVA, ambient/movement loops,
streaming dialogue and shell/faction music use mounted game assets.
Stock and Contra menu-to-game smoke checks pass. Remaining gameplay dependencies
and human review items are recorded in PORTING.md. Script execution and skirmish AI are M7.

## You need the game

No game assets are included or ever will be. ironhour reads an installed copy of Zero Hour (and the base Generals install it depends on) in place, read-only.

Install folders are found from the registry on Windows. Anywhere else, or to override, set:

- `IH_ZH_INSTALL`: the Zero Hour folder
- `IH_GENERALS_INSTALL`: the base Generals folder

With no `--profile`, ironhour mounts what the original game would load from the
install as it is. A mod launcher that renames its archives to `.big` (Contra's
does) therefore switches the mod on here too; `--profile profiles/stock.toml`
leaves Contra out.

## Tools

Linux builds need the ALSA development headers and pkg-config (on Ubuntu:
`sudo apt-get install libasound2-dev pkg-config`). Audio tests use a mock
backend and do not need a speaker or output device.

```
cargo run -p ih-tools -- audio-check GUIClick GUIComboBoxClick GUIClickDisabled
cargo run -p ih-tools -- audio-check --fx WeaponFX_GenericTankShellDetonation
cargo run -p ih-tools -- audio-check --eva-side America LowPower UnitLost
cargo run -p ih-tools -- music-check Shell Game_USA_01
cargo run -p ih-game -- --shell             # menu clicks use the installed sounds
cargo run -p ih-game -- --shell --no-audio  # disable output
cargo run -p ih-game -- --shell --no-music  # sound effects and speech only
cargo run -p ih-tools -- installs               # show detected installs
cargo run -p ih-tools -- archives               # archives in mount order
cargo run -p ih-tools -- ls "data\ini\*.ini"    # list resolved files
cargo run -p ih-tools -- which data\ini\weapon.ini
cargo run -p ih-tools -- extract data\ini\weapon.ini out/
cargo run -p ih-tools -- load-order             # INI files in the order the game loads them
cargo run -p ih-tools -- scan                   # parse and check every loaded INI file
cargo run -p ih-tools -- strings OBJECT:Crusader
cargo run -p ih-tools -- coverage               # what ironhour understands, as Markdown
cargo run -p ih-tools -- models AVCrusader       # describe a W3D model
cargo run -p ih-tools -- textures avcrusader.tga --out out/t.png
cargo run --release -p ih-tools -- render AVCrusader --hide TURRETFX --out out/crusader.png
cargo run -p ih-tools -- maps                    # inspect every mounted map
cargo run -p ih-tools -- maps "Tournament Desert"
cargo run --release -p ih-tools -- map-check     # validate road/bridge/water geometry on every map
cargo run --release -p ih-tools -- map-render "Tournament Desert" --out out/desert.png
cargo run --release -p ih-tools -- map-render "Tournament Desert" --at 1500,1500 --distance 1800 --out out/desert-close.png
cargo run -p ih-tools -- --profile profiles/contra-x-beta2.toml coverage
cargo run --release -p ih-game -- "Tournament Desert"
cargo run --release -p ih-game -- "Winding River" --overview
cargo run --release -p ih-game -- "Tournament Desert" --profile profiles/contra-x-beta2.toml
cargo run -p ih-game -- "Winding River" --smoke  # hidden-window verification
cargo run --release -p ih-tools -- --profile profiles/stock.toml sim-run scenarios/m4-desert.toml --trace out/desert-hashes.txt
cargo run --release -p ih-game -- --profile profiles/stock.toml --scenario scenarios/m4-desert.toml
cargo run --release -p ih-game -- --profile profiles/contra-x-beta2.toml --scenario scenarios/m4-contra.toml
```

## Licence

GPL v3 with Electronic Arts' additional terms (see [LICENSE.md](LICENSE.md)). ironhour translates the behaviour of the source code EA released at https://github.com/electronicarts/CnC_Generals_Zero_Hour, so the same terms apply. Every ported file names the C++ file it was translated from.
