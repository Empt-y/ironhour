//! Transports: getting in, riding, getting out, evacuating, and what happens to
//! passengers when the transport dies.
//!
//! Sources: `GameLogic/Object/Contain/OpenContain.cpp` (`addToContain`,
//! `removeFromContainViaIterator`, `addOrRemoveObjFromWorld`, `onCollide`,
//! `isValidContainerFor`, `exitObjectViaDoor`, `scatterToNearbyPosition`,
//! `orderAllPassengersToExit`, `processDamageToContained`, `onDie`, `onDelete`,
//! `update`), `TransportContain.cpp` (`isValidContainerFor`, `onContaining`,
//! `onRemoving`, `createPayload`, `update`, `killRidersWhoAreNotFreeToExit`,
//! `isSpecificRiderFreeToExit`, `reserveDoorForExit`, `isExitBusy`,
//! `letRidersUpgradeWeaponSet`), `AIStates.cpp` (`AIEnterState`, `AIExitState`),
//! `AIUpdate.cpp` (`privateEnter`, `privateExit`, `privateEvacuate`),
//! `ActionManager::canEnterObject`, and `Object::onContainedBy`/`onRemovedFrom`.
//!
//! Not ported yet: garrisons, tunnels, Overlord/Helix-style containers,
//! parachutes, heal containers, passengers firing from inside, stealth
//! garrisons, booby traps, exiting instantly, enemies kicking riders out of
//! a shared container, moving targets while entering (the goal is where the
//! transport was when the order started), `KeepContainerVelocityOnExit`,
//! falling riders, and the aircraft/airfield entry rules.
use crate::{
    SimError, Simulation,
    ai::{AiState, Outcome},
    disabled::DisabledReason,
    terrain::Waypoint,
    weapons::Relationship,
};

/// `OpenContainModuleData` and `TransportContainModuleData`.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainData {
    pub class: String,
    /// `ContainMax`, or `Slots` for transports.
    pub capacity: i32,
    pub damage_percent: f32,
    pub burned_death: bool,
    /// `AllowInsideKindOf`; `None` allows everything.
    pub allow_inside: Option<Vec<String>>,
    pub forbid_inside: Vec<String>,
    pub exit_paths: i32,
    pub door_open_time: u32,
    pub allow_allies: bool,
    pub allow_enemies: bool,
    pub allow_neutral: bool,
    pub exit_bone: String,
    pub orient_like_container: bool,
    pub reset_mood_on_exit: bool,
    pub destroy_not_free: bool,
    pub payload: Option<(String, u32)>,
    pub health_regen: f32,
    pub exit_delay: u32,
    pub delay_exit_in_air: bool,
    pub armed_riders_upgrade: bool,
}
impl ContainData {
    pub fn from_module(class: &str, b: &ih_ini::Block) -> Option<Self> {
        if class != "TransportContain" {
            return None;
        }
        let mut d = Self {
            class: class.into(),
            capacity: 0,
            damage_percent: 0.0,
            burned_death: true,
            allow_inside: Some(vec!["INFANTRY".into()]),
            forbid_inside: vec![],
            exit_paths: 1,
            door_open_time: 1,
            allow_allies: true,
            allow_enemies: true,
            allow_neutral: true,
            exit_bone: String::new(),
            orient_like_container: false,
            reset_mood_on_exit: true,
            destroy_not_free: false,
            payload: None,
            health_regen: 0.0,
            exit_delay: 0,
            delay_exit_in_air: false,
            armed_riders_upgrade: false,
        };
        for f in b.fields() {
            let v = f.values();
            let first = v.first().cloned().unwrap_or_default();
            let yes = first.eq_ignore_ascii_case("yes");
            let num = || ih_ini::values::scan_real(first.trim_end_matches('%').as_bytes()).unwrap_or(0.0);
            match f.name.as_str() {
                "Slots" => d.capacity = num() as i32,
                "DamagePercentToUnits" => d.damage_percent = num() / 100.0,
                "BurnedDeathToUnits" => d.burned_death = yes,
                "AllowInsideKindOf" => d.allow_inside = Some(v),
                "ForbidInsideKindOf" => d.forbid_inside = v,
                "NumberOfExitPaths" => d.exit_paths = num() as i32,
                "DoorOpenTime" => d.door_open_time = crate::death::duration(num()),
                "AllowAlliesInside" => d.allow_allies = yes,
                "AllowEnemiesInside" => d.allow_enemies = yes,
                "AllowNeutralInside" => d.allow_neutral = yes,
                "ExitBone" => d.exit_bone = first,
                "OrientLikeContainerOnExit" => d.orient_like_container = yes,
                "ResetMoodCheckTimeOnExit" => d.reset_mood_on_exit = yes,
                "DestroyRidersWhoAreNotFreeToExit" => d.destroy_not_free = yes,
                "InitialPayload" => {
                    let n = v.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
                    d.payload = Some((first, n));
                }
                "HealthRegen%PerSec" => d.health_regen = num(),
                "ExitDelay" => d.exit_delay = crate::death::duration(num()),
                "DelayExitInAir" => d.delay_exit_in_air = yes,
                "ArmedRidersUpgradeMyWeaponSet" => d.armed_riders_upgrade = yes,
                _ => {}
            }
        }
        Some(d)
    }
}

