//! Which lines open nested blocks.
//!
//! INI syntax alone cannot tell `Behavior = AIUpdateInterface Tag` (opens a block
//! that ends with `End`) from `Side = America` (a plain field). In the original
//! that knowledge lives in the field parse procedures that call
//! `INI::initFromINI`. This table lists them, by context.
//!
//! Sources (all under `GeneralsMD/Code/`):
//! - top-level blocks: `GameEngine/Source/Common/INI/INI.cpp` (`theTypeTable`)
//! - objects: `GameEngine/Source/Common/Thing/ThingTemplate.cpp`
//! - modules: `AIUpdate.cpp` (Turret), `W3DModelDraw.cpp` (ConditionState),
//!   `RadiusDecal.cpp` users (DeliveryDecal etc.)
//! - FX/OCL nuggets: `GameClient/FXList.cpp`, `GameLogic/Object/ObjectCreationList.cpp`
//! - AI: `GameLogic/AI/AI.cpp`; UI: `ControlBarScheme.cpp`, `ShellMenuScheme.cpp`,
//!   `GameWindowTransitions.cpp`, `InGameUI.cpp`, `Eva.cpp`, `CampaignManager.cpp`,
//!   `ChallengeGenerals.cpp`
//!
//! The stock and mod data scans in `ih-tools scan` are the test that this table is
//! complete: a missing opener shows up as an unbalanced `End`.

/// Parsing context of a block body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctx {
    /// Plain fields only.
    Leaf,
    Object,
    Module,
    FxList,
    Ocl,
    OclNugget,
    AiData,
    AiSideInfo,
    AiBuildList,
    Campaign,
    ChallengeGenerals,
    ControlBarScheme,
    ControlBarAnimatingPart,
    ShellMenuScheme,
    WindowTransition,
    InGameUi,
    EvaEvent,
}

/// The 62 top-level block keywords, in `theTypeTable` order. Matched case-sensitively.
pub const BLOCK_TYPES: &[&str] = &[
    "AIData",
    "Animation",
    "Armor",
    "AudioEvent",
    "AudioSettings",
    "Bridge",
    "Campaign",
    "ChallengeGenerals",
    "CommandButton",
    "CommandMap",
    "CommandSet",
    "ControlBarScheme",
    "ControlBarResizer",
    "CrateData",
    "Credits",
    "WindowTransition",
    "DamageFX",
    "DialogEvent",
    "DrawGroupInfo",
    "EvaEvent",
    "FXList",
    "GameData",
    "InGameUI",
    "Locomotor",
    "Language",
    "MapCache",
    "MapData",
    "MappedImage",
    "MiscAudio",
    "Mouse",
    "MouseCursor",
    "MultiplayerColor",
    "MultiplayerStartingMoneyChoice",
    "OnlineChatColors",
    "MultiplayerSettings",
    "MusicTrack",
    "Object",
    "ObjectCreationList",
    "ObjectReskin",
    "ParticleSystem",
    "PlayerTemplate",
    "Road",
    "Science",
    "Rank",
    "SpecialPower",
    "ShellMenuScheme",
    "Terrain",
    "Upgrade",
    "Video",
    "WaterSet",
    "WaterTransparency",
    "Weather",
    "Weapon",
    "WebpageURL",
    "HeaderTemplate",
    "StaticGameLOD",
    "DynamicGameLOD",
    "LODPreset",
    "BenchProfile",
    "ReallyLowMHz",
    "ScriptAction",
    "ScriptCondition",
];

/// Top-level blocks that are a single line with no body or `End`
/// (`GameLOD.cpp`: `parseLODPreset`, `parseBenchProfile`, `parseReallyLowMHz`).
pub const SINGLE_LINE_BLOCKS: &[&str] = &["LODPreset", "BenchProfile", "ReallyLowMHz"];

pub fn is_block_type(token: &str) -> bool {
    BLOCK_TYPES.contains(&token)
}

/// Body context of a top-level block, or `None` for single-line blocks.
pub fn top_level_ctx(kind: &str) -> Option<Ctx> {
    if SINGLE_LINE_BLOCKS.contains(&kind) {
        return None;
    }
    Some(match kind {
        "Object" | "ObjectReskin" => Ctx::Object,
        "FXList" => Ctx::FxList,
        "ObjectCreationList" => Ctx::Ocl,
        "AIData" => Ctx::AiData,
        "Campaign" => Ctx::Campaign,
        "ChallengeGenerals" => Ctx::ChallengeGenerals,
        "ControlBarScheme" => Ctx::ControlBarScheme,
        "ShellMenuScheme" => Ctx::ShellMenuScheme,
        "WindowTransition" => Ctx::WindowTransition,
        "InGameUI" => Ctx::InGameUi,
        "EvaEvent" => Ctx::EvaEvent,
        _ => Ctx::Leaf,
    })
}

/// If `field` opens a nested block inside `ctx`, the nested block's context.
pub fn nested_ctx(ctx: Ctx, field: &str) -> Option<Ctx> {
    use Ctx::*;
    match ctx {
        Leaf => None,
        Object => match field {
            "Behavior" | "Body" | "Draw" | "ClientUpdate" => Some(Module),
            // These wrap one module definition and have their own End.
            "AddModule" | "ReplaceModule" | "InheritableModule" | "OverrideableByLikeKind" => Some(Object),
            "ArmorSet" | "WeaponSet" | "Prerequisites" | "UnitSpecificSounds" | "UnitSpecificFX" => Some(Leaf),
            _ => None,
        },
        Module => match field {
            "Turret" | "AltTurret" => Some(Leaf),
            "DeliveryDecal" | "GridDecalTemplate" | "AttackAreaDecal" | "TargetingReticleDecal" => Some(Leaf),
            "ConditionState" | "DefaultConditionState" | "TransitionState" => Some(Leaf),
            _ => None,
        },
        FxList => matches!(
            field,
            "Sound"
                | "RayEffect"
                | "Tracer"
                | "LightPulse"
                | "ViewShake"
                | "TerrainScorch"
                | "ParticleSystem"
                | "FXListAtBonePos"
        )
        .then_some(Leaf),
        Ocl => matches!(
            field,
            "CreateObject" | "CreateDebris" | "ApplyRandomForce" | "DeliverPayload" | "FireWeapon" | "Attack"
        )
        .then_some(OclNugget),
        OclNugget => (field == "DeliveryDecal").then_some(Leaf),
        AiData => match field {
            "SideInfo" => Some(AiSideInfo),
            "SkirmishBuildList" => Some(AiBuildList),
            _ => None,
        },
        AiSideInfo => field.strip_prefix("SkillSet").is_some_and(is_digits).then_some(Leaf),
        AiBuildList => (field == "Structure").then_some(Leaf),
        Campaign => (field == "Mission").then_some(Leaf),
        ChallengeGenerals => field.strip_prefix("GeneralPersona").is_some_and(is_digits).then_some(Leaf),
        ControlBarScheme => match field {
            "ImagePart" => Some(Leaf),
            "AnimatingPart" => Some(ControlBarAnimatingPart),
            _ => None,
        },
        ControlBarAnimatingPart => (field == "ImagePart").then_some(Leaf),
        ShellMenuScheme => matches!(field, "ImagePart" | "LinePart").then_some(Leaf),
        WindowTransition => (field == "Window").then_some(Leaf),
        InGameUi => field.ends_with("RadiusCursor").then_some(Leaf),
        EvaEvent => (field == "SideSounds").then_some(Leaf),
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
