# Porting progress

Milestones are defined in [the project plan](docs/plan.md), recovered from Claude's
`a-long-time-ago-vast-abelson.md` plan. Numbers below come from `ih-tools`
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
- The model viewer uses GameData's afternoon object lights; M3's map viewer uses
  per-map lighting.

To compare against the game, see [docs/reference-models.md](docs/reference-models.md).

## M3. Map viewer (implemented; visual review pending)

Map loading, terrain with blend tiles and cliffs, roads, bridges, water, trees and props,
placed objects, map lighting and a tactical camera, in a window.

### Implemented

- Map DataChunk reader, RefPack decompression, height and blend tiles, cliff UVs,
  objects, polygon triggers, world properties and time-of-day lighting.
- Terrain atlas and base, blend and extra-blend layers, static vertex lighting,
  cloud and macro textures. Placed objects use their idle draw condition, including
  night and snow variants; standing water uses the height map for shore opacity.
- **Roads** (`crates/ih-render/src/roads.rs` and `roads/topology.rs`), continued from the unfinished
  working-tree port of `W3DRoadBuffer.cpp`:
  - adjacent map road-point pairs, reverse-endpoint duplicate suppression and the
    original 0.25-unit nudge for zero-length segments;
  - original `RoadWidth` / `RoadWidthInTexture` coordinates, terrain-cell maximum
    heights, collapsed columns and the original column-removal interpolation;
  - the shared terrain static lighting, texture alpha, cloud and macro modulation,
    depth testing without depth writes;
  - road definitions keep the reverse-creation linked-list order of
    `TerrainRoadCollection`, with `DefaultRoad` inheritance and ordered field layers.
  - tee, Y, slanted and four-way junction patches, normal/tight curved corners,
    angled mitres, cross-type alpha joins and texture stacking. Endpoint groups
    replace the original's mutable linked runs; the source's geometric calculations
    and patch UVs are retained.
- **Bridges** (`bridges.rs`): posed LEFT/SPAN/RIGHT meshes, nearest-span repetition,
  fixed bridges, horizontal fitting between map points, `BridgeScale` on width and
  height, terrain lighting, texture alpha testing, depth and projected clouds.
  Endpoints use the original triangle height interpolation plus 0.25 world units.
- **Rivers** (`water.rs`, `water.wgsl`): opposite-bank strips using `RiverStart`,
  per-point heights, original texture repeats and wobble, flowing texture coordinates,
  alpha-edge texture and sparkle/noise modulation. Standing water retains soft shores.
- **Windowed viewer** (`ih-game`): winit window and wgpu surface, resizing and surface
  recovery, terrain-following tactical camera, keyboard and edge pan, drag pan/orbit,
  wheel zoom, pitch/rotation, reset and overview. Background rendering is throttled;
  minimised windows wait. `--smoke` verifies setup and three resized/animated frames
  with a hidden window.
- Map-local `map.ini` definitions are loaded after startup definitions for rendering.
- `ih-tools maps` inspects/scans maps; `map-render` renders offscreen without opening
  a window. `--at X,Y --distance N --yaw N --pitch N` selects a closer view;
  `--time-ms N` fixes animation time. `map-check` validates road/bridge/water geometry
  and referenced textures across all mounted maps without a GPU.

### Verification (14 November 2026)

- `cargo test --workspace`: 62 tests passed, covering road geometry and topology,
  definition order, bridge span layout, river banks, terrain interpolation and
  camera controls. `cargo fmt --all --check` and workspace Clippy with
  `-D warnings` passed.
- All 150 stock maps parsed with no failures or warnings (281,480 map objects).
- All 333 Contra X maps parsed with no failures or warnings (531,339 map objects).
- CPU geometry verification: 150 stock maps, zero failures/asset warnings,
  206,360 road triangles, 508,370 water triangles, 199 bridges, 121 river areas.
  Contra: 333 maps, zero failures/asset warnings, 424,690 road triangles,
  1,258,140 water triangles, 248 bridges, 174 river areas.