/// A container's state.
#[derive(Clone, Debug, PartialEq)]
pub struct Contain {
    pub data: ContainData,
    /// `m_containList`, in entry order.
    pub riders: Vec<u32>,
    pub extra_slots: i32,
    /// `m_frameExitNotBusy`.
    pub exit_not_busy: u32,
    pub door_countdown: u32,
    /// `m_whichExitPath` (1 to `NumberOfExitPaths`).
    pub which_exit: i32,
    pub payload_created: bool,
    pub rally: Option<[f32; 2]>,
}
impl Contain {
    pub fn new(data: ContainData) -> Self {
        Self {
            data,
            riders: vec![],
            extra_slots: 0,
            exit_not_busy: 0,
            door_countdown: 0,
            which_exit: 1,
            payload_created: false,
            rally: None,
        }
    }
}

impl Simulation {
    fn contain(&self, id: u32) -> Option<&Contain> {
        self.objects.get(&id).and_then(|o| o.contain.as_ref())
    }
    fn contain_mut(&mut self, id: u32) -> &mut Contain {
        self.objects.get_mut(&id).unwrap().contain.as_mut().unwrap()
    }
    /// `getTransportSlotCount`.
    fn slots(&self, id: u32) -> i32 {
        self.templates.objects[&self.objects[&id].template].number("TransportSlotCount", 0.0) as u8 as i32
    }
    /// Whether an object is out of the world inside a container (`OBJECT_STATUS_MASKED`).
    pub fn contained(&self, id: u32) -> bool {
        self.objects.get(&id).is_some_and(|o| o.contained_by.is_some())
    }

    /// `TransportContain::isValidContainerFor` with `OpenContain`'s checks.
    fn valid_container_for(&self, container: u32, rider: u32, check_capacity: bool) -> bool {
        let Some(c) = self.contain(container) else { return false };
        let t = &self.templates.objects[&self.objects[&rider].template];
        if c.data.allow_inside.as_ref().is_some_and(|k| !k.iter().any(|k| t.has_kind(k)))
            || c.data.forbid_inside.iter().any(|k| t.has_kind(k))
        {
            return false;
        }
        let allowed = match self.relationship(rider, container) {
            Relationship::Allies => c.data.allow_allies,
            Relationship::Enemies => c.data.allow_enemies,
            Relationship::Neutral => c.data.allow_neutral,
        };
        if !allowed || self.objects[&rider].owner != self.objects[&container].owner {
            return false;
        }
        let slots = self.slots(rider);
        if slots == 0 {
            return false;
        }
        !check_capacity || c.extra_slots + c.riders.len() as i32 + slots <= c.data.capacity
    }

    /// `ActionManager::canEnterObject` (no shroud, airfields or collide modules).
    pub(crate) fn can_enter_object(&self, obj: u32, target: u32, check_capacity: bool) -> bool {
        let (Some(o), Some(t)) = (self.objects.get(&obj), self.objects.get(&target)) else { return false };
        if obj == target || t.dead {
            return false;
        }
        let sold = crate::construction::status_bits(&["SOLD"]);
        if self.under_construction(obj) || self.under_construction(target) || t.status & sold != 0 {
            return false;
        }
        let (ot, tt) = (&self.templates.objects[&o.template], &self.templates.objects[&t.template]);
        if ot.has_kind("IGNORED_IN_GUI") || ot.has_kind("MOB_NEXUS") || tt.has_kind("IGNORED_IN_GUI") {
            return false;
        }
        if t.disabled.has(DisabledReason::Subdued) || ot.has_kind("STRUCTURE") || ot.has_kind("IMMOBILE") {
            return false;
        }
        let Some(c) = t.contain.as_ref() else { return false };
        // With stealth garrisons (not ported) a foreign container could skip the capacity check.
        let check = check_capacity;
        if t.owner != o.owner {
            if !c.riders.is_empty() {
                return false;
            }
            if tt.has_kind("FS_FACTORY") || tt.has_kind("FS_BASE_DEFENSE") {
                return false;
            }
        }
        if check && self.slots(obj) == 0 {
            return false;
        }
        self.valid_container_for(target, obj, check)
    }

