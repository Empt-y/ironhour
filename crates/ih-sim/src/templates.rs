//! Logic templates, following `ThingTemplate.cpp`/`ModuleFactory.cpp`: ordered
//! module tags, object field layers, reskins, Add/Remove/ReplaceModule. Modules
//! without a runtime implementation remain explicit data-bearing stubs.
use ih_ini::{Block, Item, definitions::Definitions};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct ModuleDef {
    pub class: String,
    pub tag: String,
    pub kind: String,
    pub block: Block,
}
#[derive(Clone, Debug, Default)]
pub struct Template {
    pub name: String,
    pub fields: BTreeMap<String, Vec<String>>,
    pub modules: Vec<ModuleDef>,
    /// `ArmorSet` blocks (`body`).
    pub armor_sets: Vec<crate::body::ArmorSetDef>,
    /// `WeaponSet` blocks (`weapons`).
    pub weapon_sets: Vec<crate::weapons::WeaponSetDef>,
    /// `Prerequisites` (`m_prereqInfo`), in order.
    pub prerequisites: Vec<Prerequisite>,
}
/// One `ProductionPrerequisite`: an `Object` line (any one of its objects) or a `Science`.
#[derive(Clone, Debug, PartialEq)]
pub enum Prerequisite {
    Objects(Vec<String>),
    Science(String),
}
impl Template {
    pub fn values(&self, key: &str) -> &[String] {
        self.fields.get(key).map_or(&[], |v| v.as_slice())
    }
    /// A module's INI block by tag (empty if missing).
    pub fn modules_block(&self, tag: &str) -> ih_ini::Block {
        self.modules.iter().find(|m| m.tag == tag).map(|m| m.block.clone()).unwrap_or_else(|| ih_ini::Block {
            kind: String::new(),
            rest: String::new(),
            line: 0,
            items: Vec::new(),
        })
    }
    pub fn has_kind(&self, kind: &str) -> bool {
        self.values("KindOf").iter().any(|s| s == kind)
    }
    pub fn number(&self, key: &str, default: f32) -> f32 {
        self.values(key).first().and_then(|v| ih_ini::values::scan_real(v.as_bytes()).ok()).unwrap_or(default)
    }
    pub fn locomotor(&self) -> Option<&str> {
        let v = self.values("Locomotor");
        v.get(1).or_else(|| v.first()).map(|s| s.as_str())
    }
    pub fn radius(&self) -> f32 {
        self.number("GeometryMajorRadius", 5.0).max(self.number("GeometryMinorRadius", 0.0)) * self.number("Scale", 1.0)
    }
    /// `GeometryInfo::calcBoundingStuff`, used for live ground collisions.
    pub fn collision_radius(&self) -> f32 {
        let major = self.number("GeometryMajorRadius", 5.0).max(0.0);
        let minor = self.number("GeometryMinorRadius", 0.0).max(0.0);
        let radius = if self.values("Geometry").first().is_some_and(|v| v == "BOX") {
            ih_math::sqrt(major * major + minor * minor)
        } else {
            major
        };
        radius * self.number("Scale", 1.0).max(0.0)
    }
}

/// `LocomotorAppearance`, reduced to the ground movement models ported so far.
/// `THRUST` and `CLIMBER` use `Other` until their own ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    /// `TWO_LEGS`.
    Legs,
    /// `HOVER`: helicopters.
    Hover,
    /// `WINGS`: fixed-wing flight once airborne.
    Wings,
    /// `FOUR_WHEELS` and `MOTORCYCLE`.
    Wheels,
    Treads,
    Other,
}

/// Which client-side chassis model (`Drawable::calcPhysicsXform*`) the locomotor's
/// `Appearance` selects. Render-only: the logic uses [`Appearance`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChassisKind {
    /// `TWO_LEGS`, `CLIMBER` and `OTHER`: no chassis motion.
    #[default]
    None,
    Wheels,
    Motorcycle,
    Treads,
    HoverOrWings,
    Thrust,
}

