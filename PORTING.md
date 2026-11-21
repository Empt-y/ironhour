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
  falling damage, and the thrust and climber locomotors (see air locomotion below).
- `ih-tools sim-run --watch ID [--watch-every N]` prints one object's movement
  state every N frames (default 15); `sim-run` also reports crushes and move-asides.

Air locomotion (state hash v8), from `Locomotor.cpp`, `PhysicsUpdate.cpp`,
`AIUpdate.cpp` and `Thing.cpp`:

- `HOVER` (helicopters) and `WINGS` (fixed-wing flight) steer in 2D as `OTHER`
  does; `AllowAirborneMotiveForce` gates motive force once airborne (higher than
  three frames of falling, `-9 * Gravity`); airborne flyers are never blocked.
- `handleBehaviorZ` for every `ZAxisBehavior` and `calcLiftToUseAtPt`: lift toward
  `PreferredHeight` over the surface (water where the point is in a water area),
  with `PreferredHeightDamping`, `SpeedLimitZ` and `Lift`.
- `locoUpdate_maintainCurrentPosition` for hover (brake to a stop) and wings
  (circle the held position at `CirclingRadius` or the minimum turn radius);
  flyers keep calling it while idle, so they stay up (`doLocomotor`).
- Aerodynamic friction (`AerodynamicFriction`) instead of ground friction when
  significantly above the terrain, unless `Apply2DFrictionWhenAirborne`.
- Flyers take straight paths (`computeQuickPath`) and are not collided with ground
  units. `map-render --at X,Y,Z` aims the camera at a height.
- Not yet: `THRUST` (needs pitch/roll orientation), airfield take-off and landing
  (`JetAIUpdate`), the `CIRCLE_FOR_LANDING` code path, `ULTRA_ACCURATE`, damage-state
  lift, and the over-water model condition. `THRUST` and `CLIMBER` use `OTHER`.

### The map's own objects (state hash v9)

From `GameLogic::startNewGame` (GameLogic.cpp), `SidesList::ParseSidesDataChunk`
(SidesList.cpp), `PlayerList::newGame`/`validateTeam` (PlayerList.cpp) and
`Pathfinder::classifyObjectFootprint` (AIPathfind.cpp):

- Maps' `SidesList` chunks are read: side dictionaries, build lists (with the
  version 3 fields) and teams. Player scripts in it are left for M7. All 150
  stock and 333 Contra maps parse.
- `Simulation::load_map_objects` creates the map's logic objects in map order,
  at the stored height above the ground, with their facing (`normalizeAngle`),
  on the layer nearest that height. Road/bridge points, objects without
  templates, `OPTIMIZED_TREE`s, `PROP`s and `CLEARED_BY_BUILD` fluff without a
  fence are skipped. ironhour always takes the multiplayer setting (`useTrees` on,
  `forceFluffToProp` on), so the result does not depend on detail settings.
  `handleNameChange`'s two renames are applied.
- Owners: player 0 is neutral, then each side with a player name. An object's
  `originalOwner` team gives its `teamOwner`; a player's default team is
  `team<playerName>`; unknown owners are neutral.
- Obstacle cells: each immobile, not-small `STRUCTURE` on the ground marks its
  `BOX` (rotated, sampled every half cell) or circle footprint as obstacle cells,
  then closes off pinched clear cells nearby (`CELL_IMPASSABLE`) and marks
  the border as pinched (`MARK_BORDER_PINCHED`); `FenceWidth` objects mark a fence line
  without pinching. Mines, projectiles and bridge towers are skipped. Obstacles
  are added after the terrain and bridges, one object at a time, as the original
  does. This replaces the earlier stand-in, which blocked a disc for every
  structure or immobile object.
- Units are kept out of structures by those cells, as in the original, where AI
  units get no collision force from them: the no-overlap clipping and the overlap
  check apply to mobile bodies only. Pairs of immobile objects are left out of
  the collision pass.
- The partition registers immobile objects in every bucket their bounding circle
  covers, clipped to the map, so large ones no longer widen every query. Contra's
  water planes (`WTR_800C`, radius 5657) had made each query scan the whole map.
  Collision shapes are worked out once per object.
- Scenario spawns keep IDs 1..n and the map's objects follow. The window still
  draws the map's objects as static scenery, and only scenario objects can be
  selected, until objects can change (M5).
- `sim-run` reports how the map's objects were sorted and the obstacle-cell counts.
- Not yet: landmark bridges and wall pieces (`IsBridge`, `WALK_ON_TOP_OF_WALL`),
  object properties from the map (`objectName`, initial health and the rest of
  `updateObjValuesFromMapProperties`), `onBuildComplete`, team activation, build
  lists turned into players' bases, removing obstacles when objects die,
  and rubble cells.

### Sleepy physics (state hash v10)

`SLEEPY_PHYSICS` from `PhysicsUpdate.cpp`: after each update a `PhysicsBehavior`
sleeps (`calcSleepTime`) once it has run at least once (`UPDATE_EVER_RUN`, now
hashed) and has no velocity, acceleration or live motive force, on the ground
layer and not above the terrain. A force applied from outside its update, even a
zero one, wakes it with `setWakeFrame(UPDATE_SLEEP_NONE)`: next frame, or this
frame if it is already due. So a resting unit given an order starts moving one
frame after its AI's first motive force, and an EMP'd unit's body sleeps once its
last motive force expires. Forces from AI updates wake their own body at once;
forces from commands and collisions are checked after those passes. Overlap
tracking (`getCurrentOverlap`) is not kept yet and counts as none. The braking
slide, applied with the physics step here, also wakes the body.

Stock scenarios are run with `profiles/stock.toml`, which leaves out Contra's
archives: Contra's launcher renames the packs it enables to `.big`, so with no
profile ironhour mounts them, as the original game would. The Contra profile's
excludes now match any extension, so its result does not depend on how the
launcher last left the install.

### Terrain cells and locomotor surfaces (state hash v11)

From `AIPathfind.cpp` (`classifyMap`, `classifyMapCell`,
`validLocomotorSurfacesForCellType`, `validMovementTerrain`), `TerrainLogic.cpp`
(`getWaterHandle`, `isUnderwater`), `PolygonTrigger::pointInTrigger` and
`WorldHeightMap.cpp`/`BaseHeightMap.cpp` (cliff state):