    /// `privateEnter` (`MSG_ENTER`).
    pub(crate) fn begin_enter(&mut self, id: u32, target: u32, from_ai: bool) -> Result<(), SimError> {
        self.validate_mobile_quiet(id)?;
        if !self.can_enter_object(id, target, false) {
            return Err(SimError(format!("object {id} cannot enter {target}")));
        }
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.begin(AiState::Enter, frame);
        o.ai.cmd_from_ai = from_ai;
        o.ai.enter = Some(target);
        // AIEnterState::onEnter
        if !self.can_enter_object(id, target, true) {
            self.end_order(id, Outcome::Failure);
            return Ok(());
        }
        let goal = self.objects[&target].position;
        self.objects.get_mut(&id).unwrap().ai.ignore_obstacle = Some(target);
        // setAdjustsDestination(false)
        let wp = self.choose_goal(id, [goal[0], goal[1]], AiState::FollowPath, false);
        match self.route_to_waypoint(id, wp, true) {
            Ok((path, to)) => {
                let o = self.objects.get_mut(&id).unwrap();
                o.path = path;
                o.path.push_front(Waypoint { position: [o.position[0], o.position[1]], layer: o.layer });
                o.target = Some(to);
                o.last_repath = self.frame;
                self.wake_ai(id);
            }
            Err(_) => self.end_order(id, Outcome::Failure),
        }
        Ok(())
    }

    /// `AIEnterState::update`.
    pub(crate) fn update_enter(&mut self, id: u32) -> u32 {
        let Some(goal) = self.objects[&id].ai.enter.filter(|g| self.objects.contains_key(g)) else {
            self.end_order(id, Outcome::Failure);
            return 1;
        };
        if !self.can_enter_object(id, goal, true) {
            // Enemies taking it first are attacked instead.
            if self.relationship(id, goal) == Relationship::Enemies
                && self.begin_attack(id, Some(goal), self.objects[&goal].position).is_ok()
            {
                return 1;
            }
            self.end_order(id, Outcome::Failure);
            return 1;
        }
        if self.objects[&id].disabled.has(DisabledReason::Held) {
            self.end_order(id, Outcome::Success);
            return 1;
        }
        self.repath_traffic(id);
        self.drive(id);
        if self.objects[&id].target.is_some() {
            return 1;
        }
        // Arrived: inside the footprint is close enough to get in.
        let (o, g) = (&self.objects[&id], &self.objects[&goal]);
        let gt = &self.templates.objects[&g.template];
        let boxed = gt.values("Geometry").first().is_some_and(|v| v.eq_ignore_ascii_case("BOX"));
        let radius =
            gt.number(if boxed { "GeometryMinorRadius" } else { "GeometryMajorRadius" }, 0.0) * gt.number("Scale", 1.0);
        let d2 = (o.position[0] - g.position[0]).powi(2) + (o.position[1] - g.position[1]).powi(2);
        if d2 < radius * radius {
            self.add_to_contain(goal, id);
        }
        if self.objects[&id].ai.state == AiState::Enter {
            self.end_order(id, Outcome::Success);
        }
        1
    }

    /// `OpenContain::onCollide`: a unit trying to enter that touches the container gets in.
    pub(crate) fn contain_collide(&mut self, container: u32, other: u32) {
        let o = &self.objects[&other];
        if o.ai.state != AiState::Enter || o.ai.enter != Some(container) || self.contain(container).is_none() {
            return;
        }
        if self.valid_container_for(container, other, true) {
            self.add_to_contain(container, other);
            if self.objects[&other].ai.state == AiState::Enter {
                self.end_order(other, Outcome::Success);
            }
        }
    }