/// The locomotor fields the chassis models read (`LocomotorTemplate`), converted
/// as the original parses them; defaults as its constructor sets them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chassis {
    pub kind: ChassisKind,
    /// `AccelerationPitchLimit`/`DecelerationPitchLimit` (radians).
    pub accel_pitch_limit: f32,
    pub decel_pitch_limit: f32,
    /// `BounceAmount` (radians per frame).
    pub bounce_kick: f32,
    pub pitch_stiffness: f32,
    pub roll_stiffness: f32,
    pub pitch_damping: f32,
    pub roll_damping: f32,
    pub thrust_roll: f32,
    pub wobble_rate: f32,
    pub min_wobble: f32,
    pub max_wobble: f32,
    /// `PitchInDirectionOfZVelFactor`.
    pub pitch_by_z_vel: f32,
    pub forward_vel: f32,
    pub lateral_vel: f32,
    pub forward_accel: f32,
    pub lateral_accel: f32,
    pub uniform_axial_damping: f32,
    pub rudder_degree: f32,
    pub rudder_rate: f32,
    pub elevator_degree: f32,
    pub elevator_rate: f32,
    pub has_suspension: bool,
    pub max_wheel_extension: f32,
    /// `FrontWheelTurnAngle` (radians).
    pub wheel_turn_angle: f32,
}
impl Default for Chassis {
    fn default() -> Self {
        Self {
            kind: ChassisKind::None,
            accel_pitch_limit: 0.0,
            decel_pitch_limit: 0.0,
            bounce_kick: 0.0,
            // "it's highly unlikely you want zero for the defaults for stiffness and damping"
            pitch_stiffness: 0.1,
            roll_stiffness: 0.1,
            pitch_damping: 0.9,
            roll_damping: 0.9,
            thrust_roll: 0.0,
            wobble_rate: 0.0,
            min_wobble: 0.0,
            max_wobble: 0.0,
            pitch_by_z_vel: 0.0,
            forward_vel: 0.0,
            lateral_vel: 0.0,
            forward_accel: 0.0,
            lateral_accel: 0.0,
            uniform_axial_damping: 1.0,
            rudder_degree: 0.0,
            rudder_rate: 0.0,
            elevator_degree: 0.0,
            elevator_rate: 0.0,
            has_suspension: false,
            max_wheel_extension: 0.0,
            wheel_turn_angle: 0.0,
        }
    }
}

/// `BIGNUM`: the original's "unlimited" default for braking and turn speed.
pub const BIGNUM: f32 = 99999.0;

/// Values are per logic frame (speeds), per frame squared (accelerations) and
/// radians per frame (turn rates), as `INI::parseVelocityReal` etc. convert them.
#[derive(Clone, Debug)]
pub struct Locomotor {
    pub name: String,
    pub surfaces: Vec<String>,
    pub speed: f32,
    pub min_speed: f32,
    pub acceleration: f32,
    pub braking: f32,
    pub turn_rate: f32,
    pub appearance: Appearance,
    pub min_turn_speed: f32,
    pub turn_pivot_offset: f32,
    /// `Extra2DFriction`, per frame.
    pub extra_2d_friction: f32,
    pub stick_to_ground: bool,
    pub can_move_backward: bool,
    pub downhill_only: bool,
    pub wander_width_factor: f32,
    pub wander_length_factor: f32,
    /// `Lift`, per frame squared.
    pub lift: f32,
    pub preferred_height: f32,
    pub preferred_height_damping: f32,
    /// `MaxThrustAngle` (radians).
    pub max_thrust_angle: f32,
    /// `SpeedLimitZ`, per frame.
    pub speed_limit_z: f32,
    pub behavior_z: BehaviorZ,
    pub circling_radius: f32,
    pub allow_airborne_motive_force: bool,
    pub apply_2d_friction_when_airborne: bool,
    /// `GroupMovementPriority`: 0 back, 1 middle (default), 2 front.
    pub move_priority: i32,
    /// `CloseEnoughDist`.
    pub close_enough_dist: f32,
    /// Render-only chassis motion.
    pub chassis: Chassis,
}

/// `LocomotorBehaviorZ` (`TheLocomotorBehaviorZNames`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BehaviorZ {
    #[default]
    NoZMotiveForce,
    SeaLevel,
    SurfaceRelativeHeight,
    AbsoluteHeight,
    FixedSurfaceRelativeHeight,
    FixedAbsoluteHeight,
    FixedRelativeToGroundAndBuildings,
    RelativeToHighestLayer,
}
impl Locomotor {
    pub fn air(&self) -> bool {
        self.surfaces.iter().any(|s| s == "AIR")
    }
}

/// `PhysicsBehaviorModuleData` fields the ground integrator uses; frictions per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicsData {
    pub mass: f32,
    pub forward_friction: f32,
    pub lateral_friction: f32,
    pub z_friction: f32,
    /// `AerodynamicFriction`, per frame.
    pub aerodynamic_friction: f32,
}

impl Default for PhysicsData {
    /// `DEFAULT_MASS` and the `DEFAULT_*_FRICTION` constants (already per frame).
    fn default() -> Self {
        Self { mass: 1.0, forward_friction: 0.15, lateral_friction: 0.15, z_friction: 0.8, aerodynamic_friction: 0.0 }
    }
}

