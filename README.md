# ironhour

A new engine, written in Rust, that plays Command & Conquer Generals Zero Hour from your own copy of the game's data, runs existing mods, and extends what mods can do.

ironhour is an unofficial project. It is not affiliated with or endorsed by Electronic Arts. "Command & Conquer" and related names are trademarks of Electronic Arts Inc.; they are used here only to say which game's data this engine reads.

## Status

Milestone M0 (foundations): the virtual file system reads `.big` archives with the original game's priority rules. See [PORTING.md](PORTING.md) for progress.

## You need the game

No game assets are included or ever will be. ironhour reads an installed copy of Zero Hour (and the base Generals install it depends on) in place, read-only.

Install folders are found from the registry on Windows. Anywhere else, or to override, set:

- `IH_ZH_INSTALL`: the Zero Hour folder
- `IH_GENERALS_INSTALL`: the base Generals folder

## Tools

```
cargo run -p ih-tools -- installs               # show detected installs
cargo run -p ih-tools -- archives               # archives in mount order
cargo run -p ih-tools -- ls "data\ini\*.ini"    # list resolved files
cargo run -p ih-tools -- which data\ini\weapon.ini
cargo run -p ih-tools -- extract data\ini\weapon.ini out/
cargo run -p ih-tools -- --profile profiles/contra-x-beta2.toml archives
```

## Licence

GPL v3 with Electronic Arts' additional terms (see [LICENSE.md](LICENSE.md)). ironhour translates the behaviour of the source code EA released at https://github.com/electronicarts/CnC_Generals_Zero_Hour, so the same terms apply. Every ported file names the C++ file it was translated from.