    /// `OpenContain::addToContain` and `TransportContain::onContaining`.
    pub(crate) fn add_to_contain(&mut self, container: u32, rider: u32) {
        if self.objects[&rider].contained_by.is_some() {
            return;
        }
        let slots = self.slots(rider);
        let c = self.contain_mut(container);
        c.riders.push(rider);
        c.extra_slots += slots - 1;
        // addOrRemoveObjFromWorld(false): out of the partition and the pathfinder.
        self.forget_cells(rider);
        let frame = self.frame;
        let bits = crate::construction::status_bits(&["UNSELECTABLE", "MASKED"]);
        let o = self.objects.get_mut(&rider).unwrap();
        o.disabled.set(DisabledReason::Held, frame, None);
        o.status |= bits;
        o.contained_by = Some(container);
        o.target = None;
        o.path.clear();
        o.speed = 0.0;
        o.velocity = [0.0; 2];
        self.rebuild_partition_now();
        self.riders_upgrade_weapon_set(container);
    }

    /// `removeFromContainViaIterator` and `TransportContain::onRemoving`: the rider
    /// is back in the world at the container's position (or its exit bone).
    fn remove_from_contain(&mut self, container: u32, rider: u32) {
        let Some(c) = self.objects.get_mut(&container).and_then(|o| o.contain.as_mut()) else { return };
        let Some(i) = c.riders.iter().position(|r| *r == rider) else { return };
        c.riders.remove(i);
        let data = c.data.clone();
        let (pos, angle, layer, dead) = {
            let k = &self.objects[&container];
            (k.position, k.angle, k.layer, k.dead)
        };
        let slots = self.slots(rider);
        let bone = (!data.exit_bone.is_empty()).then(|| self.logical_bone(container, &data.exit_bone)).flatten();
        let frame = self.frame;
        let bits = crate::construction::status_bits(&["UNSELECTABLE", "MASKED"]);
        let o = self.objects.get_mut(&rider).unwrap();
        o.position = bone.unwrap_or(pos);
        o.previous_position = o.position;
        o.layer = layer;
        o.disabled.clear(DisabledReason::Held);
        if data.orient_like_container {
            o.angle = angle;
        }
        o.status &= !bits;
        o.contained_by = None;
        let c = self.contain_mut(container);
        c.extra_slots -= slots - 1;
        c.exit_not_busy = frame + data.exit_delay;
        self.rebuild_partition_now();
        self.update_pos(rider);
        if dead {
            self.scatter_to_nearby_position(container, rider);
        }
        if data.reset_mood_on_exit {
            // wakeUpAndAttemptToTarget
            self.objects.get_mut(&rider).unwrap().ai.idle.next_mood_check = frame;
        }
        self.riders_upgrade_weapon_set(container);
    }

    /// `scatterToNearbyPosition`, for riders thrown out of a dying container.
    fn scatter_to_nearby_position(&mut self, container: u32, rider: u32) {
        let angle = self.rng.real(0.0, 2.0 * std::f32::consts::PI);
        let min = Self::bounding(&self.templates.objects[&self.objects[&container].template]).0;
        let dist = self.rng.real(min, min + min / 2.0);
        let c = self.objects[&container].position;
        let to = [dist * ih_math::cos(angle) + c[0], dist * ih_math::sin(angle) + c[1]];
        self.objects.get_mut(&rider).unwrap().angle = angle;
        if self.has_ai(rider) {
            if self.begin_move_ignoring(rider, to, AiState::MoveTo, Default::default(), Some(container)).is_ok() {
                self.objects.get_mut(&rider).unwrap().ai.cmd_from_ai = true;
            }
        } else {
            let z = self.map.ground_height(to[0], to[1]);
            self.objects.get_mut(&rider).unwrap().position = [to[0], to[1], z];
        }
    }

    /// `getSingleLogicalBonePosition`: a bone of the default model in the world.
    fn logical_bone(&self, id: u32, name: &str) -> Option<[f32; 3]> {
        let o = &self.objects[&id];
        let p = self.templates.bones.get(&o.template)?.get(&name.to_ascii_lowercase())?;
        Some(crate::production::transform(o.position, o.angle, *p))
    }