impl Template {
    /// The `PhysicsBehavior` module's data, if the object has one.
    pub fn physics(&self) -> Option<PhysicsData> {
        let m = self.modules.iter().find(|m| m.class == "PhysicsBehavior")?;
        let mut d = PhysicsData::default();
        for f in m.block.fields() {
            let Some(v) = f.values().first().and_then(|v| ih_ini::values::scan_real(v.as_bytes()).ok()) else {
                continue;
            };
            match f.name.as_str() {
                "Mass" => d.mass = v,
                // parseFrictionPerSec
                "ForwardFriction" => d.forward_friction = v / 30.0,
                "LateralFriction" => d.lateral_friction = v / 30.0,
                "ZFriction" => d.z_friction = v / 30.0,
                "AerodynamicFriction" => d.aerodynamic_friction = v / 30.0,
                _ => {}
            }
        }
        Some(d)
    }
}
impl Locomotor {
    pub fn ground(&self) -> bool {
        self.surfaces.iter().any(|s| s == "GROUND")
    }
    /// `getLegalSurfaces` (`LocomotorSurfaceType` bits; unknown names are ignored).
    pub fn surface_mask(&self) -> u8 {
        use crate::footprint::surface::*;
        self.surfaces
            .iter()
            .map(|s| match s.to_ascii_uppercase().as_str() {
                "GROUND" => GROUND,
                "WATER" => WATER,
                "CLIFF" => CLIFF,
                "AIR" => AIR,
                "RUBBLE" => RUBBLE,
                _ => 0,
            })
            .fold(0, |a, b| a | b)
    }
    /// `isDoingGroundMovement`: any surface locomotor without `AIR` (boats and
    /// cliff climbers included).
    pub fn surface_movement(&self) -> bool {
        let m = self.surface_mask();
        m != 0 && m & crate::footprint::surface::AIR == 0
    }
}

#[derive(Clone, Default)]
pub struct Templates {
    pub objects: BTreeMap<String, Template>,
    pub locomotors: BTreeMap<String, Locomotor>,
    pub warnings: Vec<String>,
    pub bridges: Vec<crate::bridges::Bridge>,
    pub group: GroupSettings,
    /// `Armor` definitions by name.
    pub armors: BTreeMap<String, crate::body::ArmorTemplate>,
    pub damage: DamageSettings,
    /// Defined effects by (block kind, name): `FXList`, `ObjectCreationList`, `Weapon`.
    pub effects: BTreeSet<(String, String)>,
    /// `Weapon` definitions by name.
    pub weapons: BTreeMap<String, crate::weapons::WeaponTemplate>,
    /// `GameData`'s `PartitionCellSize` (the original's default is 0, used as 1).
    pub partition_cell_size: f32,
    /// `AIData` targeting and guard values.
    pub ai: crate::targeting::AiData,
    /// `GameData` production values.
    pub production: crate::production::ProductionSettings,
    /// `CommandSet` buttons by set name, in slot order.
    pub command_sets: BTreeMap<String, Vec<String>>,
    /// `CommandButton` (command, object) by name.
    pub command_buttons: BTreeMap<String, (String, String)>,
    /// `Upgrade` definitions by name.
    pub upgrades: BTreeMap<String, crate::upgrades::UpgradeTemplate>,
    /// `CommandButton`s' `Upgrade` by button name.
    pub command_button_upgrades: BTreeMap<String, String>,
    /// `GameData` building placement values.
    pub build: BuildSettings,
    /// Default-model bones of containers (`contain`), loaded with assets.
    pub bones: BTreeMap<String, BTreeMap<String, [f32; 3]>>,
    /// `GameData`'s `SellPercentage` (100%).
    pub sell_percentage: f32,
    /// `GameData`'s `BaseValuePerSupplyBox` (100).
    pub supply_box_value: u32,
    /// Dock bones of templates with supply docks, from their default models
    /// (loaded with assets by `for_map`).
    pub dock_bones: BTreeMap<String, crate::supply::DockBones>,
}

/// `GameData`'s `AllowedHeightVariationForBuilding` and `MinDistFromEdgeOfMapForBuild` (both 0).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BuildSettings {
    pub height_variation: f32,
    pub min_edge_distance: f32,
}

/// `GameData`'s damage-state thresholds, with the original defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DamageSettings {
    /// `UnitDamagedThreshold`.
    pub damaged: f32,
    /// `UnitReallyDamagedThreshold`.
    pub really_damaged: f32,
}
impl Default for DamageSettings {
    fn default() -> Self {
        Self { damaged: 0.5, really_damaged: 0.1 }
    }
}

