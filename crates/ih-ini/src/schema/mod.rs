//! The field schema of every INI block, extracted from the original source.
//!
//! [`generated`] is produced by `ih-schemagen` from the C++ `FieldParse` tables. This
//! module adds the hand-written parts: which table each top-level block uses, and the
//! few nested blocks whose table cannot be found from the parser name alone.
//!
//! Lookup follows `INI::initFromINIMulti`: tables are tried in order, and within a
//! table the first exact (case-sensitive) name wins; a table's catch-all entry
//! (NULL name) matches anything not matched earlier in that table.

mod generated;

use std::collections::HashMap;
use std::sync::LazyLock;

pub use generated::{LOOKUP_LISTS, MODULES, NAME_LISTS, NESTED, TABLES};

#[derive(Debug)]
pub struct FieldDef {
    /// `None` for a catch-all entry.
    pub name: Option<&'static str>,
    /// C++ parse procedure, e.g. `INI::parseReal`.
    pub parser: &'static str,
    /// C++ user data expression, e.g. a name list.
    pub user: &'static str,
}

#[derive(Debug)]
pub struct TableDef {
    pub id: &'static str,
    pub file: &'static str,
    pub fields: &'static [FieldDef],
}

#[derive(Debug)]
pub struct ModuleDef {
    pub name: &'static str,
    /// `Engine` (ModuleFactory.cpp) or `W3D` (W3DModuleFactory.cpp).
    pub origin: &'static str,
    pub data_class: &'static str,
    pub tables: &'static [&'static str],
}

