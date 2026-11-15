# M4 movement, traffic and AI orders

These are simulation slices within M4, which remains in progress. They load real
unit definitions and terrain, run recorded move commands at 30 Hz, and check
every tick's state hash against a second independent run. The native viewer can
also display and command the spawned units.

From the repository folder:

```powershell
cargo run --release -p ih-tools -- sim-run scenarios/m4-desert.toml --trace out/desert-hashes.txt
cargo run --release -p ih-tools -- --profile profiles/contra-x-beta2.toml sim-run scenarios/m4-contra.toml --trace out/contra-hashes.txt
cargo run --release -p ih-game -- --scenario scenarios/m4-desert.toml
cargo run --release -p ih-game -- --profile profiles/contra-x-beta2.toml --scenario scenarios/m4-contra.toml
```

Stock spawns a Crusader and Ranger; Contra spawns its `Humvee1` and Ranger.
Contra deliberately leaves `AmericaTankCrusader` without ground movement, so the
stock scenario is not used as the Contra test. Each scenario runs 1,200 ticks
(40 seconds), moves both units east and returns the vehicle west after frame 480.
The headless runner fails on rejected commands or unfinished orders.

Units move with the ported original locomotors and physics (see PORTING.md, "Ground
locomotion and physics"). To follow one object, add `--watch ID`: it prints position,
facing, velocity, braking, AI state and blocking every 15 frames (`--watch-every N` to change).

Pristine bridge crossings have a separate stock/Contra demo:

```powershell
cargo run --release -p ih-tools -- sim-run scenarios/m4-bridges.toml --trace out/bridge-hashes.txt
cargo run --release -p ih-tools -- --profile profiles/contra-x-beta2.toml sim-run scenarios/m4-bridges-contra.toml
cargo run --release -p ih-game -- --scenario scenarios/m4-bridges.toml
cargo run --release -p ih-game -- --profile profiles/contra-x-beta2.toml --scenario scenarios/m4-bridges-contra.toml
cargo run --release -p ih-tools -- nav-check "Winding River" --radius 15
```

The bridge scenarios cross Winding River's horizontal bridge in both directions
and require two completed opposite-end traversals. Stock uses a Crusader; Contra
uses `Humvee1`. The mod removes the map's central bridge, so these scenarios use
a crossing present in both versions. `nav-check` reports bridge widths, entry
clearance and bank-to-bank paths without opening a window.
It also reports static zones, connected components and coarse/fine search work.
An optional query inspects a longer route:

```powershell
cargo run --release -p ih-tools -- nav-check "Winding River" --radius 15 --from 2258.3945,481.0763 --to 716.765,2681.8733
```

For live vehicle traffic, two vehicles exchange positions while a third remains
parked in their path. Both the open-ground and bridge variants require detours:

```powershell
cargo run --release -p ih-tools -- sim-run scenarios/m4-traffic.toml
cargo run --release -p ih-tools -- sim-run scenarios/m4-traffic-bridges.toml
cargo run --release -p ih-tools -- --profile profiles/contra-x-beta2.toml sim-run scenarios/m4-traffic-contra.toml
cargo run --release -p ih-tools -- --profile profiles/contra-x-beta2.toml sim-run scenarios/m4-traffic-bridges-contra.toml
cargo run --release -p ih-game -- --scenario scenarios/m4-traffic-bridges.toml
```

These are 1,800-tick (60-second) scenarios. The headless runner checks live
geometry overlaps at every tick by default, including the initial spawn frame.
`require_no_overlaps = false` permits deliberately overlapping inputs for
experiments. `require_traffic_repaths` checks successful movement detours among surviving
objects; `require_bridge_traversals` checks complete opposite-end crossings.

`scenarios/m4-map-travel.toml` and `scenarios/m4-map-travel-contra.toml` exercise a
long route across Winding River over 6,000 ticks (200 seconds). They require all
orders to finish and at least two stock bridge traversals, or one in Contra:

```powershell
cargo run --release -p ih-tools -- sim-run scenarios/m4-map-travel.toml
cargo run --release -p ih-tools -- --profile profiles/contra-x-beta2.toml sim-run scenarios/m4-map-travel-contra.toml
```

The AI/status scenarios follow two points, pause the vehicle under overlapping
EMP/HELD timers, resume, then turn it in place. A Ranger is script-disabled,
explicitly re-enabled, and ordered to face the vehicle:

```powershell
cargo run --release -p ih-tools -- sim-run scenarios/m4-orders.toml
cargo run --release -p ih-tools -- --profile profiles/contra-x-beta2.toml sim-run scenarios/m4-orders-contra.toml
cargo run --release -p ih-game -- --scenario scenarios/m4-orders.toml
cargo run --release -p ih-game -- --profile profiles/contra-x-beta2.toml --scenario scenarios/m4-orders-contra.toml
```

Each has twelve timed checkpoints for position/stationarity, AI state, disabled
flags, facing and completed orders. A follow-path action counts as one order,
even when it contains multiple points. `[[check]] frame = N` inspects the snapshot
after N logic steps, before commands stamped N. Headless runs, offscreen renders
and hidden smoke checks enforce these assertions; interactive windows allow
manual commands without enforcing recorded checkpoints.
Position tolerances use world units; angle checks and their tolerances use radians.

New command kinds use these fields:

| Kind | Fields | Behavior |
|---|---|---|
| `follow_path` | `object`, `points = [[x,y], ...]` | Visit points in order |
| `face` | `object`, `target = [x,y]` | Turn in place toward a position |
| `face_object` | `object`, `target = id` | Face an object's current position |
| `disable` | `object`, `reason`, optional `frames` | Apply a separately timed reason; omit frames for indefinite duration |
| `enable` | `object`, `reason` | Clear that reason |

The thirteen original disabled reasons are supported. Reapplying a reason replaces
its deadline; other reasons keep their own timers. Timers clear at the end of the
deadline frame, and a retained ground order resumes on the following tick. New
motion orders are rejected while disabled; Stop still cancels the retained order.

In a scenario window:

| Control | Action |
|---|---|
| Left click a spawned unit | Select it; green ground ring shows selection |
| Right click terrain | Issue a move order to the selected unit |
| X | Stop the selected unit |
| Space | Pause/resume logic |
| Right drag | Pan (a drag is distinguished from a move-order click) |
| WASD/arrows, wheel, middle drag, Q/E, R/F, Home, O | Existing camera controls |

Recorded commands continue to fire at their scheduled frames even when manual
orders are given. Blocked or unsupported orders are reported in the console.
Only the scenario's spawned objects participate in logic; placed scenery supplies
static navigation obstacles and remains client-side scenery for now.
Move picking includes elevated bridge decks. Two-coordinate orders and spawns
choose the highest deck at their coordinate; explicit underpass destination
selection in the command/UI layer remains to be added.

For reproducible before/after renders:

```powershell
cargo run --release -p ih-tools -- map-render "Tournament Desert" --scenario scenarios/m4-desert.toml --sim-frame 0 --at 1150,950 --distance 550 --out out/move-start.png
cargo run --release -p ih-tools -- map-render "Tournament Desert" --scenario scenarios/m4-desert.toml --sim-frame 240 --at 1150,950 --distance 550 --out out/move-eight-seconds.png
cargo run --release -p ih-game -- --scenario scenarios/m4-desert.toml --smoke
```

## Implemented

- `ih-math`: f32 software transcendental boundary, pinned pure-Rust libm with
  `force-soft-floats`; no host C math or renderer operations drive logic.
- `ih-sim`: ordered objects and module slots, source logic RNG, frame/phase heap
  scheduler including its non-FIFO equal-priority behaviour and wake rules.
- Shared INI definition store in `ih-ini`; simulation has no renderer, client,
  wgpu or window dependency. Template field layers, reskins, ordered module tags,
  Add/Remove/ReplaceModule and normal locomotor units are available. Unimplemented
  gameplay modules remain explicit stubs and are counted by the runner.
- Ground A*: 10-unit cells, original 10/14 movement costs and 4/8/16 turn penalties,
  stable open-list ties, conservative footprint clearance, water/cliff/static
  obstacles and line-of-sight path shortening.
- Separate pristine bridge layers, end-only portals, footprint clearance at bank
  transitions, rail boundaries, slope-dependent deck height and ground underpass
  clearance. Widths come from posed W3D LEFT geometry and `BridgeScale`, loaded
  headlessly through the VFS. Path shortening preserves layer transitions. The
  grid graph is still an initial pathfinder, not the complete original implementation.
- Initial ground hierarchy follows the source's 10-by-10-cell blocks: local
  connected regions, bridge components and end portals, global equivalence and a
  coarse route before fine A*. Clearance classes have independent cached graphs.
  Disconnected goals skip fine search; a corridor with adjacent-zone slack narrows
  connected searches. Transient traffic can trigger an unrestricted fallback.
  Zone caches are derived data, omitted from logic hashes; warming them does not
  change paths or simulation state. Static terrain edits must invalidate them.
- Initial steering/ground integration uses parsed speed, acceleration, braking
  and turn rate. It follows a path corridor while the body turns. This is not yet
  the full original force, suspension and collision model.
  Intermediate waypoints must be reached before the next turn; terrain-blocked
  movement retries through the same repath pipeline as live traffic.
- Live traffic uses geometry bounding circles, source ungrouped path priority
  (dozers, vehicles, front unit, object-ID ties), blocked-frame accounting and
  delayed transient-obstacle repathing. Physics updates partition buckets as
  each position commits, so later updates in the same tick see current positions.
  Swept disc clipping prevents tunnelling and overlap while this initial corridor
  integrator is in use. Deck/underpass traffic is separated by vertical geometry;
  `NO_COLLIDE` objects do not block. A waiting unit displays idle art.
- Spawn/move/stop/destroy commands, integer time accumulation, spatial buckets,
  canonical versioned state hashes covering pending commands and scheduler order.
  State-hash format v4 includes AI states, queued path points, outcomes/counters,
  facing targets and disabled masks/deadlines, along with previous movement state.
  Earlier hash logs are obsolete and need regenerating.
- Movement AI states use the original IDs for idle, move-to, follow-path,
  face-object and face-position. Orders finish or fail back to idle; replacements
  and Stop record interruptions. A failed later path leg or destroyed face target
  is reported as failure, and `require_no_failed_orders` defaults to true.
- Disabled module eligibility follows the source's any-intersection mask rule:
  AI callbacks allow HELD, physics callbacks allow all reasons. Active ground
  actions are suspended while disabled in this integrator. This is separate from
  the stored AI action, so clearing a timer does not discard its order.
- Render-only interpolation, idle/MOVING art and animations, selection and ground
  picking. Background/minimised scenario windows continue logic without drawing.

## Verification and remaining work

Stock and Contra command logs each complete all orders with every-tick replay
hash equality locally. The tests include a fixed synthetic movement hash that
also runs in the existing Windows/Linux/macOS CI matrix without game assets.
Cross-platform runtime equality on installed game data has not yet been exercised.
Locally, all 110 workspace tests pass, including fixed ground, bridge, traffic and AI/status hashes
in debug/release. Stock and Contra bridge replays complete both crossings, with
every-tick equality and no rejected or unfinished orders. Hidden-window bridge
smoke checks pass; the stock crossing still was inspected offscreen. Contra's
Winding River reports a missing scenery model `CBOldMslm_B`, which remains a
visible asset warning.
The four traffic scenarios complete all orders over 1,800 ticks, with no overlaps
at any checked frame and matching replay hashes. Both bridge runs complete two
opposite-end traversals past a parked vehicle. Offscreen traffic stills were
inspected. Native scenario cameras initially frame all spawned units.
Hierarchy checks cover disconnected-goal rejection, reduced fine search work,
traffic corridor fallback, footprint-specific connectivity, cache invalidation,
clone isolation and seeded-grid reachability against unrestricted search. Cache
warmup leaves simulation hashes and movement unchanged.
Both long-route scenarios complete with every-tick replay equality and no overlaps
or rejected orders. The stock unit traverses two bridges; Contra uses one.
Both AI/status scenarios complete all orders with every-tick equality and all
twenty-four checkpoints passing across stock and Contra.
Disabled/final facing renders were inspected; both AI/status native hidden-window
surface, resize, rendering and encountered-checkpoint checks pass.

The traffic slice is conservative circle avoidance, not complete retail collision
parity. Rectangle narrow-phase contacts, collision forces, crushing, formations,
allied move-away requests and crowded bottleneck deadlock recovery remain pending.
An occupied destination or a route with no passing room stays pending until space
becomes available; this slice does not silently move parked units out of the way.

M4 still needs multi-surface/crusher zone equivalence and incremental terrain-zone
updates, bridge damage/destruction and
repair transitions, the remaining traffic behavior, locomotor types and physics,
combat/guard/transport/mood/temporary AI states, disabled gameplay side effects,
map-wide object creation/lifecycle and
full condition-state drawable handling. Combat, economy and construction remain
M5 work. These are implementation tasks, not a claim that all registered module
classes already simulate gameplay.
The present ground integrator stops horizontal drive immediately on disable;
retail inertia/friction, falling/held transport behavior, disabled tint/effects,
special-power pausing, rider propagation and power-system effects remain pending.

## Moving out of the way and crushing

A Crusader drives east through a Ranger standing on its route. An enemy Ranger is
crushed (removed until death modules exist in M5); an allied one steps aside and
the Crusader passes:

```powershell
cargo run --release -p ih-tools -- sim-run scenarios/m4-crush-enemy.toml
cargo run --release -p ih-tools -- sim-run scenarios/m4-crush-ally.toml
cargo run --release -p ih-game -- --scenario scenarios/m4-crush-ally.toml
```

`sim-run` reports crushes as (frame, victim, crusher) and counts move-asides.

## Helicopters

A Comanche lifts off to its preferred height (100 above the ground; Contra's 70),
flies out and back, and hovers at each goal:

```powershell
cargo run --release -p ih-tools -- sim-run scenarios/m4-air.toml --watch 1 --watch-every 60
cargo run --release -p ih-tools -- --profile profiles/contra-x-beta2.toml sim-run scenarios/m4-air-contra.toml
cargo run --release -p ih-game -- --scenario scenarios/m4-air.toml
```