- Cells have the original types and values (clear, water, cliff, rubble,
  obstacle, bridge-impassable, impassable). A cell is a cliff when the map's stored
  cliff state says so; only maps without it fall back to the 9.8 slope rule. The
  stand-in before this ORed the two. A cell is water when any corner is under
  the highest water area over it (integer `pointInTrigger`, water at z 0 or above).
  Cliffs then grow one cell into clear ground, and a pinched border is kept
  around them. The `type & CELL_CLIFF` test is kept as written.
- Each locomotor's `Surfaces` decide which cells it may use: the simulation keeps
  one terrain view per distinct surface mask (ground units use the base grid),
  for paths, steering, `fixInvalidPosition`, moving out of the way and repaths.
  Boats stay on water, cliff climbers may use cliffs, and ground units use
  neither. Collisions and blocking apply to any locomotor without `AIR`
  (`isDoingGroundMovement`), boats included.
- Hover lift over water uses the same water lookup. Grid water (`getWaterGridHeight`)
  is not supported. Cell types are part of the setup hash.
- Not yet: rubble from destroyed buildings, wall layers, crusher and per-surface
  zone tables (zones are still computed per view and radius).

### Chassis tilt and sway (render-only; state hash v12)

From `Drawable.cpp` (`applyPhysicsXform`, `calcPhysicsXform` and its `Treads`,
`Wheels`, `Motorcycle`, `HoverOrWings` and `Thrust` models) and
`BaseHeightMap.cpp` (the smoothed terrain normal from `getHeightMapHeight`):

- Vehicles settle onto the slope under them with spring/damper suspension,
  pitch and roll as they accelerate, brake and turn (`ForwardAccelerationPitchFactor`,
  `LateralAccelerationRollFactor` and the pitch limits), wheeled vehicles bounce
  on rough ground (`BounceAmount`) and rise when steeply pitched; helicopters and
  planes tilt with their speed and sway (`RudderCorrection*`, `ElevatorCorrection*`);
  thrust locomotors wobble and roll. Every locomotor field these read is parsed
  with the original's units and defaults.
- Kept as written: the acceleration roll is limited by the pitch limits;
  `deltaZ_X3` in the normal is `d6 - d0`; a motorcycle's roll test
  (`> 0.5 && < -0.5`) is never true, so motorcycles never roll.
- The original stepped the models once per drawn frame, which the retail game
  locked to the 30 Hz logic; ironhour steps them once per logic frame and
  interpolates, so they look the same at any frame rate. Bounces use the client
  random stream, never the logic one.
- The logic now keeps `m_prevAccel` (the forces other modules applied, which the
  models read); it is hashed, hence v12.
- Not yet: the tread bump over objects being crushed (needs `getCurrentOverlap`),
  per-wheel suspension and steering angles (truck draw modules), shock rates from
  explosions (M5), and pitch/roll in the logic for `THRUST` steering.

### Goal cells (state hash v13)

From `AIPathfind.cpp` (`updatePos`, `removePos`, `updateGoal`, `removeGoal`,
`adjustDestination`, `checkForAdjust`, `checkDestination`,
`snapClosestGoalPosition`, `getRadiusAndCenter`, `PathfindCell::setGoalUnit`
and `setPosUnit`) and `AIStates.cpp` (`AIInternalMoveToState::onEnter`, the idle
state):

- Every mobile AI unit marks the cells it stands on and the cells of its goal,
  with the original cell flags (goal, moving, fixed, goal-of-another-moving). Its
  footprint is one, two or three cells across (two-wide units sit on cell
  corners, odd widths on cell centres, as `getRadiusAndCenter` rounds).
- A move order adjusts its goal to the nearest cell, in the original 400-cell
  square spiral, that is not an obstacle, not a cliff, not an ally's goal, not
  occupied by a stationary unit it cannot crush, and reachable by zone; failing
  that it snaps to a nearby free cell. Units sent to one spot therefore spread
  out and stop apart instead of queueing for it. Goals sit on the grid, so a unit
  ends up to half a cell from the clicked point (with the original's 0.05-cell
  offset for even widths).
- Idle units hold a goal where they stand; destroyed units release their cells.
  Follow-path orders claim only the final point, unadjusted. Hover and wing
  aircraft keep their own aircraft goals.
- A path is routed to its goal on the layer it was chosen on. A unit standing
  exactly on a path node now passes it (the original aims past nodes with its
  look-ahead point); before, a unit that braked onto a deck end could stall.
- Not yet: group moves (`AIGroup`), the ignored obstacle, `tightenPath` and
  `checkPathCost` for group destinations, and bridge-end cells shared by both
  layers (`objectInteractsWithBridgeEnd`).

### Group moves, follow-path legs and box shapes (state hash v14)

From `AIGroup.cpp` (`groupMoveToPosition`, `friend_computeGroundPath`,
`friend_moveInfantryToPos`, `friend_moveVehicleToPos`, `computeIndividualDestination`,
`groupTightenToPosition`, `getMinMaxAndCenter`, `clampToMap`, `clampWaypointPosition`),
`AIStates.cpp` (`AIFollowPathState`), `AIPathfind.cpp` (`moveAllies`) and
`PartitionManager.cpp` (the 2D collide tests):

- A `group_move` command (and right-click with several units selected; drag a box
  to select) moves a selection as the original does. Far or large groups follow
  one shared ground path: infantry in three columns (five from 16), vehicles in
  two lanes (three from 5), each ending at its own adjusted spot. Short moves keep
  the group's shape (each unit keeps its offset, at most six bounding radii). A
  click inside the middle half of the group's box gathers it
  (`GroupMoveClickToGatherAreaFactor`). `AIData`'s `MinInfantryForGroup`,
  `MinVehiclesForGroup`, `MinDistanceForGroup` and `DistanceRequiresGroup` and the
  locomotors' `GroupMovementPriority` are read; Contra's own values apply.
- Kept as written: vehicles on a straight shared path move individually (the
  end node is the path's first, which the vehicle layout rejects); vehicles are
  balanced into lanes once per priority group; tightening measures both sides of
  the box with its width.
- Follow-path legs: intermediate legs keep their speed (`setPathExtraDistance`)
  and finish by the easy close-enough test (`CloseEnoughDist`, now parsed); points
  within a cell are skipped; a failed or unreachable leg is retried (10 retries per
  order) and then skipped; the final leg adjusts its goal.
- `moveAllies`: after a new path, idle allies standing on it are asked to move out
  of the way (infantry never ask infantry). When no route around the units exists
  the terrain route is taken and the allies on it asked to move, as the original
  plans through allies' cells.
- Collision shapes: contact, the no-overlap clipping and the overlap check use the
  original's 2D tests (a box corner inside the other box; circles as squares at
  their facing; close centres always touch) instead of bounding circles, so
  boxes park side by side as in the original. Steps involving a box are clipped by
  bisection and slide along the boxes' edges. Collision forces still use bounding
  circles, as in the original.
