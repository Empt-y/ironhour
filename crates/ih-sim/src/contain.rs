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
    /// `GarrisonContain` (else `TransportContain` or `TunnelContain`).
    pub garrison: bool,
    /// `TunnelContain`: the player's tunnels share one list of passengers.
    pub tunnel: bool,
    pub passengers_fire: bool,
    pub mobile_garrison: bool,
    /// `HealObjects`.
    pub heal: bool,
    /// `TimeForFullHeal`, in frames.
    pub heal_frames: f32,
    pub roster: Option<(String, u32)>,
    pub enclosing: bool,
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
        let (garrison, tunnel) = match class {
            "TransportContain" => (false, false),
            "GarrisonContain" => (true, false),
            "TunnelContain" => (false, true),
            _ => return None,
        };
        let mut d = Self {
            class: class.into(),
            garrison,
            tunnel,
            passengers_fire: false,
            mobile_garrison: false,
            heal: false,
            heal_frames: 0.0,
            roster: None,
            enclosing: true,
            capacity: 0,
            damage_percent: 0.0,
            burned_death: true,
            // OpenContain allows everything; transports only infantry.
            allow_inside: (!garrison && !tunnel).then(|| vec!["INFANTRY".into()]),
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
                "Slots" if !garrison => d.capacity = num() as i32,
                "ContainMax" if garrison => d.capacity = num() as i32,
                "PassengersAllowedToFire" => d.passengers_fire = yes,
                "MobileGarrison" => d.mobile_garrison = yes,
                "HealObjects" => d.heal = yes,
                // INI::parseDurationReal
                "TimeForFullHeal" => d.heal_frames = num() * 30.0 / 1000.0,
                "InitialRoster" => {
                    let n = v.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
                    d.roster = Some((first.clone(), n));
                }
                "IsEnclosingContainer" => d.enclosing = yes,
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
        // Tunnels always heal, over TimeForFullHeal.
        d.heal |= tunnel;
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
    /// `m_originalTeam`'s player while garrisoned.
    pub original_owner: Option<u32>,
    /// `m_garrisonPointData`: who stands at each of the 40 fire points.
    pub points: Vec<Option<u32>>,
}
/// A player's `TunnelTracker`: its tunnels and everyone inside them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TunnelTracker {
    pub tunnels: Vec<u32>,
    pub riders: Vec<u32>,
}

