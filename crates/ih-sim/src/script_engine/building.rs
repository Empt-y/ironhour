//! Faction build plans from AI::parseSkirmishBuildList and AISkirmishPlayer.
use super::*;

#[derive(Clone, Debug)]
pub struct Building {
    pub template: String,
    pub position: [f32; 2],
    pub angle: f32,
    pub object: Option<u32>,
    pub priority: bool,
    pub automatic: bool,
    pub remaining: i32,
}
#[derive(Clone, Debug, Default)]
pub struct Base {
    pub buildings: Vec<Building>,
    pub enabled: bool,
    pub units: bool,
    pub next_frame: u32,
}

fn field(b: &ih_ini::Block, name: &str) -> Vec<String> {
    b.fields().filter(|f| f.name == name).last().map(|f| f.values()).unwrap_or_default()
}
fn yes(b: &ih_ini::Block, name: &str) -> bool {
    field(b, name).first().is_some_and(|v| v.eq_ignore_ascii_case("yes"))
}

impl Simulation {
    /// Translate the faction build list about its command center into this start's base.
    pub fn configure_script_bases(&mut self, defs: &ih_ini::definitions::Definitions) {
        let Some(mut e) = self.script_engine.take() else { return };
        for player in e.players.iter().filter(|p| p.ai.is_some()) {
            let center = self
                .objects
                .values()
                .find(|o| o.owner == player.player && self.templates.objects[&o.template].has_kind("COMMANDCENTER"));
            let Some(center) = center else { continue };
            let mut plan = None;
            for data in defs.ordered("AIData") {
                data.walk(&mut |b, _| {
                    if b.kind == "SkirmishBuildList" && b.args().first() == Some(&player.side) {
                        plan = Some(b.clone());
                    }
                });
            }
            let Some(plan) = plan else { continue };
            let mut buildings = vec![];
            plan.walk(&mut |b, _| {
                if b.kind != "Structure" {
                    return;
                }
                let Some(template) = b.args().first().cloned() else { return };
                let loc = field(b, "Location");
                let xy = |key: &str| loc.iter().find_map(|v| v.strip_prefix(key)?.parse::<f32>().ok()).unwrap_or(0.0);
                buildings.push(Building {
                    template,
                    position: [xy("X:"), xy("Y:")],
                    angle: field(b, "Angle").first().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0).to_radians(),
                    object: None,
                    priority: yes(b, "InitiallyBuilt"),
                    automatic: yes(b, "AutomaticallyBuild"),
                    remaining: field(b, "Rebuilds")
                        .first()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0i32)
                        .saturating_add(1),
                });
            });
            let Some(anchor) = buildings
                .iter()
                .position(|b| self.templates.objects.get(&b.template).is_some_and(|t| t.has_kind("COMMANDCENTER")))
            else {
                continue;
            };
            let origin = buildings[anchor].position;
            let lo = [self.map.border as f32 * 10.0; 2];
            let size = self
                .map
                .boundaries
                .first()
                .copied()
                .unwrap_or([self.map.width - 2 * self.map.border, self.map.height - 2 * self.map.border]);
            let hi = [lo[0] + size[0] as f32 * 10.0, lo[1] + size[1] as f32 * 10.0];
            let x = ((center.position[0] - lo[0]) / ((hi[0] - lo[0]) / 3.0)).clamp(0.0, 2.0) as usize;
            let y = ((center.position[1] - lo[1]) / ((hi[1] - lo[1]) / 3.0)).clamp(0.0, 2.0) as usize;
            let rotate = defs
                .ordered("AIData")
                .filter_map(|b| field(b, "RotateSkirmishBases").first().cloned())
                .last()
                .is_some_and(|v| v.eq_ignore_ascii_case("yes"));
            let angles = [0.0, 0.25, 0.5, -0.25, 0.0, 0.75, -0.5, -0.75, 1.0];
            let angle = (if rotate { angles[y * 3 + x] } else { 0.0 } + 0.75) * std::f32::consts::PI;
            let (s, c) = (ih_math::sin(angle), ih_math::cos(angle));
            for b in &mut buildings {
                let d = [b.position[0] - origin[0], b.position[1] - origin[1]];
                b.position = [center.position[0] + d[0] * c - d[1] * s, center.position[1] + d[1] * c + d[0] * s];
            }
            buildings[anchor].object = Some(center.id);
            buildings[anchor].remaining -= 1;
            e.bases.insert(player.player, Base { buildings, enabled: false, units: false, next_frame: 0 });
        }
        self.script_engine = Some(e);
    }

    pub fn spawn_skirmish_armies(
        &mut self,
        defs: &ih_ini::definitions::Definitions,
        players: &[SkirmishPlayer],
        money: u32,
    ) -> Result<(), crate::SimError> {
        for player in players {
            let side = defs.ordered("PlayerTemplate").find(|b| field(b, "Side").first() == Some(&player.side));
            let Some(side) = side else { return Err(crate::SimError(format!("missing faction {}", player.side))) };
            let waypoint = format!("Player_{}_Start", player.start + 1);
            let start = self
                .map
                .objects
                .iter()
                .find(|o| o.properties.ascii("waypointName").is_some_and(|n| n.eq_ignore_ascii_case(&waypoint)))
                .map(|o| o.position)
                .ok_or_else(|| crate::SimError(format!("missing {waypoint}")))?;
            let mut radius = 100.0;
            if let Some(building) = field(side, "StartingBuilding").first() {
                self.spawn(building, [start[0], start[1]], player.player)?;
                radius = Self::bounding(&self.templates.objects[building]).1;
            }
            let mut around = start;
            around[1] -= radius / 2.0;
            if let Some(rally) = self.map.objects.iter().find(|o| {
                o.properties
                    .ascii("waypointName")
                    .is_some_and(|n| n.eq_ignore_ascii_case(&format!("Player_{}_Rally", player.start + 1)))
            }) {
                around = rally.position;
            }
            around[2] = self.map.ground_height(around[0], around[1]);
            for i in 0..10 {
                if let Some(unit) =
                    field(side, &format!("StartingUnit{i}")).first().filter(|v| !v.eq_ignore_ascii_case("none"))
                {
                    let unit_radius = self.templates.objects[unit].collision_radius();
                    let mut position = None;
                    for _ in 0..32 {
                        if let Some(at) =
                            self.find_position_around(around, radius * 0.7, radius * 1.3, None, None, 10.0)
                            && self.grid.passable_on(0, (at[0] / 10.0) as i32, (at[1] / 10.0) as i32, unit_radius)
                        {
                            position = Some(at);
                            break;
                        }
                    }
                    let at =
                        position.ok_or_else(|| crate::SimError(format!("no legal starting position for {unit}")))?;
                    self.spawn(unit, [at[0], at[1]], player.player)?;
                }
            }
            self.player(player.player).deposit(money);
            for science in field(side, "IntrinsicSciences").iter().filter(|s| !s.eq_ignore_ascii_case("none")) {
                self.player(player.player).sciences.insert(science.clone());
            }
        }
        Ok(())
    }

    pub(super) fn update_script_buildings(&mut self, e: &mut ScriptEngine) {
        for (&player, base) in &mut e.bases {
            if !base.enabled || self.frame < base.next_frame {
                continue;
            }
            base.next_frame = self.frame + 60;
            for b in &mut base.buildings {
                if b.object.is_some_and(|id| self.objects.get(&id).is_none_or(|o| o.dead)) {
                    b.object = None;
                }
                if b.object.is_some() || (!b.priority && !b.automatic) || b.remaining == 0 {
                    continue;
                }
                let builder = self
                    .objects
                    .values()
                    .find(|o| {
                        o.owner == player
                            && !o.dead
                            && !o.disabled.any()
                            && o.dozer.as_ref().is_some_and(|d| d.primary == crate::construction::Primary::Idle)
                            && self.can_make_unit(o.id, &b.template) == crate::production::CanMake::Ok
                    })
                    .map(|o| o.id);
                let Some(builder) = builder else { continue };
                if self.location_legal_to_build(&b.template, b.position, b.angle, Some(builder))
                    != crate::construction::LegalBuild::Ok
                {
                    continue;
                }
                match self.build_object_now(builder, &b.template, b.position, b.angle) {
                    Ok(id) => {
                        b.object = Some(id);
                        b.priority = false;
                        b.remaining -= 1;
                        break;
                    }
                    Err(err) => self.events.push(format!("AI player {player}: {err}")),
                }
            }
        }
    }
}