/// Field tables for each top-level block keyword. Sources: the block parse procedures
/// listed in `INI.cpp`'s `theTypeTable`.
const TOP_LEVEL: &[(&str, &[&str])] = &[
    ("AIData", &["TheAIFieldParseTable"]),
    ("Animation", &["Anim2DTemplate::s_anim2DFieldParseTable"]),
    ("Armor", &["ArmorStore::parseArmorDefinition::myFieldParse"]),
    ("AudioEvent", &["AudioEventInfo::m_audioEventInfo"]),
    ("AudioSettings", &["audioSettingsFieldParseTable"]),
    ("Bridge", &["TerrainRoadType::m_terrainBridgeFieldParseTable"]),
    ("Campaign", &["CampaignManager::m_campaignFieldParseTable"]),
    ("ChallengeGenerals", &["ChallengeGenerals::s_fieldParseTable"]),
    ("CommandButton", &["CommandButton::s_commandButtonFieldParseTable"]),
    ("CommandMap", &["TheMetaMapFieldParseTable"]),
    ("CommandSet", &["CommandSet::m_commandSetFieldParseTable"]),
    ("ControlBarScheme", &["ControlBarSchemeManager::m_controlBarSchemeFieldParseTable"]),
    ("ControlBarResizer", &["ControlBarResizer::m_controlBarResizerParseTable"]),
    ("CrateData", &["CrateTemplate::TheCrateTemplateFieldParseTable"]),
    ("Credits", &["CreditsManager::m_creditsFieldParseTable"]),
    ("WindowTransition", &["GameWindowTransitionsHandler::m_gameWindowTransitionsFieldParseTable"]),
    ("DamageFX", &["DamageFX::getFieldParse::myFieldParse"]),
    ("DialogEvent", &["AudioEventInfo::m_audioEventInfo"]),
    ("DrawGroupInfo", &["DrawGroupInfo::s_fieldParseTable"]),
    ("EvaEvent", &["EvaCheckInfo::s_evaEventInfo"]),
    ("FXList", &["TheFXListFieldParse"]),
    ("GameData", &["GlobalData::s_GlobalDataFieldParseTable"]),
    ("InGameUI", &["InGameUI::s_fieldParseTable"]),
    ("Locomotor", &["LocomotorTemplate::getFieldParse::TheFieldParse"]),
    ("Language", &["TheGlobalLanguageDataFieldParseTable"]),
    ("MapCache", &["MapMetaDataReader::m_mapFieldParseTable"]),
    ("MapData", &[]),
    ("MappedImage", &["Image::m_imageFieldParseTable"]),
    ("MiscAudio", &["MiscAudio::m_fieldParseTable"]),
    ("Mouse", &["TheMouseFieldParseTable"]),
    ("MouseCursor", &["TheMouseCursorFieldParseTable"]),
    ("MultiplayerColor", &["MultiplayerColorDefinition::m_colorFieldParseTable"]),
    ("MultiplayerStartingMoneyChoice", &["startingMoneyFieldParseTable"]),
    ("OnlineChatColors", &["GameSpyColorFieldParse"]),
    ("MultiplayerSettings", &["MultiplayerSettings::m_multiplayerSettingsFieldParseTable"]),
    ("MusicTrack", &["AudioEventInfo::m_audioEventInfo"]),
    ("Object", &["ThingTemplate::s_objectFieldParseTable"]),
    ("ObjectCreationList", &["TheObjectCreationListFieldParse"]),
    ("ObjectReskin", &["ThingTemplate::s_objectReskinFieldParseTable"]),
    ("ParticleSystem", &["ParticleSystemTemplate::m_fieldParseTable"]),
    ("PlayerTemplate", &["PlayerTemplate::getFieldParse::TheFieldParseTable"]),
    ("Road", &["TerrainRoadType::m_terrainRoadFieldParseTable"]),
    ("Science", &["ScienceStore::friend_parseScienceDefinition::myFieldParse"]),
    ("Rank", &["RankInfoStore::friend_parseRankDefinition::myFieldParse"]),
    ("SpecialPower", &["SpecialPowerTemplate::m_specialPowerFieldParse"]),
    ("ShellMenuScheme", &["ShellMenuSchemeManager::m_shellMenuSchemeFieldParseTable"]),
    ("Terrain", &["TerrainType::m_terrainTypeFieldParseTable"]),
    ("Upgrade", &["UpgradeTemplate::m_upgradeFieldParseTable"]),
    ("Video", &["VideoPlayer::m_videoFieldParseTable"]),
    ("WaterSet", &["WaterSetting::m_waterSettingFieldParseTable"]),
    ("WaterTransparency", &["WaterTransparencySetting::m_waterTransparencySettingFieldParseTable"]),
    ("Weather", &["WeatherSetting::m_weatherSettingFieldParseTable"]),
    ("Weapon", &["WeaponTemplate::TheWeaponTemplateFieldParseTable"]),
    ("WebpageURL", &["WebBrowserURL::m_URLFieldParseTable"]),
    ("HeaderTemplate", &["HeaderTemplateManager::m_headerFieldParseTable"]),
    ("StaticGameLOD", &["TheStaticGameLODFieldParseTable"]),
    ("DynamicGameLOD", &["TheDynamicGameLODFieldParseTable"]),
    ("ScriptAction", &["TheTemplateFieldParseTable"]),
    ("ScriptCondition", &["TheTemplateFieldParseTable"]),
];

/// Nested blocks whose tables are not found from the parser name.
const NESTED_OVERRIDES: &[(&str, &[&str])] = &[
    // These wrap a module definition; the body is parsed with the object table.
    ("ThingTemplate::parseAddModule", &["ThingTemplate::s_objectFieldParseTable"]),
    ("ThingTemplate::parseReplaceModule", &["ThingTemplate::s_objectFieldParseTable"]),
    ("ThingTemplate::parseInheritableModule", &["ThingTemplate::s_objectFieldParseTable"]),
    ("ThingTemplate::OverrideableByLikeKind", &["ThingTemplate::s_objectFieldParseTable"]),
    ("ThingTemplate::parseArmorTemplateSet", &["ArmorTemplateSet::parseArmorTemplateSet::myFieldParse"]),
    ("ThingTemplate::parseWeaponTemplateSet", &["WeaponTemplateSet::parseWeaponTemplateSet::myFieldParse"]),
    ("AIUpdateModuleData::parseTurret", &["TurretAIData::buildFieldParse::dataFieldParse"]),
    ("parseSideSoundsList", &["EvaSideSounds::s_evaSideSounds"]),
];