- Headless GPU renders inspected: stock Tournament Desert (580 placed objects),
  stock Alpine Assault (664), and Contra X Tournament Desert (756). Road textures
  resolved without warnings on all three. These are visual smoke checks, not
  comparisons against retail screenshots.
- Further renders inspected: Twilight Flame bridge close-up, Winding River with
  bridges and flowing river strips, Dark Night. Dark Night contains an undefined
  `Scorch` object template, which is reported and skipped.
- Two Winding River renders at the same fixed 5,000 ms animation time produced
  identical PNG hashes on the local GPU; changing time from 0 to 5,000 ms changed
  the image. Stock and Contra INI coverage was regenerated and remains 100% with
  zero invalid values/unknown fields and the existing 69/207 dangling references.
- Hidden-window surface/configuration and resize/render checks passed on the local
  NVIDIA RTX 3070 Laptop GPU for stock Tournament Desert, Winding River and Contra
  Tournament Desert. Visible-window presentation and input feel need user review;
  Linux/macOS runtime behaviour has not been exercised locally.

### Milestone review

- Follow [the viewer review guide](docs/map-viewer.md): compare stock and Contra
  maps against the original and check visible presentation and input feel.
- This is a map viewer: object animations still use their initial idle pose; bridge
  damage updates, gameplay lights, shadows, shroud and fog remain in later work.
  Undefined road types are reported and skipped. Complex/degenerate junctions
  need the visual comparisons in addition to geometry validation.

## M4. Simulation core (movement, locomotion, traffic and initial AI slices implemented)

- Shared data definitions now live in `ih-ini`; `ih-client` adds render-specific
  interpretations. `ih-sim` has no dependency on the renderer, client or windowing.
- New `ih-math` f32/software-transcendental boundary and `ih-sim` logic RNG port
  (`RandomValue.cpp`), original frame/phase sleepy heap and wake behaviour
  (`GameLogic.cpp`, `UpdateModule.cpp`). Scheduler ties retain heap behaviour,
  rather than adding a new FIFO/object-ID ordering.
- Ordered objects and module slots, field layers/reskins, module-tag namespaces
  and Add/Remove/ReplaceModule operations. Unknown/unimplemented gameplay remains
  a data-bearing stub reported by class.
- Ground navigation and movement slice: 10-unit A*, source cost/turn constants,
  conservative footprints, cliffs/water/static obstacles, path shortening, parsed
  per-frame locomotor values, initial steering/integration and spatial buckets.
- Initial ground hierarchy uses the source's 10x10-cell blocks, local regions,
  global component equivalence and bridge-end connections. Clearance graphs are
  cached independently, and coarse corridors narrow fine search. Disconnected
  destinations skip fine search; transient obstacles may trigger an unrestricted
  fallback. Derived caches are excluded from state hashes and explicitly
  invalidated for static edits. `nav-check --from X,Y --to X,Y` reports search work.
- Pristine bridge navigation layers from posed W3D mesh widths and map endpoints,
  connected only at end cells. Bank/deck footprint transitions, rail boundaries,
  plane heights, ground underpass clearance and bridge picking are implemented.
  `nav-check` inspects CPU-side entry clearance and bank-to-bank routes.
- Initial live traffic: geometry bounding circles, source ungrouped priority
  rules, blocked-frame tracking and delayed repathing around transient objects.
  Swept disc clipping guards the corridor integrator against tunnelling/overlap;
  headless scenarios assert no overlaps every tick. Partition relocation commits
  during each physics update. Waiting units use idle art, and scenario cameras
  initially frame all spawned units.
- Timestamped spawn/move/stop/destroy commands, a 30 Hz integer clock and versioned
  canonical v4 state hash including bridge/layer setup, RNG, objects, module state,
  heap order, queued commands, traffic, AI actions and disabled status. `ih-tools sim-run` replays twice
  and compares every tick. Prior-version hash logs must be regenerated.
- Initial AI movement states use the source IDs for idle, move-to, follow-path,
  face-position and face-object. Success/failure returns to idle; replacement and
  Stop interrupt old actions. Failed actions remain observable in order counters,
  including a global count that survives object removal.