/// Group movement settings: `AIData` (`TAiData`) and `GameData`
/// (`GroupMoveClickToGatherAreaFactor`), with the original defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroupSettings {
    pub min_infantry: i32,
    pub min_vehicles: i32,
    pub min_distance: f32,
    pub distance_requires_group: f32,
    pub gather_factor: f32,
}
impl Default for GroupSettings {
    fn default() -> Self {
        Self { min_infantry: 3, min_vehicles: 4, min_distance: 100.0, distance_requires_group: 0.0, gather_factor: 1.0 }
    }
}
/// The last value of a field across every block of a single-instance kind
/// (`AIData`, `GameData`), so later files and map overrides win.
fn global(defs: &Definitions, kind: &str, field: &str) -> Option<f32> {
    defs.all(kind, "")
        .iter()
        .rev()
        .find_map(|b| b.fields().filter(|f| f.name.eq_ignore_ascii_case(field)).last().map(|f| f.values()))
        .and_then(|v| v.first().and_then(|v| ih_ini::values::scan_real(v.as_bytes()).ok()))
}
impl Templates {
    /// Compile logic definitions and resolve map bridge dimensions without a
    /// renderer. Missing bridge assets are errors rather than invisible holes.
    pub fn for_map(
        defs: &Definitions,
        map: &ih_formats::map::Map,
        vfs: &ih_vfs::Vfs,
        language: &str,
    ) -> Result<Self, crate::SimError> {
        let mut out = Self::from_definitions(defs);
        out.bridges = crate::bridges::load(map, defs, vfs, language)?;
        out.dock_bones = crate::bridges::dock_bones(&out, vfs, language);
        out.bones = crate::bridges::logical_bones(&out, vfs, language);
        Ok(out)
    }
    pub fn from_definitions(defs: &Definitions) -> Self {
        let mut out = Self::default();
        let g = GroupSettings::default();
        out.group = GroupSettings {
            min_infantry: global(defs, "AIData", "MinInfantryForGroup").map_or(g.min_infantry, |v| v as i32),
            min_vehicles: global(defs, "AIData", "MinVehiclesForGroup").map_or(g.min_vehicles, |v| v as i32),
            min_distance: global(defs, "AIData", "MinDistanceForGroup").unwrap_or(g.min_distance),
            distance_requires_group: global(defs, "AIData", "DistanceRequiresGroup")
                .unwrap_or(g.distance_requires_group),
            gather_factor: global(defs, "GameData", "GroupMoveClickToGatherAreaFactor").unwrap_or(g.gather_factor),
        };
        out.damage = DamageSettings {
            damaged: global(defs, "GameData", "UnitDamagedThreshold").unwrap_or(0.5),
            really_damaged: global(defs, "GameData", "UnitReallyDamagedThreshold").unwrap_or(0.1),
        };
        for kind in ["FXList", "ObjectCreationList", "Weapon"] {
            out.effects.extend(defs.names(kind).map(|n| (kind.to_string(), n.clone())));
        }
        out.partition_cell_size = global(defs, "GameData", "PartitionCellSize").unwrap_or(0.0);
        let a = crate::targeting::AiData::default();
        let ms = |key: &str, d: u32| global(defs, "AIData", key).map_or(d, crate::death::duration);
        let flag = |key: &str, d: bool| {
            defs.all("AIData", "")
                .iter()
                .rev()
                .find_map(|b| b.fields().filter(|f| f.name.eq_ignore_ascii_case(key)).last().map(|f| f.values()))
                .and_then(|v| v.first().map(|s| s.eq_ignore_ascii_case("yes")))
                .unwrap_or(d)
        };
        out.ai = crate::targeting::AiData {
            force_idle_frames: ms("ForceIdleMSEC", a.force_idle_frames),
            guard_inner_human: global(defs, "AIData", "GuardInnerModifierHuman").unwrap_or(a.guard_inner_human),
            guard_outer_human: global(defs, "AIData", "GuardOuterModifierHuman").unwrap_or(a.guard_outer_human),
            guard_chase_frames: ms("GuardChaseUnitsDuration", a.guard_chase_frames),
            guard_scan: ms("GuardEnemyScanRate", a.guard_scan),
            guard_return_scan: ms("GuardEnemyReturnScanRate", a.guard_return_scan),
            attack_uses_los: flag("AttackUsesLineOfSight", a.attack_uses_los),
            attack_ignore_insignificant: flag("AttackIgnoreInsignificantBuildings", a.attack_ignore_insignificant),
        };
        out.production = crate::production::ProductionSettings {
            min_low_energy_speed: global(defs, "GameData", "MinLowEnergyProductionSpeed").unwrap_or(0.0),
            max_low_energy_speed: global(defs, "GameData", "MaxLowEnergyProductionSpeed").unwrap_or(0.0),
            low_energy_penalty: global(defs, "GameData", "LowEnergyPenaltyModifier").unwrap_or(0.0),
            multiple_factory: global(defs, "GameData", "MultipleFactory").unwrap_or(0.0),
        };
        out.build = BuildSettings {
            height_variation: global(defs, "GameData", "AllowedHeightVariationForBuilding").unwrap_or(0.0),
            min_edge_distance: global(defs, "GameData", "MinDistFromEdgeOfMapForBuild").unwrap_or(0.0),
        };
        // INI::parsePercentToReal
        out.sell_percentage = defs
            .all("GameData", "")
            .iter()
            .rev()
            .find_map(|b| {
                b.fields().filter(|f| f.name.eq_ignore_ascii_case("SellPercentage")).last().map(|f| f.values())
            })
            .and_then(|v| v.first().and_then(|s| ih_ini::values::scan_real(s.trim_end_matches('%').as_bytes()).ok()))
            .map_or(1.0, |p| p / 100.0);
        out.supply_box_value = global(defs, "GameData", "BaseValuePerSupplyBox").map_or(100, |v| v as u32);
        for name in defs.names("CommandSet") {
            // Slots 1..=MAX_COMMANDS_PER_SET; later definitions replace slots.
            let mut slots = BTreeMap::new();
            for b in defs.all("CommandSet", name) {
                for f in b.fields() {
                    if let (Ok(i), Some(v)) = (f.name.parse::<u32>(), f.values().first()) {
                        slots.insert(i, v.clone());
                    }
                }
            }
            out.command_sets.insert(name.clone(), slots.into_values().collect());
        }
        for name in defs.names("CommandButton") {
            let (mut command, mut object) = (String::new(), String::new());
            for b in defs.all("CommandButton", name) {
                for f in b.fields() {
                    match f.name.as_str() {
                        "Command" => command = f.values().first().cloned().unwrap_or_default(),
                        "Object" => object = f.values().first().cloned().unwrap_or_default(),
                        "Upgrade" => {
                            if let Some(u) = f.values().first() {
                                out.command_button_upgrades.insert(name.clone(), u.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
            out.command_buttons.insert(name.clone(), (command, object));
        }
        let default = defs.names("Upgrade").find(|n| *n == "DefaultUpgrade").map(|n| {
            crate::upgrades::UpgradeTemplate::parse(n, None, &defs.all("Upgrade", n).iter().collect::<Vec<_>>())
        });
        for name in defs.names("Upgrade") {
            let blocks: Vec<&ih_ini::Block> = defs.all("Upgrade", name).iter().collect();
            out.upgrades.insert(name.clone(), crate::upgrades::UpgradeTemplate::parse(name, default.as_ref(), &blocks));
        }
        for name in defs.names("Weapon") {
            let blocks: Vec<&ih_ini::Block> = defs.all("Weapon", name).iter().collect();
            out.weapons.insert(name.clone(), crate::weapons::WeaponTemplate::parse(name, &blocks));
        }
        for name in defs.names("Armor") {
            // A later definition replaces an earlier one (`clear` then parse).
            if let Some(b) = defs.all("Armor", name).last() {
                out.armors.insert(name.clone(), crate::body::ArmorTemplate::parse(b));
            }
        }
        for name in defs.names("Object").chain(defs.names("ObjectReskin")) {
            let mut visiting = BTreeSet::new();
            let t = resolve(defs, name, &mut visiting, &mut out.warnings);
            if let Some(t) = t {
                out.objects.insert(name.clone(), t);
            }
        }
        for name in defs.names("Locomotor") {
            let mut fields = BTreeMap::new();
            for b in defs.all("Locomotor", name) {
                for f in b.fields() {
                    fields.insert(f.name.as_str(), f.values());
                }
            }
            let number = |key: &str, default: f32| {
                fields
                    .get(key)
                    .and_then(|v| v.first())
                    .and_then(|v| ih_ini::values::scan_real(v.as_bytes()).ok())
                    .unwrap_or(default)
            };
            let flag = |key: &str| {
                fields
                    .get(key)
                    .and_then(|v| v.first())
                    .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1"))
            };
            out.locomotors.insert(
                name.clone(),
                Locomotor {
                    name: name.clone(),
                    surfaces: fields.get("Surfaces").cloned().unwrap_or_default(),
                    speed: number("Speed", 0.0) / 30.0,
                    min_speed: number("MinSpeed", 0.0) / 30.0,
                    acceleration: number("Acceleration", 0.0) / (30.0 * 30.0),
                    // Unset Braking and MinTurnSpeed stay BIGNUM, unconverted.
                    braking: if fields.contains_key("Braking") { number("Braking", 0.0) / 900.0 } else { BIGNUM },
                    turn_rate: number("TurnRate", 0.0) * (std::f32::consts::PI / 180.0) / 30.0,
                    appearance: match fields.get("Appearance").and_then(|v| v.first()).map(String::as_str) {
                        Some("TWO_LEGS") => Appearance::Legs,
                        Some("HOVER") => Appearance::Hover,
                        Some("WINGS") => Appearance::Wings,
                        Some("FOUR_WHEELS" | "MOTORCYCLE") => Appearance::Wheels,
                        Some("TREADS") => Appearance::Treads,
                        _ => Appearance::Other,
                    },
                    min_turn_speed: if fields.contains_key("MinTurnSpeed") {
                        number("MinTurnSpeed", 0.0) / 30.0
                    } else {
                        BIGNUM
                    },
                    turn_pivot_offset: number("TurnPivotOffset", 0.0),
                    extra_2d_friction: number("Extra2DFriction", 0.0) / 30.0,
                    stick_to_ground: flag("StickToGround"),
                    can_move_backward: flag("CanMoveBackwards"),
                    downhill_only: flag("DownhillOnly"),
                    wander_width_factor: number("WanderWidthFactor", 0.0),
                    wander_length_factor: number("WanderLengthFactor", 1.0),
                    lift: number("Lift", 0.0) / 900.0,
                    preferred_height: number("PreferredHeight", 0.0),
                    preferred_height_damping: number("PreferredHeightDamping", 1.0),
                    max_thrust_angle: number("MaxThrustAngle", 0.0).to_radians(),
                    speed_limit_z: if fields.contains_key("SpeedLimitZ") {
                        number("SpeedLimitZ", 0.0) / 30.0
                    } else {
                        999_999.0
                    },
                    behavior_z: match fields.get("ZAxisBehavior").and_then(|v| v.first()).map(String::as_str) {
                        Some("SEA_LEVEL") => BehaviorZ::SeaLevel,
                        Some("SURFACE_RELATIVE_HEIGHT") => BehaviorZ::SurfaceRelativeHeight,
                        Some("ABSOLUTE_HEIGHT") => BehaviorZ::AbsoluteHeight,
                        Some("FIXED_SURFACE_RELATIVE_HEIGHT") => BehaviorZ::FixedSurfaceRelativeHeight,
                        Some("FIXED_ABSOLUTE_HEIGHT") => BehaviorZ::FixedAbsoluteHeight,
                        Some("FIXED_RELATIVE_TO_GROUND_AND_BUILDINGS") => BehaviorZ::FixedRelativeToGroundAndBuildings,
                        Some("RELATIVE_TO_HIGHEST_LAYER") => BehaviorZ::RelativeToHighestLayer,
                        _ => BehaviorZ::NoZMotiveForce,
                    },
                    circling_radius: number("CirclingRadius", 0.0),
                    allow_airborne_motive_force: flag("AllowAirborneMotiveForce"),
                    apply_2d_friction_when_airborne: flag("Apply2DFrictionWhenAirborne"),
                    close_enough_dist: number("CloseEnoughDist", 1.0),
                    move_priority: match fields.get("GroupMovementPriority").and_then(|v| v.first()).map(String::as_str)
                    {
                        Some("MOVES_BACK") => 0,
                        Some("MOVES_FRONT") => 2,
                        _ => 1,
                    },
                    chassis: {
                        let rad = std::f32::consts::PI / 180.0;
                        Chassis {
                            kind: match fields.get("Appearance").and_then(|v| v.first()).map(String::as_str) {
                                Some("FOUR_WHEELS") => ChassisKind::Wheels,
                                Some("MOTORCYCLE") => ChassisKind::Motorcycle,
                                Some("TREADS") => ChassisKind::Treads,
                                Some("HOVER" | "WINGS") => ChassisKind::HoverOrWings,
                                Some("THRUST") => ChassisKind::Thrust,
                                _ => ChassisKind::None,
                            },
                            accel_pitch_limit: number("AccelerationPitchLimit", 0.0) * rad,
                            decel_pitch_limit: number("DecelerationPitchLimit", 0.0) * rad,
                            bounce_kick: number("BounceAmount", 0.0) * rad / 30.0,
                            pitch_stiffness: number("PitchStiffness", 0.1),
                            roll_stiffness: number("RollStiffness", 0.1),
                            pitch_damping: number("PitchDamping", 0.9),
                            roll_damping: number("RollDamping", 0.9),
                            thrust_roll: number("ThrustRoll", 0.0),
                            wobble_rate: number("ThrustWobbleRate", 0.0),
                            min_wobble: number("ThrustMinWobble", 0.0),
                            max_wobble: number("ThrustMaxWobble", 0.0),
                            pitch_by_z_vel: number("PitchInDirectionOfZVelFactor", 0.0),
                            forward_vel: number("ForwardVelocityPitchFactor", 0.0),
                            lateral_vel: number("LateralVelocityRollFactor", 0.0),
                            forward_accel: number("ForwardAccelerationPitchFactor", 0.0),
                            lateral_accel: number("LateralAccelerationRollFactor", 0.0),
                            uniform_axial_damping: number("UniformAxialDamping", 1.0),
                            rudder_degree: number("RudderCorrectionDegree", 0.0),
                            rudder_rate: number("RudderCorrectionRate", 0.0),
                            elevator_degree: number("ElevatorCorrectionDegree", 0.0),
                            elevator_rate: number("ElevatorCorrectionRate", 0.0),
                            has_suspension: flag("HasSuspension"),
                            max_wheel_extension: number("MaximumWheelExtension", 0.0),
                            wheel_turn_angle: number("FrontWheelTurnAngle", 0.0) * rad,
                        }
                    },
                },
            );
        }
        out
    }
}
fn resolve(
    defs: &Definitions,
    name: &str,
    visiting: &mut BTreeSet<String>,
    warnings: &mut Vec<String>,
) -> Option<Template> {
    if !visiting.insert(name.into()) {
        warnings.push(format!("object inheritance cycle at {name}"));
        return None;
    }
    let blocks = defs.all("Object", name);
    let reskin = defs.all("ObjectReskin", name);
    if blocks.is_empty() && reskin.is_empty() {
        visiting.remove(name);
        return None;
    }
    let mut t = if let Some(b) = reskin.first() {
        let base = b.args().get(1).cloned().unwrap_or_default();
        resolve(defs, &base, visiting, warnings).unwrap_or_else(|| {
            warnings.push(format!("reskin {name}: base {base} missing"));
            Template::default()
        })
    } else if name != "DefaultThingTemplate" {
        resolve(defs, "DefaultThingTemplate", visiting, warnings).unwrap_or_default()
    } else {
        Template::default()
    };
    t.name = name.into();
    for b in defs.layers(name).filter(|b| b.kind == "Object" || b.kind == "ObjectReskin") {
        if b.kind == "ObjectReskin" && b.blocks().any(|m| m.kind == "Draw") {
            t.modules.retain(|m| m.kind != "Draw");
        }
        // `setCopiedFromDefault`: this definition's first ArmorSet replaces the
        // inherited ones.
        let mut copied = (true, true);
        apply(&mut t, &b.items, &mut copied, warnings);
    }
    visiting.remove(name);
    Some(t)
}
fn apply(t: &mut Template, items: &[Item], copied: &mut (bool, bool), warnings: &mut Vec<String>) {
    for item in items {
        match item {
            Item::Field(f) if f.name == "RemoveModule" => {
                if let Some(tag) = f.values().first() {
                    remove(t, tag, warnings);
                }
            }
            Item::Field(f) if f.name == "KindOf" => {
                let values = f.values();
                let mut kinds = t.values("KindOf").to_vec();
                if !values.iter().any(|v| v.starts_with(['+', '-'])) {
                    kinds.clear();
                }
                for v in values {
                    if v == "NONE" {
                        kinds.clear();
                        continue;
                    }
                    if let Some(v) = v.strip_prefix('-') {
                        kinds.retain(|k| k != v);
                    } else {
                        let v = v.strip_prefix('+').unwrap_or(&v);
                        if !kinds.iter().any(|k| k == v) {
                            kinds.push(v.to_owned());
                        }
                    }
                }
                t.fields.insert("KindOf".into(), kinds);
            }
            Item::Field(f) if f.name == "Locomotor" => {
                let v = f.values();
                if v.first().is_some_and(|s| s == "SET_NORMAL") || !t.fields.contains_key("Locomotor") {
                    t.fields.insert(f.name.clone(), v);
                }
            }
            Item::Field(f) => {
                t.fields.insert(f.name.clone(), f.values());
            }
            Item::Block(b) => match b.kind.as_str() {
                "Behavior" | "Body" | "Draw" | "ClientUpdate" => {
                    let args = b.args();
                    let Some(class) = args.first() else { continue };
                    let Some(tag) = args.get(1) else {
                        warnings.push(format!("{}: {class} missing module tag", t.name));
                        continue;
                    };
                    // The default's body and like-kind module are replaced by explicit definitions.
                    t.modules.retain(|m| {
                        !m.tag.starts_with("ModuleTag_Default")
                            || !(m.class == *class
                                || m.kind == "Body" && b.kind == "Body"
                                || m.kind == "Draw" && b.kind == "Draw")
                    });
                    let group = |kind: &str| match kind {
                        "Body" | "Behavior" => 0,
                        "Draw" => 1,
                        _ => 2,
                    };
                    if t.modules.iter().any(|m| m.tag == *tag && group(&m.kind) == group(&b.kind)) {
                        warnings.push(format!("{}: duplicate module tag {tag}", t.name));
                        continue;
                    }
                    t.modules.push(ModuleDef {
                        class: class.clone(),
                        tag: tag.clone(),
                        kind: b.kind.clone(),
                        block: b.clone(),
                    });
                }
                "AddModule" | "InheritableModule" | "OverrideableByLikeKind" => apply(t, &b.items, copied, warnings),
                "ArmorSet" => {
                    if copied.0 {
                        copied.0 = false;
                        t.armor_sets.clear();
                    }
                    t.armor_sets.push(crate::body::ArmorSetDef::parse(b));
                }
                "WeaponSet" => {
                    // parseWeaponTemplateSet: the first in a definition replaces the inherited ones.
                    if copied.1 {
                        copied.1 = false;
                        t.weapon_sets.clear();
                    }
                    t.weapon_sets.push(crate::weapons::WeaponSetDef::parse(b));
                }
                "Prerequisites" => {
                    t.prerequisites.clear();
                    for f in b.fields() {
                        match f.name.as_str() {
                            "Object" => t.prerequisites.push(Prerequisite::Objects(f.values())),
                            "Science" => {
                                if let Some(v) = f.values().first() {
                                    t.prerequisites.push(Prerequisite::Science(v.clone()));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "ReplaceModule" => {
                    let old = b.args().first().cloned().unwrap_or_default();
                    let class = remove(t, &old, warnings);
                    if let Some(class) = class {
                        for child in b
                            .blocks()
                            .filter(|m| matches!(m.kind.as_str(), "Behavior" | "Body" | "Draw" | "ClientUpdate"))
                        {
                            let args = child.args();
                            if args.first() != Some(&class) || args.get(1) == Some(&old) {
                                warnings.push(format!("{}: invalid ReplaceModule {old}", t.name));
                            }
                        }
                    }
                    apply(t, &b.items, copied, warnings);
                }
                _ => {}
            },
        }
    }
}
fn remove(t: &mut Template, tag: &str, warnings: &mut Vec<String>) -> Option<String> {
    for kinds in [&["Behavior", "Body"][..], &["Draw"][..], &["ClientUpdate"][..]] {
        if let Some(i) = t.modules.iter().position(|m| m.tag == tag && kinds.contains(&m.kind.as_str())) {
            return Some(t.modules.remove(i).class);
        }
    }
    warnings.push(format!("{}: module tag {tag} not found", t.name));
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reskin_layers_and_module_operations_keep_order_and_tags() {
        let mut d = Definitions::default();
        d.add_document(ih_ini::parse(b"Object Base\nScale = 2\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior Physics\nEnd\nEnd\nObjectReskin Skin Base\nScale = 3\nEnd\nObject Skin\nRemoveModule = AI\nAddModule\nBehavior = LifetimeUpdate Life\nEnd\nEnd\nReplaceModule Physics\nBehavior = PhysicsBehavior NewPhysics\nEnd\nEnd\nEnd\n"));
        let t = Templates::from_definitions(&d);
        let skin = &t.objects["Skin"];
        assert_eq!(skin.number("Scale", 1.0), 3.0);
        assert_eq!(skin.modules.iter().map(|m| m.tag.as_str()).collect::<Vec<_>>(), ["Life", "NewPhysics"]);
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    }
    #[test]
    fn locomotor_units_are_per_logic_frame() {
        let mut d = Definitions::default();
        d.add_document(ih_ini::parse(
            b"Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 450\nTurnRate = 90\nEnd\n",
        ));
        let t = Templates::from_definitions(&d);
        let l = &t.locomotors["L"];
        assert_eq!(l.speed, 1.0);
        assert_eq!(l.acceleration, 1.0);
        assert_eq!(l.braking, 0.5);
        assert!(l.ground());
    }
    #[test]
    fn draw_and_behavior_tags_have_separate_namespaces_and_reskins_replace_draws() {
        let mut d = Definitions::default();
        d.add_document(ih_ini::parse(b"Object Base\nDraw = W3DModelDraw Same\nEnd\nBehavior = PhysicsBehavior Same\nEnd\nEnd\nObjectReskin Skin Base\nDraw = W3DModelDraw Same\nEnd\nEnd\nObject Skin\nRemoveModule = Same\nEnd\n"));
        let t = Templates::from_definitions(&d);
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        let skin = &t.objects["Skin"];
        assert_eq!(skin.modules.len(), 1);
        assert_eq!(skin.modules[0].kind, "Draw");
    }
    #[test]
    fn later_object_fields_override_reskin_fields_and_kind_masks_modify_the_base() {
        let mut d = Definitions::default();
        d.add_document(ih_ini::parse(b"Object Base\nScale = 2\nKindOf = VEHICLE SELECTABLE\nEnd\nObjectReskin Skin Base\nScale = 3\nEnd\nObject Skin\nScale = 4\nKindOf = +PRELOAD -SELECTABLE\nEnd\n"));
        let t = Templates::from_definitions(&d);
        let skin = &t.objects["Skin"];
        assert_eq!(skin.number("Scale", 1.0), 4.0);
        assert!(skin.has_kind("VEHICLE"));
        assert!(skin.has_kind("PRELOAD"));
        assert!(!skin.has_kind("SELECTABLE"));
    }
}
#[cfg(test)]
mod locomotor_defaults {
    use super::*;

    #[test]
    fn unset_braking_and_min_turn_speed_stay_unconverted_bignum() {
        let mut d = Definitions::default();
        d.add_document(ih_ini::parse(
            b"Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAppearance = FOUR_WHEELS\nExtra2DFriction = 30\nEnd\n",
        ));
        let l = &Templates::from_definitions(&d).locomotors["L"];
        assert_eq!(l.braking, BIGNUM);
        assert_eq!(l.min_turn_speed, BIGNUM);
        assert_eq!(l.appearance, Appearance::Wheels);
        assert_eq!(l.extra_2d_friction, 1.0);
    }
}