    /// `TransportContain::isSpecificRiderFreeToExit`.
    fn rider_free_to_exit(&self, container: u32, rider: u32) -> bool {
        if self.airborne_now(container) {
            return true;
        }
        if !self.has_ai(rider) {
            return false;
        }
        let k = &self.objects[&container];
        let t = &self.templates.objects[&self.objects[&rider].template];
        let Some(l) = t.locomotor().and_then(|n| self.templates.locomotors.get(n)) else { return false };
        // validMovementTerrain for the rider's surfaces at the container's position.
        let key = crate::simulation::view_key(t, l);
        let terrain = self.views.get(&key).unwrap_or(&self.grid);
        let (x, y) = terrain.world_to_cell([k.position[0], k.position[1]]);
        terrain.cell_at(k.layer, x, y).is_some_and(|c| k.layer != 0 || !terrain.ground.blocked[c.node.unwrap()])
    }
    fn airborne_now(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        o.position[2] - self.map.ground_height(o.position[0], o.position[1]) > 1.0 && o.layer == 0
    }
    /// `TransportContain::isExitBusy`.
    fn exit_busy(&self, container: u32) -> bool {
        let c = self.contain(container).unwrap();
        (c.data.delay_exit_in_air && self.airborne_now(container)) || self.frame < c.exit_not_busy
    }

    /// `privateExit` (`MSG_EXIT`): leave the container, waiting if needed.
    pub(crate) fn begin_exit(&mut self, id: u32, from_ai: bool) -> Result<(), SimError> {
        let Some(container) = self.objects.get(&id).and_then(|o| o.contained_by) else {
            return Err(SimError(format!("object {id} is not inside anything")));
        };
        if self.objects[&container].disabled.has(DisabledReason::Subdued) {
            return Ok(());
        }
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.begin(AiState::Exit, frame);
        o.ai.cmd_from_ai = from_ai;
        o.ai.enter = Some(container);
        self.wake_ai(id);
        Ok(())
    }
    /// `privateEvacuate` (`MSG_EVACUATE`): every passenger is told to get out.
    pub(crate) fn evacuate(&mut self, id: u32, from_ai: bool) -> Result<(), SimError> {
        let Some(c) = self.contain(id) else { return Err(SimError(format!("object {id} carries nothing"))) };
        if self.objects[&id].disabled.has(DisabledReason::Subdued) {
            return Ok(());
        }
        for r in c.riders.clone() {
            if self.has_ai(r) {
                self.begin_exit(r, from_ai)?;
            }
        }
        Ok(())
    }

    /// `AIExitState::update`.
    pub(crate) fn update_exit(&mut self, id: u32) -> u32 {
        let Some(goal) = self.objects[&id].ai.enter.filter(|g| self.contain(*g).is_some()) else {
            self.end_order(id, Outcome::Failure);
            return 1;
        };
        if self.objects[&id].contained_by != Some(goal) {
            self.end_order(id, Outcome::Failure);
            return 1;
        }
        if self.exit_busy(goal) {
            return 1;
        }
        if !self.rider_free_to_exit(goal, id) {
            self.end_order(id, Outcome::Failure);
            return 1;
        }
        self.exit_object_via_door(goal, id);
        if self.objects[&id].ai.state == AiState::Exit {
            self.end_order(id, Outcome::Success);
        }
        1
    }

    /// `OpenContain::exitObjectViaDoor`.
    fn exit_object_via_door(&mut self, container: u32, rider: u32) {
        self.remove_from_contain(container, rider);
        let c = self.contain_mut(container);
        c.door_countdown = c.data.door_open_time;
        let paths = c.data.exit_paths;
        if paths <= 0 {
            return;
        }
        let (mut start, mut end) = ("ExitStart".to_string(), "ExitEnd".to_string());
        if paths > 1 {
            let n = c.which_exit;
            c.which_exit = (n % paths) + 1;
            start = format!("{start}{n:02}");
            end = format!("{end}{n:02}");
        }
        let k = &self.objects[&container];
        let (kpos, angle, layer) = (k.position, k.angle, k.layer);
        let start = self.logical_bone(container, &start).unwrap_or(kpos);
        let end = self.logical_bone(container, &end).unwrap_or(kpos);
        let frame = self.frame;
        let rally = self.contain(container).unwrap().rally;
        let o = self.objects.get_mut(&rider).unwrap();
        o.position = start;
        o.previous_position = start;
        o.angle = angle;
        o.layer = layer;
        if !self.has_ai(rider) {
            return;
        }
        let o = self.objects.get_mut(&rider).unwrap();
        o.ai.ignore_obstacle = None;
        // setIgnoreCollisionTime(LOGICFRAMES_PER_SECOND)
        o.traffic.ignore_until = frame + 30;
        let mut to = [end[0], end[1]];
        let l = self.grid.destination(&self.map, to).layer;
        self.adjust_destination(rider, &mut to, l);
        let mut rest: std::collections::VecDeque<[f32; 2]> = [to].into();
        rest.extend(rally);
        // aiFollowPath(path, me, CMD_FROM_AI)
        if self.begin_move_ignoring(rider, to, AiState::FollowPath, rest, Some(container)).is_ok() {
            self.objects.get_mut(&rider).unwrap().ai.cmd_from_ai = true;
        }
    }

