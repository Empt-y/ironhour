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
    pub gatherers: u32,
}

fn field(b: &ih_ini::Block, name: &str) -> Vec<String> {
    b.fields().filter(|f| f.name == name).last().map(|f| f.values()).unwrap_or_default()
}
fn yes(b: &ih_ini::Block, name: &str) -> bool {
    field(b, name).first().is_some_and(|v| v.eq_ignore_ascii_case("yes"))
}

impl Simulation {
    pub(crate) fn supply_spawn_update(&mut self, id: u32, index: usize) -> u32 {
        if self.objects[&id].dead {
            return crate::scheduler::FOREVER;
        }
        if self.under_construction(id) {
            return 1;
        }
        let tag = self.objects[&id].modules[index].tag.clone();
        let block = self.templates.objects[&self.objects[&id].template].modules_block(&tag);
        let Some(unit) = field(&block, "SpawnTemplateName").first().cloned() else { return crate::scheduler::FOREVER };
        match self.exit_object(id, &unit) {
            Ok(_) => crate::scheduler::FOREVER,
            Err(err) => {
                self.events.push(format!("supply spawn: {err}"));
                30
            }
        }
    }
    pub(super) fn request_supply_building(&mut self, e: &mut ScriptEngine, player: u32, template: &str, minimum: i32) {
        let Some(base) = e.bases.get(&player) else { return };
        let Some(building) = self.templates.objects.get(template) else { return };
        let Some(center) = self
            .objects
            .values()
            .find(|o| o.owner == player && self.templates.objects[&o.template].has_kind("COMMANDCENTER"))
            .map(|o| o.position)
        else {
            return;
        };
        let angle = building.number("PlacementViewAngle", 0.0).to_radians();
        let mut sources: Vec<_> = self
            .objects
            .values()
            .filter(|o| {
                !o.dead
                    && o.dock.as_ref().is_some_and(|d| {
                        d.kind == crate::supply::DockKind::Warehouse
                            && d.boxes.saturating_mul(self.templates.supply_box_value as i32) >= minimum
                    })
            })
            .map(|o| o.position)
            .collect();
        sources.sort_by(|a, b| {
            ih_math::length([a[0] - center[0], a[1] - center[1]])
                .total_cmp(&ih_math::length([b[0] - center[0], b[1] - center[1]]))
        });
        for warehouse in sources {
            if base.buildings.iter().any(|b| {
                b.template == template
                    && (b.object.is_some() || b.priority)
                    && ih_math::length([b.position[0] - warehouse[0], b.position[1] - warehouse[1]]) < 350.0
            }) {
                continue;
            }
            let builder = self
                .objects
                .values()
                .find(|o| {
                    o.owner == player
                        && !o.dead
                        && o.dozer.is_some()
                        && self.can_make_unit(o.id, template) == crate::production::CanMake::Ok
                })
                .map(|o| o.id);
            let Some(builder) = builder else { return };
            let direction = ih_math::atan2(center[1] - warehouse[1], center[0] - warehouse[0]);
            for radius in [30.0, 60.0, 90.0, 120.0, 160.0, 200.0, 250.0, 300.0] {
                for step in 0..16 {
                    let a = direction + step as f32 * ih_math::TAU / 16.0;
                    let position = [warehouse[0] + radius * ih_math::cos(a), warehouse[1] + radius * ih_math::sin(a)];
                    if self.location_legal_to_build(template, position, angle, Some(builder))
                        == crate::construction::LegalBuild::Ok
                    {
                        let base = e.bases.get_mut(&player).unwrap();
                        base.buildings.push(Building {
                            template: template.to_string(),
                            position,
                            angle,
                            object: None,
                            priority: true,
                            automatic: false,
                            remaining: 1,
                        });
                        base.next_frame = self.frame;
                        return;
                    }
                }
            }
        }
    }
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
            let lo = [0.0; 2];
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
            let mut gatherers = 1;
            for data in defs.ordered("AIData") {
                data.walk(&mut |b, _| {
                    if b.kind == "SideInfo" && b.args().first() == Some(&player.side) {
                        let key = match player.ai.unwrap() {
                            Difficulty::Easy => "ResourceGatherersEasy",
                            Difficulty::Normal => "ResourceGatherersNormal",
                            Difficulty::Hard => "ResourceGatherersHard",
                        };
                        gatherers = field(b, key).first().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) + 1;
                    }
                });
            }
            e.bases.insert(player.player, Base { buildings, enabled: false, units: false, next_frame: 0, gatherers });
        }
        self.script_engine = Some(e);
    }

    fn skirmish_start_position(
        &mut self,
        around: [f32; 3],
        base_radius: f32,
        unit: &str,
        ignore: Option<u32>,
    ) -> Option<[f32; 3]> {
        let radius = self.templates.objects[unit].collision_radius();
        let start = self.rng.real(0.0, ih_math::TAU);
        let mut distance = base_radius * 0.7;
        while distance <= base_radius * 1.3 + radius * 2.0 + 50.0 {
            for step in 0..64 {
                let angle = start + step as f32 * ih_math::TAU / 64.0;
                let x = around[0] + distance * ih_math::cos(angle);
                let y = around[1] + distance * ih_math::sin(angle);
                let z = self.map.ground_height(x, y);
                if (z - around[2]).abs() > 10.0
                    || !self.grid.passable_on(0, (x / 10.0) as i32, (y / 10.0) as i32, radius)
                {
                    continue;
                }
                if self.partition.candidates([x, y], radius, radius + self.collision_radius_bound).iter().any(|id| {
                    if Some(*id) == ignore {
                        return false;
                    }
                    let o = &self.objects[id];
                    !o.dead
                        && o.body.collides
                        && o.status & crate::body::status::NO_COLLISIONS == 0
                        && (o.position[0] - x).powi(2) + (o.position[1] - y).powi(2)
                            < (radius + Self::bounding(&self.templates.objects[&o.template]).0).powi(2)
                }) {
                    continue;
                }
                return Some([x, y, z]);
            }
            distance += 5.0;
        }
        None
    }

    /// Keep starting units outside terrain/structure cells inflated by their own radius.
    pub fn clear_skirmish_start_positions(&mut self, players: &[SkirmishPlayer]) -> Result<(), crate::SimError> {
        for player in players {
            let Some(center) = self
                .objects
                .values()
                .find(|o| o.owner == player.player && self.templates.objects[&o.template].has_kind("COMMANDCENTER"))
                .cloned()
            else {
                continue;
            };
            let radius = Self::bounding(&self.templates.objects[&center.template]).1;
            let ids: Vec<_> =
                self.objects.values().filter(|o| o.owner == player.player && o.dozer.is_some()).map(|o| o.id).collect();
            for id in ids {
                let o = &self.objects[&id];
                let r = self.templates.objects[&o.template].collision_radius();
                if self.grid.passable_on(0, (o.position[0] / 10.0) as i32, (o.position[1] / 10.0) as i32, r) {
                    continue;
                }
                let around = [center.position[0], center.position[1] - radius / 2.0, center.position[2]];
                let template = self.objects[&id].template.clone();
                let at = self.skirmish_start_position(around, radius, &template, Some(id)).ok_or_else(|| {
                    crate::SimError(format!(
                        "no passable start for {id} {template}, center {:?}, base radius {radius}, unit radius {r}",
                        center.position
                    ))
                })?;
                self.forget_cells(id);
                let o = self.objects.get_mut(&id).unwrap();
                o.position = at;
                o.previous_position = at;
                self.update_pos(id);
                self.rebuild_partition_now();
            }
        }
        Ok(())
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
                    let at = self
                        .skirmish_start_position(around, radius, unit, None)
                        .ok_or_else(|| crate::SimError(format!("no legal starting position for {unit}")))?;
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

    pub(super) fn update_script_gatherers(&mut self, e: &ScriptEngine) {
        if !self.frame.is_multiple_of(30) {
            return;
        }
        for (&player, base) in &e.bases {
            let centers: Vec<_> = self
                .objects
                .values()
                .filter(|o| {
                    o.owner == player
                        && !o.dead
                        && !self.under_construction(o.id)
                        && o.dock.as_ref().is_some_and(|d| d.kind == crate::supply::DockKind::Center)
                })
                .map(|o| o.id)
                .collect();
            let live = self
                .objects
                .values()
                .filter(|o| {
                    o.owner == player
                        && !o.dead
                        && o.truck.is_some()
                        && self.templates.objects[&o.template].has_kind("HARVESTER")
                })
                .count();
            let queued = self
                .objects
                .values()
                .filter(|o| o.owner == player)
                .filter_map(|o| o.production.as_ref())
                .flat_map(|p| &p.queue)
                .filter(|entry| {
                    !entry.upgrade
                        && self.templates.objects.get(&entry.template).is_some_and(|t| t.has_kind("HARVESTER"))
                })
                .count();
            if live + queued >= centers.len() * base.gatherers as usize {
                continue;
            }
            for center in centers {
                let what = self
                    .templates
                    .command_sets
                    .get(
                        self.templates.objects[&self.objects[&center].template]
                            .values("CommandSet")
                            .first()
                            .map_or("", String::as_str),
                    )
                    .into_iter()
                    .flatten()
                    .filter_map(|b| self.templates.command_buttons.get(b))
                    .find(|(command, unit)| {
                        command == "UNIT_BUILD"
                            && self.templates.objects.get(unit).is_some_and(|t| t.has_kind("HARVESTER"))
                    })
                    .map(|(_, unit)| unit.clone());
                if let Some(what) = what
                    && self.can_make_unit(center, &what) == crate::production::CanMake::Ok
                {
                    let _ = self.queue_create_unit(center, &what);
                    break;
                }
            }
        }
    }
}
