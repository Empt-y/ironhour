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
}
impl Template {
    pub fn values(&self, key: &str) -> &[String] {
        self.fields.get(key).map_or(&[], |v| v.as_slice())
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

#[derive(Clone, Debug)]
pub struct Locomotor {
    pub name: String,
    pub surfaces: Vec<String>,
    pub speed: f32,
    pub min_speed: f32,
    pub acceleration: f32,
    pub braking: f32,
    pub turn_rate: f32,
}
impl Locomotor {
    pub fn ground(&self) -> bool {
        self.surfaces.iter().any(|s| s == "GROUND")
    }
}

#[derive(Clone, Default)]
pub struct Templates {
    pub objects: BTreeMap<String, Template>,
    pub locomotors: BTreeMap<String, Locomotor>,
    pub warnings: Vec<String>,
    pub bridges: Vec<crate::bridges::Bridge>,
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
        Ok(out)
    }
    pub fn from_definitions(defs: &Definitions) -> Self {
        let mut out = Self::default();
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
            out.locomotors.insert(
                name.clone(),
                Locomotor {
                    name: name.clone(),
                    surfaces: fields.get("Surfaces").cloned().unwrap_or_default(),
                    speed: number("Speed", 0.0) / 30.0,
                    min_speed: number("MinSpeed", 0.0) / 30.0,
                    acceleration: number("Acceleration", 0.0) / (30.0 * 30.0),
                    braking: number("Braking", 99999.0) / (30.0 * 30.0),
                    turn_rate: number("TurnRate", 0.0) * (std::f32::consts::PI / 180.0) / 30.0,
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
        apply(&mut t, &b.items, warnings);
    }
    visiting.remove(name);
    Some(t)
}
fn apply(t: &mut Template, items: &[Item], warnings: &mut Vec<String>) {
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
                "AddModule" | "InheritableModule" | "OverrideableByLikeKind" => apply(t, &b.items, warnings),
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
                    apply(t, &b.items, warnings);
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