- Stand-ins: tightening goes through an adjusted move, not
  `AIMoveAndTightenState`'s approach path; infantry pass through infantry, and a
  stationary one does not block them; turning boxes may overlap at their corners,
  and the overlap check counts a centre inside another body. Formations and
  waypoint appends are not ported. (The shared path is now `findGroundPath`; see
  the pathfinder section.)

### Wall pieces

From `GameLogic::startNewGame` (the bridge pass) and `Pathfinder::newMap`:
`WALK_ON_TOP_OF_WALL` map objects (the fortress walls on Fallen Empire, Iron
Dragon and many Contra maps) are created first, by the neutral player, and their
footprints become obstacle cells like any structure's, arches included, as
`newMap` classifies every existing object. The wall-top layer
(`classifyWallCells`, `WallHeight`) has no connection to the ground in the
original; only units placed or garrisoned on walls reach it, so it comes with
containers in M5.

### Landmark bridges

From `TerrainLogic.cpp` (`addLandmarkBridgeToLogic`, `Bridge::Bridge(Object*)`):
`IsBridge` objects (Monument train bridge, Tsing Ma, European landmark bridge,
Asian flood bridge) become bridge layers: the object's box is the deck, flat at
the object's height, end to end along its facing and as wide as the box. They
load on every stock and Contra map that places them; where their ends meet the
ground (Tsing Ma, the European bridge on gc_airgeneral) units cross them. Towers,
damage and the bridge objects themselves come with M5. `ih-tools maps --uses
NAME,...` lists the maps that place given templates.

### Crushers and fences (state hash v15)

From `AIPathfind.cpp` (`validMovementPosition`, `m_crusherZones` and
`getEffectiveZone`'s crusher flag): units that can crush (`CrusherLevel` above
0) may enter fence obstacle cells, so they path straight through fence lines
that others must go round, with zones of their own. Each crusher gets a terrain
view with fence cells open (only on maps that have fences). Contra's long
Winding River run changes for that reason. A test drives a tank through a fence
that a jeep goes round.

### Overlaps and the tread bump (state hash v15)

From `PhysicsUpdate.cpp` (`addOverlap`, `checkForOverlapCollision`, the
overlap hand-over at the end of `update`, `calcSleepTime`) and `Drawable.cpp`
(`calcPhysicsXformTreads`):

- A crusher records what it is driving over (`m_currentOverlap`, the latest one
  each frame); each physics update moves it to `m_previousOverlap`. Both are
  hashed, and a body with either set does not sleep.
- Tracked vehicles rise over what they crush: the chassis tilts along the
  overlapped object with rough vibration at speed, sits on top of it once over
  it, lifts by half a unit over infantry and `LOW_OVERLAPPABLE` things, kicks
  its pitch when it leaves an overlap in the air, and falls back with the
  original's fake z. Shrubbery is smashed through without a bump.
- Not yet: the zero-damage crush hit (`DamageFX`), front and back crush states
  and the crush point logic, which need bodies (M5).

### Truck wheels and turning (state hash v16)

From `W3DTruckDraw.cpp` (`doDrawModule`, `updateBones`), the wheel part of
`W3DTankTruckDraw.cpp`, `Drawable::calcPhysicsXformWheels`' `DO_WHEELS` blocks,
`Locomotor::rotateObjAroundLocoPivot` and `HTreeClass::Control_Bone`:

- The logic keeps `getTurning` (turning at the limit this update, cleared at the
  start of each move), hashed, hence v16.
- Wheeled vehicles with `HasSuspension` keep per-wheel offsets and a steering
  angle (spring factor 0.9, dropping wheels damped, extension limit, smoothing
  by tenths, reversed when backing up), and their wheels extend when airborne.
- Truck draw modules spin their tires with speed (backwards when reversing, the
  rear faster while powersliding), steer and lift the front tires, lift the rear
  ones, drive the mid pairs, and swing a cab towards the path and a trailer the
  other way, by controlling bones after the animation pose as `Control_Bone`
  does. Motorcycles use the four-wheel suspension rules.
- Not yet: dust, dirt and powerslide effects and sounds (particles, audio), and
  `W3DTankDraw`'s tread scrolling.

All seventeen scenarios pass with v16 hashes (every-tick replay equality, no
overlaps or rejected commands, all orders finished, required bridge traversals),
now with every map object loaded: 142 logic objects on Tournament Desert (756 with
Contra) and 492 on Winding River (764):

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `387bf78ac1f6f6a7` | `30bde09daa5df1f4` |
| AI/status orders | `351c9012c36ef05f` | `b4662293ac58b4b1` |
| Bridges | `88acb55e28cf4839` | `e8f4bc1e7cc349d5` |
| Traffic, Tournament Desert (three move-asides) | `f41dc0ff3e07af9b` | `591b2d9156d603b0` |
| Traffic, Winding River | `3d4582ed81caebdb` | `77920fe2a9ae1647` |
| Long map travel | `d9d9b1034c61a9e9` | `701f331ecfb89675` |
| Crusader through an enemy Ranger (crushed at frame 132) | `0b10383d2dfa496a` | |
| Crusader through an allied Ranger (steps aside) | `71d778e59d93a55c` | |
| Comanche out and back (`m4-air`; Contra `AirF_AmericaVehicleComanche`) | `f79bee39535e32a9` | `8b1fc73e686ebcd1` |
| Group move across Tournament Desert, then gather (`m4-group`) | `3594e1aa80ee9440` | |

Asset-free fixtures: `87aaf71c7fb391f4` (ground), `5061e484e5c3d3d6`
(bridge/deck stop), `f9a106e167100524` (head-on traffic, resolved by a
move-aside), `2931e91aa547ffcb` (AI/status orders). Unit tests also cover a tank
squishing enemy infantry, allied infantry stepping aside, a hovering body
climbing to and holding its preferred height (over ground and water), box,
circle and fence footprints with pinched-gap closing, map-object sorting and
owners, and a tank routing around a map structure. Earlier hash tables are
superseded.


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