/// Parser that turns a field into a module block (`Behavior = <Class> <Tag>`).
pub const MODULE_PARSER: &str = "ThingTemplate::parseModuleName";

/// How a field definition affects parsing.
#[derive(Debug, Clone, Copy)]
pub enum Shape {
    /// A one-line field.
    Field,
    /// Opens a nested block parsed with these tables.
    Block(&'static [&'static str]),
    /// Opens a module block; tables come from the module class named on the line.
    Module,
}

pub struct Schema {
    tables: HashMap<&'static str, &'static TableDef>,
    modules: HashMap<&'static str, &'static ModuleDef>,
    nested: HashMap<&'static str, &'static [&'static str]>,
    top: HashMap<&'static str, &'static [&'static str]>,
    name_lists: HashMap<&'static str, &'static [&'static str]>,
    lookup_lists: HashMap<&'static str, &'static [(&'static str, &'static str)]>,
}

static SCHEMA: LazyLock<Schema> = LazyLock::new(|| {
    let mut nested: HashMap<_, _> = NESTED.iter().copied().collect();
    nested.extend(NESTED_OVERRIDES.iter().copied());
    Schema {
        tables: TABLES.iter().map(|t| (t.id, t)).collect(),
        modules: MODULES.iter().map(|m| (m.name, m)).collect(),
        nested,
        top: TOP_LEVEL.iter().copied().collect(),
        name_lists: NAME_LISTS.iter().copied().collect(),
        lookup_lists: LOOKUP_LISTS.iter().copied().collect(),
    }
});

pub fn schema() -> &'static Schema {
    &SCHEMA
}

impl Schema {
    pub fn table(&self, id: &str) -> Option<&'static TableDef> {
        self.tables.get(id).copied()
    }

    pub fn module(&self, class: &str) -> Option<&'static ModuleDef> {
        self.modules.get(class).copied()
    }

    pub fn top_level(&self, kind: &str) -> Option<&'static [&'static str]> {
        self.top.get(kind).copied()
    }

    /// Tables for the nested block opened by a field with this parser, if it opens one.
    pub fn nested(&self, parser: &str) -> Option<&'static [&'static str]> {
        self.nested.get(parser).copied()
    }

    /// What a field definition does to the parse structure.
    pub fn shape(&self, def: &FieldDef) -> Shape {
        if def.parser == MODULE_PARSER {
            return Shape::Module;
        }
        // `AliasConditionState` shares parseConditionState but with PARSE_ALIAS it reads
        // a single line (W3DModelDraw.cpp).
        if def.user.contains("PARSE_ALIAS") {
            return Shape::Field;
        }
        match self.nested(def.parser) {
            Some(t) => Shape::Block(t),
            None => Shape::Field,
        }
    }

    pub fn name_list(&self, id: &str) -> Option<&'static [&'static str]> {
        let id = id.trim_start_matches('&');
        self.name_lists.get(id).or_else(|| self.name_lists.get(id.rsplit("::").next()?)).copied()
    }

    pub fn lookup_list(&self, id: &str) -> Option<&'static [(&'static str, &'static str)]> {
        let id = id.trim_start_matches('&');
        self.lookup_lists.get(id).or_else(|| self.lookup_lists.get(id.rsplit("::").next()?)).copied()
    }

    /// Finds the definition `field` resolves to, as `initFromINIMulti` would.
    pub fn find_field(&self, tables: &[&str], field: &str) -> Option<&'static FieldDef> {
        for id in tables {
            let Some(t) = self.table(id) else { continue };
            if let Some(f) = t.fields.iter().find(|f| f.name == Some(field)) {
                return Some(f);
            }
            if let Some(f) = t.fields.iter().find(|f| f.name.is_none()) {
                return Some(f);
            }
        }
        None
    }
}