- All thirteen source disabled reasons have independent replacement deadlines,
  explicit clearing and end-of-frame expiry. Module eligibility uses the source's
  any-intersection mask (AI allows HELD, physics allows all reasons). The current
  ground integrator suspends drive while disabled and retains the AI action.
- Scenario checkpoints assert snapshot positions, stationary state, flags, AI
  state/outcome, facing and completed orders. They run headlessly, in offscreen
  scenario renders and in hidden smoke checks, while interactive play remains free.
- `ih-game --scenario ...` displays interpolated units with idle/MOVING art and
  animations, selection rings, terrain picking and move/stop/pause controls.
  `map-render --scenario ... --sim-frame N` produces reproducible movement stills.
- See [the movement guide](docs/m4-movement.md) and `scenarios/m4-*.toml`.

### Ground locomotion, physics and collisions

The initial ground integrator is replaced by ports of the original movement code
(`crates/ih-sim/src/locomotion.rs`):

- `Locomotor.cpp`: `locoUpdate_moveTowardsPosition` with the `TREADS`,
  `FOUR_WHEELS`/`MOTORCYCLE` (minimum turn speed, turning only while moving,
  reversing and three-point turns, the impassable-terrain look-ahead, the "donut"
  timer), `TWO_LEGS` (wander offset) and `OTHER` models; `rotateObjAroundLocoPivot`
  with `TurnPivotOffset`; braking factors and the exact braking slide onto the goal;
  `locoUpdate_maintainCurrentPosition`; the three logic random draws of
  `Locomotor::Locomotor` at object creation.
- `PhysicsUpdate.cpp`: motive forces (outside forces keep only their lateral part
  while driven), forward/lateral friction with `Extra2DFriction`, gravity
  (`Gravity = -64`), Euler integration, no x/y integration while braking, the
  ground clamp and `StickToGround`, `getForwardSpeed2D`'s per-axis magnitude,
  `scrubVelocity2D`. `PhysicsBehavior` mass and friction come from INI.
- Path following uses `Path::computePointOnPath`: closest segment, the next node
  or next segment's middle as goal when the line is passable, and the remaining
  distance along the path. Object paths now begin with the current segment's
  start node.
- `Locomotor` INI parsing keeps the original's unconverted `BIGNUM` defaults for
  unset `Braking` and `MinTurnSpeed` (they were previously divided like set values).

Collisions and traffic (state hash v7), from `PhysicsUpdate.cpp`, `AIUpdate.cpp`,
`AIStates.cpp`, `AIPathfind.cpp`, `Object.cpp`, `SquishCollide.cpp` and `Locomotor.cpp`:

- An end-of-frame collision pass runs each touching ground pair both ways, in
  object-ID order: `SquishCollide` first, then `PhysicsBehavior::onCollide`.
  Insubstantial objects (no physics, not `IMMOBILE`) are ignored; immobile objects
  bounce with `StructureStiffness`; non-AI bodies are pushed apart by their
  overlap (at most 5).
- AI units go through `AIUpdateInterface::processCollision` and `blockedBy`
  (goal proximity, crushable targets, reversing, crossing infantry, collision and
  facing angles, same-cell and head-on path priority), `calculateMaxBlockedSpeed`,
  `needToRotate` and the collision-ignore timer.
- `doLocomotor`: blocked frames, the blocked speed cap and the decaying/recovering
  bump speed limit feed the locomotor's original `*blocked` branch.
- `AIInternalMoveToState` recomputes the path when blocked and stuck, or blocked
  for more than two seconds (ironhour also spaces recomputations by one second).