### Model condition states (render-only)

Drawables pick their art from the object's condition flags as `W3DModelDraw` does
(`crates/ih-client/src/condition.rs`):

- Parsing as `parseConditionState`: `DefaultConditionState` copied into later
  states (animations replaced as soon as a state names its own),
  `AliasConditionState`, `TransitionState` pairs, `IgnoreConditionStates`,
  `Model = NONE`, accumulated `Show/HideSubObject` lists, `Animation` /
  `IdleAnimation` with distance covered and repeat counts, `AnimationMode`,
  `TransitionKey`, `WaitForStateToFinishIfPossible`, `Flags`,
  `AnimationSpeedFactorRange` and `WeaponMuzzleFlash`.
- `findBestInfo` (`SparseMatchFinder`): most shared flags, then fewest flags the
  object lacks, the first state winning ties.
- `setModelState`: transition states between transition keys, waiting for the
  current animation to finish, keeping hidden sub-objects while the model stays the
  same; `adjustAnimation` (random choice, a different idle animation each time,
  random/first/last/maintained start frames, speed factor); every animation mode;
  idle-animation cycling and `RESTART_ANIM_WHEN_COMPLETE`; walk cycles synced to
  movement speed (`adjustAnimSpeedToMovementSpeed`).
- Sub-object visibility as `doHideShowSubObjs` (full name, then the part after the
  container name; sub-objects on bones below a hidden one's bone follow it),
  muzzle flashes hidden, headlights shown only at night. Models are built once;
  each instance carries a hidden mask.
- The viewer and `map-render --scenario` set `MOVING` while a move order is under
  way and not blocked for more than a quarter second (`AIInternalMoveToState`),
  plus the map's `NIGHT`/`SNOW`. Placed map objects use the same best-match rule
  for their idle state.
- `ih-tools draw-check` parses every template's states. Stock: 2,585 draw modules,
  23,042 states and 4,774 transitions, all with an idle state; 68 of 5,918 models
  and 51 of 3,146 animations do not resolve (debug tiles, unshipped snow/night
  variants, files without the named animation). Contra: 6,040 modules, 37,024
  states, 7,279 transitions, all with an idle state; 110 of 8,936 models and 129
  of 4,278 animations do not resolve.

Not yet: turret rotation and recoil, weapon/particle-system bones, the condition
flags that combat, damage, construction and garrisons set (M5), terrain decals,
shadows and track marks.

### Pathfinder (state hash v17)

From `AIPathfind.cpp` (`findPath`, `internalFindPath`, `findClosestPath`,
`findGroundPath`, `examineNeighboringCells` and its straight-line probe,
`checkChangeLayers`, `checkForMovement`, `checkDestination`,
`validMovementPosition`, `validMovementTerrain`, `clearCellForDiameter`,
`buildActualPath`, `buildGroundPath`, `prependCells`, `Path::optimize`,
`optimizeGroundPath`, `isLinePassable`, `isGroundPathPassable`,
`iterateCellsAlongLine`, `updateLayer`, the `PathfindCell` cost functions and
the sorted open list), `AIUpdateInterface::computePath` and
`TerrainLogic::objectInteractsWithBridgeLayer` / `Bridge::isCellOnEnd`:
`crates/ih-sim/src/pathfind.rs`.

- Terrain is tested at the cell a unit's centre is in, as in the original; the
  unit's footprint (`getRadiusAndCenter`) is tested only against units' cells.
  Parked allies cost three diagonals and mark the path blocked-by-ally (so they
  are asked to move), moving allies near the start cost the same, parked
  enemies that cannot be crushed block. Pinched cells, cliffs, obstacles and
  cells outside the coarse corridor carry the original costs; the search keeps
  the 30,000-cell pool limit and `costToGoal`'s heuristic.
- `computePath`: flyers and a clear line to an intermediate follow-path point
  go straight; otherwise `findPath`, then `findClosestPath` (the reachable cell
  nearest the goal where the unit may stop). Stuck units repath with `findPath`
  through units' cells. Paths end on cell coordinates.
- Bridge layers cover their padded extent with impassable off-deck cells and
  fall back to the ground beyond it (`getCell`); a following unit takes the
  layer of its path segment while on the deck or at an end within ten units of
  the deck height (`updateLayer`), so it drives off a bridge without an exit
  node. Off-deck layer cells are valid for `fixInvalidPosition`.
- Group moves follow `findGroundPath`'s six-cell-wide path (goal moved to a
  cell clear for the full width, narrow cells cost more, parked units that may
  not be crushed narrow the width), optimised by `optimizeGroundPath`.
- Removed stand-ins: the clearance-radius search and traffic-avoiding repath,
  the group path's kept final goal (dropped from the hash, hence v17), and the
  four-cell skip of intermediate points.
- ironhour differences: the zone manager's hierarchical pre-pass is the coarse
  zone corridor; the logical extent is the whole grid (computer players do not
  path off the map); no ignored obstacle, dozer exception or `patchPath` yet
  (they need structures as objects, M5); destinations beyond the map are
  rejected rather than clipped; a step over a deck's side still stops (falling
  is not modelled), but the ends are open.

All seventeen scenarios pass with v17 hashes (every-tick replay equality, no
overlaps or rejected commands, all orders finished, required bridge traversals):

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `50ac676da45a40c6` | `52945e86afc31803` |
| AI/status orders | `435bb8287e60d776` | `f1a093696099c2ca` |
| Bridges | `c32127bf385468d4` | `7a203c2dd1c44503` |
| Traffic, Tournament Desert (three move-asides) | `f102d581f183da5e` | `638f23a99abb4f01` |
| Traffic, Winding River | `5a5a63357f0e945c` | `a4318decb3944f73` |
| Long map travel | `04f1dcf92f79ff61` | `6e3453a4602374bf` |
| Crusader through an enemy Ranger (crushed at frame 132) | `1eb317d2ffd8664b` | |
| Crusader through an allied Ranger (steps aside) | `edb54d8c71a1e88d` | |
| Comanche out and back | `7ee5d752c9169f04` | `1647afb075f3cf6f` |
| Group move across Tournament Desert, then gather (`m4-group`) | `c306251d7532e9d4` | |

