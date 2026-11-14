# Map viewer review

M3 is ready for the end-of-milestone review in the project plan. The engine is
still a viewer by default. The separate [M4 movement scenario](m4-movement.md)
enables unit selection and move orders.

From the repository folder:

```powershell
cargo run --release -p ih-game -- "Tournament Desert"
cargo run --release -p ih-game -- "Winding River" --overview
cargo run --release -p ih-game -- "Twilight Flame" --overview
cargo run --release -p ih-game -- "Dark Night" --overview
cargo run --release -p ih-game -- "Tournament Desert" --profile profiles/contra-x-beta2.toml
```

The game installations are read-only. Existing mod profiles select archives
without renaming or modifying anything in the installs.

| Control | Action |
|---|---|
| WASD / arrows | Pan relative to the view |
| Pointer near a focused window's edge | Edge pan |
| Shift | Faster keyboard/edge pan |
| Mouse wheel | Zoom |
| Right mouse drag | Pan |
| Middle mouse drag | Orbit and pitch |
| Q / E | Rotate |
| R / F | Pitch |
| Home | Reset the initial camera |
| O | Centre and show an overview |
| Escape | Close |

Please check these before moving on to M4:

1. Resize, minimise and restore the window. Pan, zoom, rotate and pitch; check for
   unexpected jumps, clipping, colour differences or input that feels wrong.
2. Compare a few views against Zero Hour. Tournament Desert exercises tight
   corners and road junctions; Twilight Flame exercises bridge ends and repeated
   spans; Winding River exercises banks, bridge heights and flowing water; Dark
   Night exercises map lighting with clouds disabled.
3. Try Contra's Tournament Desert using the profile command above. Check trees,
   props, roads and lighting against Contra.

The plan prioritises the original look. In particular, report terrain seams,
wrong texture orientation, missing props, mismatched road joins or bridges that
float above or sink into their endpoints. Shadows, fog/shroud and full object
condition/animation updates are later work.

For a reproducible still:

```powershell
cargo run --release -p ih-tools -- map-render "Twilight Flame" --at 2440,2400 --distance 1600 --out out/bridges.png
cargo run --release -p ih-tools -- map-render "Winding River" --time-ms 5000 --out out/river.png
```

Automated checks (all stay offscreen or use a hidden window):

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo run --release -p ih-tools -- map-check
cargo run --release -p ih-tools -- --profile profiles/contra-x-beta2.toml map-check
cargo run --release -p ih-game -- "Winding River" --smoke
```

The geometry checks cover all 150 stock maps and all 333 mounted Contra maps.
The hidden smoke check creates/configures a native surface, then verifies three
offscreen frames at different sizes/camera positions/times. It deliberately does
not open a visible window or claim to verify human input or visible presentation.