- Moving out of the way (`aiMoveAwayFromUnit`, `Pathfinder::getMoveAwayFromPath`,
  `AIMoveOutOfTheWayState`): infantry blocking a vehicle, and the lower-priority
  unit of a head-on deadlock, take the cheapest reachable cell whose box (grown by
  both units' cell radii) clears the other unit's path and the previous one's,
  follow it as a temporary state for up to ten seconds, then resume their order.
- Crushing: `Object::canCrushOrSquish` (`CrusherLevel` above `CrushableLevel`, or
  any crusher against `SquishCollide`; never allies), `checkForOverlapCollision`
  (a moving crusher overlaps what it can crush, the crushee stays passive) and
  `SquishCollide` (an enemy crusher reaching the infantry's centre while moving
  toward it). Crushers are never blocked by what they can crush.
- `Locomotor::fixInvalidPosition` pushes ground bodies off invalid cells; there is
  no terrain gate on ground steps (deck edges still stop a step).

Deviations until the missing systems exist, each marked in the code:

- Bodies that cannot crush each other may not overlap: swept clipping stops a step
  at contact and the rest slides along it. The original lets bodies overlap, so
  collisions are detected within `CONTACT_RANGE` (1 unit) of touching.
- Squished objects are removed outright and logged in `Simulation::crushed`;
  the original deals `HUGE_DAMAGE_AMOUNT` of crush damage, and death modules,
  vehicle crush damage (front/back crush points) and `CrushDie` arrive with
  bodies and damage in M5.
- Allies are objects with the same owner until players and teams exist.
- Not ported: `moveAllies` (allies on a move-aside path moving too), ability/busy
  exemptions, dead and panic states, `isWaitingForPath`, and the always-checked
  look-ahead to the next path segment (the original skips it when very close).
- Off-centre pivot turning goes through the same clipping as the rest of the step.
- Not ported yet: damage-state speed/turn penalties, bouncing, pitch/roll/yaw,
  falling damage, air/hover/wings/thrust/climber locomotors and `ZAxisBehavior`
  other than resting on the ground. `HOVER`, `THRUST`, `WINGS` and `CLIMBER`
  ground movement uses the `OTHER` model.
- `ih-tools sim-run --watch ID [--watch-every N]` prints one object's movement
  state every N frames (default 15); `sim-run` also reports crushes and move-asides.

All fourteen scenarios pass with v7 hashes (every-tick replay equality, no
overlaps or rejected commands, all orders finished, required bridge traversals):

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `8890b2e94036cc18` | `dad945a5f61bde9f` |
| AI/status orders | `346ef704b8d1e187` | `0a7df79f6788bfea` |
| Bridges | `08391e0ac9bc1e81` | `c7676cae944eed22` |
| Traffic, Tournament Desert (one move-aside each) | `faecd6efbb0a34dd` | `c60b7d5e669a59b7` |
| Traffic, Winding River | `e42e0da698af6360` | `d0c02bc7475ead52` |
| Long map travel | `2fce2fab1ba1961f` | `842c52c64e964722` |
| Crusader through an enemy Ranger (crushed at frame 131) | `eaf7c20f02e28bfb` | |
| Crusader through an allied Ranger (steps aside) | `9c5f8ac52f1b512f` | |

Asset-free fixtures: `5f5f68319f62cf7f` (ground), `5f866c9f7c83c1b5`
(bridge/deck stop), `6a39c8d0cf2cd386` (head-on traffic, resolved by a
move-aside), `19c96d8f45ef7ec6` (AI/status orders). Unit tests also cover a tank
squishing enemy infantry and allied infantry stepping aside. The v4 hashes below
are superseded and kept as history.

### Verification (v4, before the locomotion port)

- 110 workspace tests passed; formatting and workspace Clippy pass.
- Asset-free synthetic fixtures hash to `f3653e098b76c23b` (ground),
  `eb2e076f238f6b44` (bridge/deck stop), `66512131ec5b2c98` (head-on traffic),
  and `71e29433699be8fd` (AI/status orders).
  They pass in debug/release locally and run in
  the existing three-OS CI test matrix. Only local Windows execution was observed.
- Stock and Contra scenarios each replay 1,200 ticks twice with matching hashes
  at every frame and no rejected/unfinished orders. Stock uses Crusader/Ranger;
  Contra uses Humvee1/Ranger because Contra disables the stock Crusader's movement.
- Before/after offscreen unit renders inspected; hidden-window scenario smoke
  checks passed for stock and Contra.
- Stock and Contra bridge scenarios each complete two opposite-end traversals
  over 1,200 ticks with matching hashes at every tick, no rejected commands and
  all orders finished. Final v4 hashes: stock `23669f0448a21c71`, Contra
  `4e58507f8039d36b`. Desert v4 hashes: stock `5906f49c0df99820`, Contra
  `b29baf2aa2c7ae30`.
- Bank-to-bank navigation succeeds on all four stock Winding River crossings and
  all three retained by Contra for a 15-unit footprint radius. Bridge crossing
  still inspected offscreen; hidden-window surface/resize checks passed for both
  bridge scenarios. Contra reports a missing placed building model `CBOldMslm_B`;
  movement is unaffected and the warning remains visible.
- Four 1,800-tick traffic replays (stock/Contra, desert/bridge) finish with
  matching hashes every tick, no overlaps/rejected orders and all orders finished.
  Each exchanges two vehicles past a parked third vehicle; both bridge runs
  complete two traversals. Final v4 hashes:

  | Traffic scenario | Stock | Contra |
  |---|---|---|
  | Tournament Desert | `25f624d03c84e581` | `2c55d8aedb1921e3` |
  | Winding River | `ee5bb98597773bd3` | `b3392d3351dd6688` |

- Traffic tests cover off-centre contact escape with stock-sized boxes, parked
  obstacles, opposing units, followers, blocker removal, occupied-deck underpasses
  and long-step tunnelling. Open-ground and bridge traffic stills were inspected.
- Hierarchy tests verify reachability against unrestricted search on seeded grids,
  reduced fine-cell work on a large detour, disconnected-goal rejection before
  fine search, alternate-route fallback for transient traffic, separate clearance
  classes, cache invalidation/clone isolation and unchanged hashes after cache warmup.
- Both 6,000-tick long-map scenarios finish with every-tick replay equality, no
  overlaps/rejected orders and completed orders. Stock traverses two bridges;
  Contra traverses one. Final v4 hashes are `ed48eb0a2350f00d` and
  `a4e5f69307b55ccb`, respectively. Corner regressions cover reaching intermediate
  waypoints before turning and retrying terrain-blocked movement.
- At radius 15, Winding River has 914 zones/17 components in stock and 915/13 in
  Contra. The cross-map query expands 280 coarse/3,228 fine nodes in stock and
  304 coarse/3,404 fine nodes in Contra; these counts exclude initial zone building.
- Two AI/status logs run 1,200 ticks each with matching every-tick hashes, all
  twenty-four checkpoints passing, no overlaps/rejections/failed actions and all
  actions finished. They exercise overlapping EMP/HELD expiry, explicit script
  re-enable, follow-path and both facing states. Final v4 hashes: stock
  `1d222a39729ab9d7`, Contra `d0970703c7536062`.
- Disabled and final-facing scenario stills were inspected; AI/status hidden-window
  surface/resize/render/checkpoint smoke checks pass for both stock and Contra.
- AI/status regressions cover module exception masks, independent/replaced timers,
  stop during disable, new-command rejection, moving/destroyed facing targets,
  later-leg failure, interruption, path visit order and per-tick replay equality.

### Still required for M4

Multi-surface/crusher zone tables and incremental zone updates, bridge damage/destruction/repair,
full traffic/formation and collision handling,
air/hover/thrust locomotors, the physics and traffic pieces listed above (vehicle crush damage, moveAllies, overlap handling), combat/guard/transport/mood/temporary AI states,
disabled gameplay side effects,
map-wide object lifecycle and complete condition-state drawables. Traffic uses the ported blocked/stuck, move-aside and crush logic on bounding circles;
shape narrow-phase and crowded bottleneck recovery remain pending. Occupied goals/routes
without passing room wait until space is available. M3's visual review remains pending.
Disabled movement currently stops horizontal drive immediately; original inertia,
gravity/falling, held transports, tint/effects, special-power timers, rider
propagation and power-system effects need their remaining runtime systems.