Asset-free fixtures: `47b60629732b08cf` (ground), `6102ebcddb58c498` (bridge;
its deck now sits 8 units up, within the layer's height tolerance),
`83d672c19cc9a9ed` (head-on traffic), `85a16f8f5203655c` (AI/status orders).
The tight-valley group jam and the mixed tank and infantry start deadlock noted
earlier have not been re-checked in the window since this port.

### Still required for M4

The milestone check (units path across a map on command; a command log gives the
same state hashes everywhere) is met for ground, helicopter and group movement: seventeen
stock and Contra scenarios replay identically, and the asset-free fixtures assert
fixed hashes on Windows, Linux and macOS in CI. Runs on installed data have only
been made on Windows. Remaining M4 work (movement and the simulation core):

- Pathfinder: `patchPath`, the ignored obstacle and dozer rules (with
  structures as objects), the zone manager's own blocks and hierarchical
  search, attack and approach paths (M5).
- Group moves: formations and waypoint appends (with the control bar, M6).
- Traffic: overlapping bodies (and with them vehicle crush damage), crowded
  bottleneck recovery. Occupied goals or routes with no passing room wait until
  space is free.
- Drawables: tread scrolling (`W3DTankDraw`), which needs per-instance texture
  offsets in the renderer.

Moved to M5, because they need weapons, bodies, containers or production:

- `THRUST` locomotors: every one in stock and Contra drives a projectile
  (missiles, shells, flame and toxin streams), so they come with
  `MissileAIUpdate`. Airfield take-off and landing (`JetAIUpdate`).
- Damage-state speed penalties, bouncing and falling damage, rubble cells and
  obstacle removal on death, bridge damage, destruction and repair.
- Incremental zone updates (obstacles change only when things are built or
  destroyed) and the wall-top layer (reached only by placed or garrisoned units).
- Combat, guard, transport, mood and other AI states; disabled side effects
  (inertia, held transports, tint, special-power timers, riders, power).
- Map object properties (health, veterancy, names for scripts), build lists and
  players' bases.
- Turrets, recoil, weapon and particle-system bones; crush damage states.

Your review: [the M4 checklist](docs/m4-movement.md#review-checklist), and the
M3 visual review ([docs/map-viewer.md](docs/map-viewer.md)).

## M5. Combat and economy (in progress)

### Bodies, damage and death (state hash v18)

From `ActiveBody.cpp`, `StructureBody.cpp`, `HighlanderBody.cpp`,
`ImmortalBody.cpp`, `InactiveBody.cpp`, `Armor.cpp`, `Damage.h`,
`Object::attemptDamage`/`attemptHealing`/`kill`/`onDie`, `DieModule.cpp`
(`DieMuxData`), `DestroyDie.cpp`, `SlowDeathBehavior.cpp`, `AIDeadState`
(`markAsDead`), `GameLogic::destroyObject` and `processDestroyList`, and
`ThingTemplate::parseArmorTemplateSet` with `SparseMatchFinder`:
`crates/ih-sim/src/body.rs` and `death.rs`.

- `Armor` definitions (`Default` and per damage type, as percentages) and each
  template's `ArmorSet`s: a definition's first `ArmorSet` replaces the inherited
  ones, and the set with the most matching conditions (then the fewest extra)
  applies. Unresistable damage ignores armour.
- Bodies keep health, maximum and initial health, subdual damage (with the
  `DISABLED_SUBDUED` switch), the damage state (`UnitDamagedThreshold` and
  `UnitReallyDamagedThreshold` from GameData) and the last damage. Highlander
  bodies stop at one hit point except to unresistable damage, immortal ones
  always do, inactive ones die only to unresistable damage. Healing caps at the
  maximum. Crossing a quarter of health draws the fear voice's random roll.
- Death runs each die module in order. `DestroyDie` (death types, veterancy,
  exempt and required status) destroys the object, which is removed at the end
  of the frame. `SlowDeathBehavior` marks the AI dead, rolls among the
  applicable slow-death modules by probability plus overkill bonus, sinks the
  body after its delay (holding it), runs its midpoint and final phases and
  destroys it; flung deaths get their random force. Units in the dead state stop
  (infantry slow by a fifth each frame) and take no orders.
- Crushing deals `HUGE_DAMAGE_AMOUNT` of `CRUSH` with a `CRUSHED` death
  (`SquishCollide`), so crushed units die through their own die modules.
- A `damage` scenario command (named damage and death types) stands in for
  weapons and scripts. Drawables show `DAMAGED`, `REALLYDAMAGED`, `RUBBLE` and
  `DYING`.
- Not yet: damage FX, body particle systems, damaged sounds and the fear voice
  itself; retaliation; killing pilots and garrisons and status damage; veterancy
  and upgrade armour sets; structure rubble height and its pathfinding update;
  the phases' FX, object creation lists and weapons (their random choices are
  drawn); slow-death subclasses (helicopter, jet, battle bus and others); the
  game LOD's slow-death scale (always full); a bounce model for flung bodies;
  other die and damage modules. The map's own objects are still drawn by the
  static scene, so one destroyed by a command stays visible.

All seventeen scenarios pass with v18 hashes:

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `3d7410f1f15adc41` | `9caea83f77aeb8fa` |
| AI/status orders | `6879bc25a296c681` | `3cc9a469fee93897` |
| Bridges | `0202bf4b647acc86` | `3ab5c85622b787de` |
| Traffic, Tournament Desert (three move-asides) | `13cdd24e8a137a79` | `dee4ba3edf8a32c0` |
| Traffic, Winding River | `99f1e9daf2674bec` | `5f75eb20ed989183` |
| Long map travel | `39cad5878bdf5bcb` | `98f226b7b9c12e0a` |
| Crusader through an enemy Ranger (crushed at frame 132, removed by its `DestroyDie`) | `4726f75067e747b4` | |
| Crusader through an allied Ranger (steps aside) | `df73bb880242f11a` | |
| Comanche out and back | `ff6cdd3898328ab8` | `259e95cb35e7dd79` |
| Group move across Tournament Desert, then gather (`m4-group`) | `33dafea8b9bfcfee` | |

Asset-free fixtures: `1ca894b4bfa34d26` (ground), `c9162aeaa9b686da` (bridge),
`c2893ca6584acd42` (head-on traffic), `afafec6389d4c4e1` (AI/status orders).
`tests/bodies.rs` covers armour, damage states, healing, the three special
bodies, `DestroyDie` death types, the end-of-frame removal, and a slow death
that sinks and is destroyed on schedule with matching hashes in two runs.

### Weapons and the attack order (state hash v19)

From `Weapon.cpp` (`WeaponTemplate` fields, `getAttackRange` and the other
accessors with `RATIONALIZE_ATTACK_RANGE`, `fireWeaponTemplate`,
`dealDamageInternal`, `estimateWeaponTemplateDamage`, `Weapon` construction,
`reloadWithBonus`, `privateFireWeapon`, `preFireWeapon`, `getStatus`,
`isWithinAttackRange`, the weapon store's delayed damage), `WeaponSet.cpp`
(`parseWeaponTemplateSet`, `updateWeaponSet`, `getVictimAntiMask`,
`chooseBestWeaponForTarget`), `PartitionManager`'s boundary distances, and
`AIStates.cpp` (`AIAttackState`, `AttackStateMachine`, the aim, fire and
approach states and their range transitions) with `StateMachine`'s transition
order: `crates/ih-sim/src/weapons.rs` and `attack.rs`.

- Weapon definitions and each template's `WeaponSet`s (a definition's first set
  replaces the inherited ones; the best condition match applies). Objects get
  their weapons with full clips when created.
- Firing: clips, clip reloads, delays between shots (random between `Min` and
  `Max`), pre-attack delays per shot, attack or clip, shared reload times, one
  barrel per model. Range uses the 2D distance between bounding circles, less a
  quarter cell. Instant weapons deal damage now; slow ones after the flight time
  (distance over `WeaponSpeed`). Area damage reaches objects whose bounding
  spheres are within the radius, with the primary and secondary amounts and the
  allies, enemies, neutrals, self, suicide, similar and airborne rules, and
  directional damage within `RadiusDamageAngle`.
- `attack` and `attack_position` commands: the unit aims, fires when ready, aims
  again while reloading, and closes in when out of range. The attack succeeds
  when the victim dies and fails when the attacker runs out of ammo. As in the
  original, aiming and firing alternate frames, so shot delays round up to even
  frame counts.
- Relationships until players exist: the same owner is an ally, the neutral
  player is neutral, other owners are enemies.
- Stand-ins: a projectile weapon's damage lands at its (scattered) destination
  after the flight time instead of launching its projectile object
  (projectiles come next); the approach paths to the victim and stops once in
  range (`findAttackPath` is not ported); the chase is the approach (no
  pursuit); aiming turns the body (no turrets).
- Not yet: weapon bonuses (all neutral), FX, OCLs, fire sounds, lasers and
  streams, historic bonuses, scatter-target tables, shockwaves, request-assist,
  leech range, continued attacks, firing line of sight, pitch limits, weapon
  locks, auto-targeting, retaliation and guard.

All scenarios pass with v19 hashes. `m5-fight` (stock): a Crusader closes on
another and kills it with its cannon while two Rangers duel; both attacks
succeed.

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `92dcce6ece5fb53d` | `84c18d5059ad4895` |
| AI/status orders | `341bd9068e5e1233` | `a74fb36a525e6400` |
| Bridges | `2f01fea0569a04fa` | `a4afc94009038079` |
| Traffic, Tournament Desert | `132ffa7db58e479c` | `397aa432361b0306` |
| Traffic, Winding River | `535422fdc3ea0a76` | `b53643c864df3626` |
| Long map travel | `eac430d5a5668475` | `61be301a8529eec5` |
| Crush enemy / crush ally | `c9f1be7a3d420baf` / `4696436a97b705b6` | |
| Comanche out and back | `7d6c2873ea937e3c` | `f7d79dfeb3d1b415` |
| Group move (`m4-group`) | `4f499b57d231d345` | |
| Fight (`m5-fight`) | `cc0f042ff17f26d9` | |

Asset-free fixtures: `b09fcd2ac1f31379` (ground), `3342568b41d0ffed` (bridge),
`5e5c5299b925f3f7` (head-on traffic), `73ae2a0f752820ea` (AI/status orders).
`tests/weapons.rs` covers firing cadence, clip reloads, closing into range,
area damage that spares allies, and delayed projectile damage with matching
hashes in two runs.

### Projectiles (state hash v20)

From `DumbProjectileBehavior.cpp`, `BezierSegment.cpp` and `BezFwdIterator.cpp`,
the projectile branch of `WeaponTemplate::fireWeaponTemplate`,
`WeaponStore::handleProjectileDetonation`, `shouldProjectileCollideWith`,
`PhysicsBehavior::onCollide`'s hand-off to `projectileHandleCollision`, and
`PartitionManager::estimateTerrainExtremesAlongLine` (partition cells of
`PartitionCellSize`, each the highest ground sample a grid step apart):
`crates/ih-sim/src/projectiles.rs`.

- A weapon with a `ProjectileObject` creates that object for the launcher's
  owner. A dumb projectile flies a Bezier arc whose middle control points sit
  `FirstHeight`/`SecondHeight` above the highest terrain along the line, at
  `FirstPercentIndent`/`SecondPercentIndent` of the way, one forward-difference
  segment per frame at the weapon's speed (scaled by range with
  `ScaleWeaponSpeed`). It bends towards a moving victim at
  `FlightPathAdjustDistPerSecond`, faces along its path, and detonates at the
  end of the arc, at `MaxLifespan`, or when it touches something its weapon's
  `ProjectileCollidesWith` allows (always the intended victim, never the
  launcher). The detonation fires a fresh copy of the weapon from the
  projectile at its position, crediting the launcher, then destroys or kills
  the projectile.