    /// `TransportContain::letRidersUpgradeWeaponSet`.
    fn riders_upgrade_weapon_set(&mut self, container: u32) {
        let Some(c) = self.contain(container) else { return };
        if !c.data.armed_riders_upgrade {
            return;
        }
        let armed = c.riders.iter().any(|r| {
            let o = &self.objects[r];
            self.templates.objects[&o.template].has_kind("INFANTRY")
                && o.weapon_set.weapons.iter().flatten().any(|w| {
                    self.weapon_template(&w.template).is_some_and(|t| !t.is_contact() && t.primary_damage > 0.0)
                })
        });
        let bit = crate::upgrades::weapon_set_flag("PLAYER_UPGRADE");
        let o = self.objects.get_mut(&container).unwrap();
        if armed {
            o.upgrades.weapon_set_flags |= bit;
        } else {
            o.upgrades.weapon_set_flags &= !bit;
        }
        self.update_weapon_set(container);
    }

    /// `TransportContain::update`: the initial payload, health regeneration and the door.
    pub(crate) fn contain_update(&mut self, id: u32) -> u32 {
        let Some(c) = self.contain(id) else { return 1 };
        if !c.payload_created {
            let payload = c.data.payload.clone();
            self.contain_mut(id).payload_created = true;
            if let Some((name, n)) = payload {
                let (p, layer, owner) = (self.objects[&id].position, self.objects[&id].layer, self.objects[&id].owner);
                for _ in 0..n {
                    if let Ok(r) = self.create_object(&name, p, 0.0, layer, owner)
                        && self.valid_container_for(id, r, true)
                    {
                        self.add_to_contain(id, r);
                    }
                }
            }
        }
        let c = self.contain(id).unwrap();
        if c.data.health_regen > 0.0 {
            let regen = c.data.health_regen;
            for r in c.riders.clone() {
                if let Some(h) = self.objects[&r].health.as_ref()
                    && h.current < h.max
                {
                    let amount = h.max * regen / 100.0 * (1.0 / 30.0);
                    self.attempt_healing(r, amount, id);
                }
            }
        }
        let c = self.contain_mut(id);
        if c.door_countdown > 0 {
            c.door_countdown -= 1;
        }
        1
    }

    /// `OpenContain::onDie`: passengers take their share of damage, those that
    /// cannot get out die, the rest scatter.
    pub(crate) fn contain_on_die(&mut self, id: u32) {
        let Some(c) = self.contain(id).cloned() else { return };
        if c.data.damage_percent > 0.0 {
            let death = if c.data.burned_death { "BURNED" } else { "NORMAL" };
            let death = crate::body::death_type(death).unwrap_or(0);
            for r in c.riders.clone() {
                let max = self.objects.get(&r).and_then(|o| o.health.as_ref()).map_or(0.0, |h| h.max);
                let info = crate::body::DamageInfo::new(
                    id,
                    crate::body::DAMAGE_UNRESISTABLE,
                    death,
                    max * c.data.damage_percent,
                );
                self.attempt_damage(r, info);
                if !self.objects[&r].dead && c.data.damage_percent == 1.0 {
                    self.kill(r, crate::body::DAMAGE_UNRESISTABLE, death);
                }
            }
        }
        // killRidersWhoAreNotFreeToExit
        for r in c.riders.clone() {
            if self.objects.contains_key(&r) && !self.rider_free_to_exit(id, r) {
                if c.data.destroy_not_free {
                    self.destroy_object(r);
                } else {
                    let d = crate::body::death_type("NORMAL").unwrap_or(0);
                    self.kill(r, crate::body::DAMAGE_UNRESISTABLE, d);
                }
            }
        }
        // removeAllContained
        while let Some(&r) = self.contain(id).and_then(|c| c.riders.first()) {
            self.remove_from_contain(id, r);
        }
    }
    /// `OpenContain::onDelete`: anything still inside goes with the container.
    pub(crate) fn contain_on_delete(&mut self, id: u32) {
        let Some(c) = self.contain(id) else { return };
        for r in c.riders.clone() {
            self.destroy_object(r);
        }
    }
}