/// `MAX_GARRISON_POINTS`.
pub const GARRISON_POINTS: usize = 40;
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
            original_owner: None,
            points: vec![None; GARRISON_POINTS],
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
    /// The passengers of a container: its own, or its player's tunnel network.
    pub fn riders_of(&self, container: u32) -> Vec<u32> {
        match self.contain(container) {
            Some(c) if c.data.tunnel => {
                self.players.get(&self.objects[&container].owner).map(|p| p.tunnels.riders.clone()).unwrap_or_default()
            }
            Some(c) => c.riders.clone(),
            None => vec![],
        }
    }
    fn rider_list_mut(&mut self, container: u32) -> &mut Vec<u32> {
        if self.contain(container).is_some_and(|c| c.data.tunnel) {
            let owner = self.objects[&container].owner;
            &mut self.player(owner).tunnels.riders
        } else {
            &mut self.contain_mut(container).riders
        }
    }
    /// `TunnelTracker::onTunnelCreated` (`TunnelContain::onObjectCreated`).
    pub(crate) fn register_tunnel(&mut self, id: u32) {
        if self.contain(id).is_some_and(|c| c.data.tunnel) {
            let owner = self.objects[&id].owner;
            self.player(owner).tunnels.tunnels.push(id);
        }
    }
    /// `TunnelTracker::onTunnelDestroyed`: with the last tunnel gone, everyone
    /// inside is destroyed; otherwise those inside the dead one move to another.
    fn tunnel_destroyed(&mut self, id: u32) {
        let owner = self.objects[&id].owner;
        let t = &mut self.player(owner).tunnels;
        let Some(i) = t.tunnels.iter().position(|x| *x == id) else { return };
        t.tunnels.remove(i);
        if t.tunnels.is_empty() {
            for r in std::mem::take(&mut t.riders) {
                if let Some(o) = self.objects.get_mut(&r) {
                    o.contained_by = None;
                }
                self.destroy_object(r);
            }
        } else {
            let other = t.tunnels[0];
            for r in t.riders.clone() {
                if let Some(o) = self.objects.get_mut(&r)
                    && o.contained_by == Some(id)
                {
                    o.contained_by = Some(other);
                }
            }
        }
    }
    /// Whether a rider is inside a container (or, for tunnels, its network).
    fn is_inside(&self, container: u32, rider: u32) -> bool {
        self.riders_of(container).contains(&rider)
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
        if !allowed {
            return false;
        }
        if c.data.tunnel {
            // TunnelTracker::isValidContainerFor
            return !t.has_kind("AIRCRAFT")
                && (!check_capacity || (self.riders_of(container).len() as i32) < self.templates.max_tunnel_capacity);
        }
        if c.data.garrison {
            // GarrisonContain::isValidContainerFor
            let k = &self.objects[&container];
            let Some(h) = k.health.as_ref() else { return false };
            let kt = &self.templates.objects[&k.template];
            if h.current <= 0.0
                || (h.state == crate::body::DamageState::ReallyDamaged && !kt.has_kind("GARRISONABLE_UNTIL_DESTROYED"))
                || t.has_kind("NO_GARRISON")
            {
                return false;
            }
            return !check_capacity || (c.riders.len() as i32) < c.data.capacity;
        }
        if self.objects[&rider].owner != self.objects[&container].owner {
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
        if t.contain.is_none() {
            return false;
        }
        // With stealth garrisons (not ported) a foreign container could skip the capacity check.
        let check = check_capacity;
        if t.owner != o.owner {
            // Someone else's occupied building (no stealth garrisons yet).
            if !self.riders_of(target).is_empty() {
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
        self.rider_list_mut(container).push(rider);
        let c = self.contain_mut(container);
        let (garrison, tunnel) = (c.data.garrison, c.data.tunnel);
        if !garrison && !tunnel {
            c.extra_slots += slots - 1;
        }
        // addOrRemoveObjFromWorld(false): out of the partition and the pathfinder.
        self.forget_cells(rider);
        let frame = self.frame;
        let bits = crate::construction::status_bits(&["UNSELECTABLE", "MASKED"]);
        let o = self.objects.get_mut(&rider).unwrap();
        o.disabled.set(DisabledReason::Held, frame, None);
        o.status |= bits;
        o.contained_by = Some(container);
        o.contained_frame = frame;
        o.target = None;
        o.path.clear();
        o.speed = 0.0;
        o.velocity = [0.0; 2];
        if garrison {
            // GarrisonContain::onContaining: the building can attack and takes
            // the occupants' side.
            let can_attack = crate::construction::status_bits(&["CAN_ATTACK"]);
            let p = self.objects[&container].position;
            self.objects.get_mut(&rider).unwrap().position = p;
            self.objects.get_mut(&container).unwrap().status |= can_attack;
            self.recalc_garrison_owner(container);
        } else if !tunnel {
            self.redeploy_occupants(container);
        }
        self.rebuild_partition_now();
        self.riders_upgrade_weapon_set(container);
    }

    /// `recalcApparentControllingPlayer`: a garrisoned building belongs to its
    /// first occupant's player, and goes back when empty.
    fn recalc_garrison_owner(&mut self, container: u32) {
        let owner = self.objects[&container].owner;
        let c = self.contain_mut(container);
        if c.original_owner.is_none() {
            c.original_owner = Some(owner);
        }
        let first = c.riders.first().copied();
        let original = c.original_owner;
        let new = match first {
            Some(r) => self.objects[&r].owner,
            None => {
                self.contain_mut(container).original_owner = None;
                original.unwrap_or(owner)
            }
        };
        self.objects.get_mut(&container).unwrap().owner = new;
    }

    /// `OpenContain::redeployOccupants` / `putObjAtNextFirePoint`: riders sit at the
    /// transport's `FIREPOINTnn` bones, or at its centre.
    fn redeploy_occupants(&mut self, container: u32) {
        let points: Vec<[f32; 3]> =
            (1..=99).map_while(|i| self.logical_bone(container, &format!("firepoint{i:02}"))).collect();
        let pos = self.objects[&container].position;
        let riders = self.contain(container).unwrap().riders.clone();
        for (n, r) in riders.iter().rev().enumerate() {
            let p = if points.is_empty() { pos } else { points[n % points.len()] };
            self.objects.get_mut(r).unwrap().position = p;
        }
    }

    // --- garrisons ---

    /// `findConditionIndex`: fire points for pristine, damaged and really damaged models.
    fn garrison_condition(&self, container: u32) -> usize {
        match self.objects[&container].health.as_ref().map(|h| h.state) {
            Some(crate::body::DamageState::Damaged) => 1,
            Some(crate::body::DamageState::ReallyDamaged | crate::body::DamageState::Rubble) => 2,
            _ => 0,
        }
    }
    /// `loadGarrisonPoints`: the `FIREPOINTnn` bones of the model for that
    /// condition; points without a bone are at the building's position.
    fn garrison_point(&self, container: u32, condition: usize, i: usize) -> [f32; 3] {
        let o = &self.objects[&container];
        let bone = self
            .templates
            .condition_bones
            .get(&o.template)
            .and_then(|b| b[condition].get(&format!("firepoint{:02}", i + 1)));
        bone.map_or(o.position, |p| crate::production::transform(o.position, o.angle, *p))
    }
    /// `findClosestFreeGarrisonPointIndex`.
    fn closest_free_point(&self, container: u32, target: [f32; 3]) -> Option<usize> {
        let c = self.contain(container)?;
        let condition = self.garrison_condition(container);
        let mut best: Option<(usize, f32)> = None;
        for (i, used) in c.points.iter().enumerate() {
            if used.is_some() {
                continue;
            }
            let p = self.garrison_point(container, condition, i);
            let d = (p[0] - target[0]).powi(2) + (p[1] - target[1]).powi(2) + (p[2] - target[2]).powi(2);
            if best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }
    /// `calcBestGarrisonPosition`: where a garrisoned unit would fire from at this target.
    pub(crate) fn garrison_fire_position(&self, rider: u32, target: [f32; 3]) -> Option<[f32; 2]> {
        let k = self.objects.get(&rider)?.contained_by?;
        let c = self.contain(k)?;
        if !c.data.garrison || !c.data.enclosing {
            return None;
        }
        // Already at a point: that is where it fires from.
        if let Some(i) = c.points.iter().position(|p| *p == Some(rider)) {
            let p = self.garrison_point(k, self.garrison_condition(k), i);
            let free =
                self.closest_free_point(k, target).map(|j| self.garrison_point(k, self.garrison_condition(k), j));
            let d = |a: [f32; 3]| (a[0] - target[0]).powi(2) + (a[1] - target[1]).powi(2) + (a[2] - target[2]).powi(2);
            let best = free.filter(|f| d(*f) < d(p)).unwrap_or(p);
            return Some([best[0], best[1]]);
        }
        let i = self.closest_free_point(k, target)?;
        let p = self.garrison_point(k, self.garrison_condition(k), i);
        Some([p[0], p[1]])
    }
    fn put_at_point(&mut self, container: u32, rider: u32, i: usize) {
        let p = self.garrison_point(container, self.garrison_condition(container), i);
        self.contain_mut(container).points[i] = Some(rider);
        self.objects.get_mut(&rider).unwrap().position = p;
    }
    fn remove_from_point(&mut self, container: u32, rider: u32) {
        let Some(c) = self.objects.get_mut(&container).and_then(|o| o.contain.as_mut()) else { return };
        if let Some(i) = c.points.iter().position(|p| *p == Some(rider)) {
            c.points[i] = None;
            let p = self.objects[&container].position;
            self.objects.get_mut(&rider).unwrap().position = p;
        }
    }
    /// A garrisoned unit's current target position, if it is attacking.
    fn rider_target(&self, rider: u32) -> Option<[f32; 3]> {
        let m = self.objects[&rider].ai.attack.as_ref()?;
        match m.victim {
            Some(v) => self.objects.get(&v).map(|o| o.position),
            None => Some(m.position),
        }
    }
    /// `matchObjectsToGarrisonPoints`: attackers take the free fire point
    /// nearest their target, others go back to the centre.
    fn match_garrison_points(&mut self, container: u32) {
        let c = self.contain(container).unwrap();
        if !c.data.enclosing {
            return;
        }
        // removeInvalidObjectsFromGarrisonPoints
        for i in 0..GARRISON_POINTS {
            let Some(r) = self.contain(container).unwrap().points[i] else { continue };
            let attacking = matches!(self.objects[&r].ai.state, AiState::AttackObject | AiState::AttackPosition);
            let valid = attacking
                && self.objects[&r].ai.attack.as_ref().is_some_and(|m| {
                    m.victim.is_none_or(|v| {
                        self.objects.contains_key(&v) && {
                            let cur = self.objects[&r].weapon_set.current as usize;
                            self.within_attack_range(r, cur, None, Some(v), self.objects[&v].position)
                        }
                    })
                });
            if !valid {
                self.remove_from_point(container, r);
            }
        }
        // addValidObjectsToGarrisonPoints, then trackTargets.
        for r in self.contain(container).unwrap().riders.clone() {
            let Some(target) = self.rider_target(r) else { continue };
            let at = self.contain(container).unwrap().points.iter().position(|p| *p == Some(r));
            match at {
                None => {
                    if let Some(i) = self.closest_free_point(container, target) {
                        self.put_at_point(container, r, i);
                    }
                }
                Some(i) => {
                    let Some(j) = self.closest_free_point(container, target) else { continue };
                    let cond = self.garrison_condition(container);
                    let d = |a: [f32; 3]| {
                        (a[0] - target[0]).powi(2) + (a[1] - target[1]).powi(2) + (a[2] - target[2]).powi(2)
                    };
                    let here = self.objects[&r].position;
                    if d(self.garrison_point(container, cond, j)) < d(here) {
                        self.contain_mut(container).points[i] = None;
                        self.put_at_point(container, r, j);
                    }
                }
            }
        }
    }
    /// `GarrisonContain::update` after `OpenContain::update`.
    fn garrison_update(&mut self, id: u32) {
        let c = self.contain(id).unwrap();
        if let Some((name, n)) = c.data.roster.clone().filter(|_| !c.payload_created) {
            // onObjectCreated: the initial roster.
            self.contain_mut(id).payload_created = true;
            let (p, layer, owner) = (self.objects[&id].position, self.objects[&id].layer, self.objects[&id].owner);
            for _ in 0..n {
                if let Ok(r) = self.create_object(&name, p, 0.0, layer, owner)
                    && self.valid_container_for(id, r, true)
                {
                    self.add_to_contain(id, r);
                }
            }
        }
        for r in self.contain(id).unwrap().riders.clone() {
            if self.objects.get(&r).is_none_or(|o| o.dead) {
                self.remove_from_contain(id, r);
            }
        }
        self.match_garrison_points(id);
        // healObjects: a full heal over TimeForFullHeal.
        let c = self.contain(id).unwrap();
        if c.data.heal && c.data.heal_frames > 0.0 {
            let frames = c.data.heal_frames;
            for r in c.riders.clone() {
                if let Some(h) = self.objects[&r].health.as_ref()
                    && h.current < h.max
                {
                    let amount = h.max / frames;
                    self.attempt_healing(r, amount, id);
                }
            }
        }
        let c = self.contain(id).unwrap();
        if c.data.mobile_garrison && self.has_ai(id) {
            let p = self.objects[&id].position;
            for r in c.riders.clone() {
                self.objects.get_mut(&r).unwrap().position = p;
            }
        }
    }
    /// `GarrisonContain::onBodyDamageStateChange`: a really damaged building is abandoned.
    pub(crate) fn garrison_damage_state(&mut self, id: u32, state: crate::body::DamageState) {
        let Some(c) = self.contain(id) else { return };
        if !c.data.garrison || c.riders.is_empty() || state != crate::body::DamageState::ReallyDamaged {
            return;
        }
        if self.templates.objects[&self.objects[&id].template].has_kind("GARRISONABLE_UNTIL_DESTROYED") {
            return;
        }
        let _ = self.evacuate(id, true);
    }
    /// `isPassengerAllowedToFire`.
    pub(crate) fn passenger_allowed_to_fire(&self, rider: u32) -> bool {
        let Some(k) = self.objects.get(&rider).and_then(|o| o.contained_by) else { return true };
        let Some(c) = self.contain(k) else { return false };
        if c.data.garrison {
            return !self.objects[&k].disabled.has(DisabledReason::Subdued);
        }
        c.data.passengers_fire
            && self.templates.objects[&self.objects[&rider].template].has_kind("INFANTRY")
            && self.objects[&k].contained_by.is_none()
    }

    /// `removeFromContainViaIterator` and `TransportContain::onRemoving`: the rider
    /// is back in the world at the container's position (or its exit bone).
    fn remove_from_contain(&mut self, container: u32, rider: u32) {
        let Some(data) = self.contain(container).map(|c| c.data.clone()) else { return };
        let list = self.rider_list_mut(container);
        let Some(i) = list.iter().position(|r| *r == rider) else { return };
        list.remove(i);
        let (pos, angle, layer, dead) = {
            let k = &self.objects[&container];
            (k.position, k.angle, k.layer, k.dead)
        };
        let slots = if data.garrison || data.tunnel { 1 } else { self.slots(rider) };
        if data.garrison {
            self.remove_from_point(container, rider);
        }
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
        if data.garrison {
            // GarrisonContain::onRemoving: back to its own side once empty.
            if c.riders.is_empty() {
                let can_attack = crate::construction::status_bits(&["CAN_ATTACK"]);
                self.objects.get_mut(&container).unwrap().status &= !can_attack;
            }
            let z = self.map.ground_height(pos[0], pos[1]);
            self.objects.get_mut(&rider).unwrap().position[2] = z;
            self.recalc_garrison_owner(container);
        }
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
        let p = self.templates.condition_bones.get(&o.template)?[0].get(&name.to_ascii_lowercase())?;
        Some(crate::production::transform(o.position, o.angle, *p))
    }

    /// `TransportContain::isSpecificRiderFreeToExit`.
    fn rider_free_to_exit(&self, container: u32, rider: u32) -> bool {
        // Only transports can refuse (OpenContain says yes).
        if self.contain(container).is_some_and(|c| c.data.garrison || c.data.tunnel) || self.airborne_now(container) {
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
        if c.data.tunnel || c.data.garrison {
            return false;
        }
        (c.data.delay_exit_in_air && self.airborne_now(container)) || self.frame < c.exit_not_busy
    }

    /// `privateExit` (`MSG_EXIT`): leave the container, waiting if needed.
    pub(crate) fn begin_exit(&mut self, id: u32, from_ai: bool) -> Result<(), SimError> {
        self.begin_exit_via(id, None, from_ai)
    }
    /// `aiExit(container)`: out through that container (any tunnel of the network).
    fn begin_exit_via(&mut self, id: u32, via: Option<u32>, from_ai: bool) -> Result<(), SimError> {
        let Some(container) = via.or_else(|| self.objects.get(&id).and_then(|o| o.contained_by)) else {
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
        let _ = c;
        for r in self.riders_of(id) {
            if self.has_ai(r) {
                self.begin_exit_via(r, Some(id), from_ai)?;
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
        if !self.is_inside(goal, id) {
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
        if self.contain(container).is_some_and(|c| c.data.garrison) {
            return self.garrison_exit(container, rider);
        }
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

    /// `GarrisonContain::exitObjectViaDoor` (`EVAC_BURST_FROM_CENTER`): out of the
    /// building's centre, or its front or back if the centre is not walkable.
    fn garrison_exit(&mut self, container: u32, rider: u32) {
        self.remove_from_contain(container, rider);
        let k = &self.objects[&container];
        let (kp, angle) = (k.position, k.angle);
        let kt = &self.templates.objects[&k.template];
        let offset = kt.number("GeometryMajorRadius", 5.0) * kt.number("Scale", 1.0);
        let walkable = |s: &Self, p: [f32; 2]| {
            let t = &s.templates.objects[&s.objects[&rider].template];
            let Some(l) = t.locomotor().and_then(|n| s.templates.locomotors.get(n)) else { return true };
            let terrain = s.views.get(&crate::simulation::view_key(t, l)).unwrap_or(&s.grid);
            let (x, y) = terrain.world_to_cell(p);
            terrain.cell_at(0, x, y).is_some_and(|c| !terrain.ground.blocked[c.node.unwrap()])
        };
        let (c, s) = (ih_math::cos(angle), ih_math::sin(angle));
        let mut start = [kp[0], kp[1]];
        if self.has_ai(rider) && !walkable(self, start) {
            start = [kp[0] - offset * c, kp[1] - offset * s];
            if !walkable(self, start) {
                start = [start[0] + 2.0 * offset * c, start[1] + 2.0 * offset * s];
                if !walkable(self, start) {
                    start = [kp[0], kp[1]];
                }
            }
        }
        let z = self.map.ground_height(start[0], start[1]);
        let o = self.objects.get_mut(&rider).unwrap();
        o.position = [start[0], start[1], z];
        o.previous_position = o.position;
        o.angle = angle;
        if !self.has_ai(rider) {
            return;
        }
        let mut end = start;
        let l = self.grid.destination(&self.map, end).layer;
        self.adjust_destination(rider, &mut end, l);
        if self.begin_move_ignoring(rider, end, AiState::FollowPath, [end].into(), Some(container)).is_ok() {
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
        if c.data.tunnel {
            // TunnelTracker::healObjects: each tunnel heals the whole network.
            let frames = c.data.heal_frames;
            for r in self.riders_of(id) {
                let Some(h) = self.objects.get(&r).and_then(|o| o.health.as_ref()) else { continue };
                let amount = if (self.frame - self.objects[&r].contained_frame) as f32 >= frames {
                    h.max
                } else {
                    h.max / frames
                };
                self.attempt_healing(r, amount, 0);
            }
            let c = self.contain_mut(id);
            c.door_countdown = c.door_countdown.saturating_sub(1);
            return 1;
        }
        if c.data.garrison {
            self.garrison_update(id);
            let c = self.contain_mut(id);
            c.door_countdown = c.door_countdown.saturating_sub(1);
            return 1;
        }
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
        // containReactToTransformChange: riders follow the transport.
        self.redeploy_occupants(id);
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
        // TunnelContain::onDie: the network loses a tunnel; nobody is thrown out.
        if c.data.tunnel {
            self.tunnel_destroyed(id);
            return;
        }
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
        if c.data.tunnel {
            self.tunnel_destroyed(id);
            return;
        }
        for r in c.riders.clone() {
            self.destroy_object(r);
        }
    }
}