- Stock Crusader shells (`GenericTankShell`) now fly as objects.
- Not yet: launch bones (projectiles start at the launcher's position), the
  launcher's velocity, bridge layers under a flight, garrison-clearing hits,
  tumbling (its random rates are drawn), and collisions are tested as bounding
  spheres. Projectiles without a `DumbProjectileBehavior` (missiles and other
  behaviours) still land their damage after the flight time.

All scenarios pass with v20 hashes:

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `783b74a096faa502` | `7bf11f35573fb3e8` |
| AI/status orders | `454e09a9635fb0f4` | `5f03fe2e895da75d` |
| Bridges | `d505e5ba8d16b15f` | `4e30134741e84e64` |
| Traffic, Tournament Desert | `345a0cafd845ee19` | `361b09020a0582d7` |
| Traffic, Winding River | `dc58499c45f191cb` | `dc69051c5fd90b1f` |
| Long map travel | `6217fd0e58bee944` | `9d3ed54ec4fd997c` |
| Crush enemy / crush ally | `2cc44a4fd8e6f56e` / `076f619a4a6a6da5` | |
| Comanche out and back | `166caea28c23d331` | `380cd86ffd0efca4` |
| Group move (`m4-group`) | `a8444f5cbda807d0` | |
| Fight (`m5-fight`) | `f476524da4693a07` | |

Asset-free fixtures: `a4804c1138cc3e1c` (ground), `0ff7f69f7b2ea864` (bridge),
`66d284ec19e96e74` (head-on traffic), `1e477c5d7e3dc353` (AI/status orders).
`tests/projectiles.rs` covers a shell arcing to its victim and detonating once
(with matching hashes in two runs) and a structure in the way taking the hit.

### Missiles (state hash v21)

From `MissileAIUpdate.cpp` (launch, ignition, the no-turn and attack states,
lock-on, kill and kill-self, fuel, collisions and detonation) and the `THRUST`
locomotor (`moveTowardsPositionThrust`, `calcDirectionToApplyThrust`,
`tryToRotateVector3D`, `tryToOrientInThisDirection3D`, `getForwardSpeed3D`,
and the template values capped by `setMaxAcceleration`/`setMaxTurnRate`/
`setMaxSpeed`): `crates/ih-sim/src/missiles.rs`.

- A weapon whose projectile has a `MissileAIUpdate` launches a missile object:
  an initial push along the launch direction (raised towards higher targets),
  no thrust until `IgnitionDelay`, then ignition (armed, fuel started), straight
  flight for `DistanceToTravelBeforeTurning`, then steering with thrust inside
  `MaxThrustAngle` and the body turning along its velocity (missiles keep a
  pitch). Within `DistanceToTargetForLock` it cheats straight at the target.
- It detonates on touching what its weapon collides with or the ground near its
  goal, or out of fuel with `DetonateOnNoFuel`; a tracked target that is gone
  makes it destroy itself after `KillSelfDelay`. Detonation fires the weapon
  from the missile, credited to the launcher (`MissileCallsOnDie` kills the
  missile too).
- `m5-missiles` (stock): a Missile Defender kills a Crusader with its missiles.
- Stand-ins and gaps: missiles start at the top of the launcher's geometry
  (`GeometryHeight`) facing its way, without launch bones or the launcher's
  velocity; the flight integrates gravity, aerodynamic friction and motive
  forces itself; ground contact counts only while descending. Not yet:
  ignition FX and exhaust, countermeasures and jamming, garrison hits, bridge
  layers, dive heights, damaged-state locomotor values.

All scenarios pass with v21 hashes:

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `76d08ba8d1d9e967` | `1bc548d946a441c7` |
| AI/status orders | `138037e0d9802541` | `d2ec8c4c2738f26a` |
| Bridges | `186b888868f8965c` | `a25acd32dfa1581b` |
| Traffic, Tournament Desert | `79a05ff5325dd5a6` | `afeabd9ecc073788` |
| Traffic, Winding River | `5bd237bf5160b4c4` | `45e0708f84c7da6c` |
| Long map travel | `d553f605bd14c037` | `65f70eaae5a83053` |
| Crush enemy / crush ally | `adcb1415763bf381` / `d8ce6fca23ae31f8` | |
| Comanche out and back | `7b5b5ee12e96839e` | `39a8fdc8cfba7d7f` |
| Group move (`m4-group`) | `1b644470378e8a37` | |
| Fight (`m5-fight`) | `ce416d6364f2058e` | |
| Missiles (`m5-missiles`) | `d2fd2dd4fd008335` | |

Asset-free fixtures: `5d61b43b47c11dcf` (ground), `6ecd8cf53572cdbf` (bridge),
`39eef15d88b28099` (head-on traffic), `ccbf347b2bfe827c` (AI/status orders).
`tests/missiles.rs` covers the state sequence of a missile that turns onto a
target off its launch line and hits it once (matching hashes in two runs), and
a missile that destroys itself when its target is gone.

### Auto-targeting and guard (state hash v22)

From `AIIdleState` (`AIStates.cpp`), `AIUpdateInterface::getNextMoodTarget`,
`resetNextMoodCheckTime`, `AI::findClosestEnemy` and its partition filters,
`AI::getAdjustedVisionRangeForObject`, `Object::isAbleToAttack`,
`WeaponSet::getAbleToAttackSpecificObject`/`getAbleToUseWeaponAgainstTarget`,
`AIGuard.cpp` (`AIGuardMachine`, its inner, outer, return, idle, crate and
attack-aggressor states, `lookForInnerTarget`, `hasAttackedMeAndICanReturnFire`,
`ExitConditions`) and `AIGuardState`: `crates/ih-sim/src/targeting.rs` and
`guard.rs`.

- Idle units enter `AIIdleState` (a random first sleep of up to two seconds)
  and, with `AutoAcquireEnemiesWhenIdle = YES`, check every
  `MoodAttackCheckRate` (the first check randomly offset by up to half of it)
  for the closest enemy within vision range (times `GuardOuterModifierHuman`)
  that a weapon can already reach; they attack it as the AI's own order, so they
  do not chase it out of range. Enemy buildings count only with
  `ATTACK_BUILDINGS` or as base defences; insignificant buildings are skipped
  with `AttackIgnoreInsignificantBuildings`.
- `guard` command (a position): the unit returns to its post, scans every
  `GuardEnemyScanRate` within its inner range (vision times
  `GuardInnerModifierHuman`), attacks what it finds until it dies or leaves that
  range, pursues it within the outer range for `GuardChaseUnitsDuration`, then
  returns; anything that damages it while it guards is attacked back and
  followed. The original's transition limit (twenty nested transitions) applies.
- Player orders clear the AI-picked flag, so attacks ordered by the player
  still close in.
- `m5-guard` (stock): two Rangers guarding a point shoot down an enemy Ranger
  and return to their posts.
- Not yet: guarding objects or areas, the flying-only and no-pursuit modes,
  entering and hijacking guards, crates, team targets, attack priorities, mood
  for computer players (every player is treated as human), fog, stealth, line
  of sight, retaliation by nearby friends.

All scenarios pass with v22 hashes:

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `a5379f8623070107` | `0e66aabd7e3487e1` |
| AI/status orders | `6b6739d6bc7f2aea` | `c458c305abe7e6c5` |
| Bridges | `b2fd995c6680b812` | `5da7b6c20c0b3ac4` |
| Traffic, Tournament Desert | `efc270b2ae6b3cc2` | `efedc6e7480ff8c6` |
| Traffic, Winding River | `a84b72351f97ca87` | `983bc9a98c5cdf9d` |
| Long map travel | `cca97896b540f998` | `20137a0db77db178` |
| Crush enemy / crush ally | `86b3430e9e325355` / `95d9e726b8428f3e` | |
| Comanche out and back | `d78d09f6ef3f8fe3` | `76fde6240f1be8ed` |
| Group move (`m4-group`) | `465aecb74284d311` | |
| Fight (`m5-fight`) | `673237b5acaa6cbb` | |
| Missiles (`m5-missiles`) | `29267b4d3bccaab8` | |
| Guard (`m5-guard`) | `0210974d049f4be0` | |

Asset-free fixtures: `83a9ed56e8160497` (ground), `18c270831950fb5a` (bridge),
`45f03e44a29c2b00` (head-on traffic), `7b652696f20dc574` (AI/status orders).
`tests/targeting.rs` covers an idle unit shooting an enemy that walks into
range (with matching hashes in two runs), not chasing an auto-acquired target
that runs, and a guard that returns to its post, hunts an intruder within its
range and goes back.

### Orders in the window

`ih-game` now gives combat orders to the selection: right-click an enemy to
attack it (unarmed units move there), Ctrl+right-click to force-fire at any
object or at the ground, and Alt+right-click to guard the point. Weapons'
projectiles (shells, missiles) are loaded with the scenario's units and drawn,
missiles pitched along their flight. `m5-skirmish` sets up a small battle to
command: try it with
`cargo run --release -p ih-game -- --profile profiles/stock.toml --scenario scenarios/m5-skirmish.toml`.
Map objects destroyed in the logic are still drawn by the static scene.

### Money, power and unit production (state hash v23)

From `Money.cpp`, `Energy.cpp`, `ProductionPrerequisite.cpp`,
`ThingTemplate::calcCostToBuild`/`calcTimeToBuild`/`getBuildFacilityTemplate`,
`BuildAssistant::canMakeUnit`/`isPossibleToMakeUnit`, `Player::canBuild`/
`canBuildMoreOfType`, `ProductionUpdate.cpp` and the default, queue and
supply-centre production exits: `crates/ih-sim/src/production.rs`.

- Players have money (`deposit` command; withdrawals never go below zero) and
  sciences (`grant_science`). Power is the sum of owned objects'
  `EnergyProduction`: positive values produce, negative ones consume.
- `produce` command (factory, unit): the unit needs a build button in the
  factory's `CommandSet`, its `Buildable` status, every `Prerequisites` line
  (any one object of an `Object` line, each `Science`), room under
  `MaxSimultaneousOfType` (queued units count) and a free queue slot
  (`MaxQueueEntries`, 9). The cost is withdrawn when queued.
  `cancel_production` refunds it in full.
- Build time is `BuildTime` in frames, slowed by low power
  (`LowEnergyPenaltyModifier`, `Min`/`MaxLowEnergyProductionSpeed`; no power at
  all counts as a supply ratio of 0) and sped up by `MultipleFactory` for each
  extra factory of the unit's first prerequisite. It is recomputed every frame,
  as in the original.
- `QuantityModifier` builds several units per entry. Factories with
  `NumDoorAnimations` open the door (`DoorOpeningTime`) before the unit leaves
  and keep it waiting open while units follow.
- Exits: the unit appears at `UnitCreatePoint`, faces the factory's way and
  follows an exit path (as the AI's own order) to `NaturalRallyPoint`, then to
  the rally point (`set_rally_point`) if one is set. It ignores collisions with
  its factory while leaving. Queue exits (War Factory) wait `ExitDelay` between
  units after the `InitialBurst`.
- `m5-production` (stock): a Barracks makes three Rangers and a War Factory two
  Crusaders that drive to a rally point.
- Not yet: upgrades in the queue, handicaps and faction cost/time changes,
  dozer construction and structures under construction, spawn-point exits
  (bones), parking places (airfields), the airborne-creation push, power
  bonuses and disabled power plants, sold factories, max-simultaneous link
  keys, script-disabled factories, computer players (`ONLY_BY_AI` units are
  refused), the exit path's path-through-units flag and its own state ID (it
  uses the follow-path state), supply gathering, sciences from player
  templates.

All scenarios pass with v23 hashes:

| Scenario | Stock | Contra |
|---|---|---|
| Desert (`m4-desert` / `m4-contra`) | `fdc79f7b9c1042f0` | `95a7b54cf948cfa5` |
| AI/status orders | `4c017667743f7032` | `3eeba263bed102f2` |
| Bridges | `088adca66c474509` | `e3cc1d0f19136396` |
| Traffic, Tournament Desert | `da7a38a29b26df8c` | `2185195b0c48d485` |
| Traffic, Winding River | `272ca6b786b2f2c1` | `f5fb263d6afe6193` |
| Long map travel | `7e18d2bba63867c1` | `3356193203ec647a` |
| Crush enemy / crush ally | `bae37b906dff2fd0` / `35f544b0cd855ea7` | |
| Comanche out and back | `8578cd5c5ebf036c` | `0926024f055d6391` |
| Group move (`m4-group`) | `24bf3d4ddf4ca127` | |
| Fight (`m5-fight`) | `2dd640aed16fa1d7` | |
| Missiles (`m5-missiles`) | `24604e8be0711ef7` | |
| Guard (`m5-guard`) | `58b0731ca1c75c98` | |
| Skirmish (`m5-skirmish`) | `3be775044bd9bdc7` | |
| Production (`m5-production`) | `ac3bb3767d236ded` | |

Asset-free fixtures: `9131a353573eafc4` (ground), `424d3a3306db6ff1` (bridge),
`bc2ec9dcb92ea481` (head-on traffic), `7dc29ebf6aca1043` (AI/status orders).
`tests/production.rs` covers cost, build time and the exit to the natural rally
point; the power and multiple-factory time changes; prerequisites, money, the
nine-entry queue and refunds; a queue exit's delay between a batch's two units
and the rally point; the door delay; and matching hashes in two runs.
