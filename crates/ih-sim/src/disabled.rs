//! Object.cpp disabled masks and independent end-of-frame expiry timers.
//! Ordered reasons match DisabledTypes.h; effects on special powers, riders,
//! power production, particles and audio await their gameplay systems.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum DisabledReason {
    #[serde(alias = "DEFAULT")]
    Default,
    #[serde(alias = "DISABLED_HACKED")]
    Hacked,
    #[serde(alias = "DISABLED_EMP")]
    Emp,
    #[serde(alias = "DISABLED_HELD")]
    Held,
    #[serde(alias = "DISABLED_PARALYZED")]
    Paralyzed,
    #[serde(alias = "DISABLED_UNMANNED")]
    Unmanned,
    #[serde(alias = "DISABLED_UNDERPOWERED")]
    Underpowered,
    #[serde(alias = "DISABLED_FREEFALL")]
    Freefall,
    #[serde(alias = "DISABLED_AWESTRUCK")]
    Awestruck,
    #[serde(alias = "DISABLED_BRAINWASHED")]
    Brainwashed,
    #[serde(alias = "DISABLED_SUBDUED")]
    Subdued,
    #[serde(alias = "DISABLED_SCRIPT_DISABLED")]
    ScriptDisabled,
    #[serde(alias = "DISABLED_SCRIPT_UNDERPOWERED")]
    ScriptUnderpowered,
}
pub const REASONS: [DisabledReason; 13] = [
    DisabledReason::Default,
    DisabledReason::Hacked,
    DisabledReason::Emp,
    DisabledReason::Held,
    DisabledReason::Paralyzed,
    DisabledReason::Unmanned,
    DisabledReason::Underpowered,
    DisabledReason::Freefall,
    DisabledReason::Awestruck,
    DisabledReason::Brainwashed,
    DisabledReason::Subdued,
    DisabledReason::ScriptDisabled,
    DisabledReason::ScriptUnderpowered,
];
#[derive(Clone, Debug, Default)]
pub struct DisabledStatus {
    pub until: [u32; 13],
    pub mask: u32,
}
impl DisabledStatus {
    pub fn any(&self) -> bool {
        self.mask != 0
    }
    pub fn has(&self, reason: DisabledReason) -> bool {
        self.mask & (1 << (reason as u32)) != 0
    }
    pub fn set(&mut self, reason: DisabledReason, now: u32, frames: Option<u32>) {
        let end = frames.map_or(u32::MAX, |n| now.saturating_add(n));
        self.until[reason as usize] = end;
        if end > now {
            self.mask |= 1 << (reason as u32);
        } else {
            self.mask &= !(1 << (reason as u32));
        }
    }
    pub fn clear(&mut self, reason: DisabledReason) {
        self.until[reason as usize] = 0;
        self.mask &= !(1 << (reason as u32));
    }
    pub fn expire(&mut self, now: u32) -> bool {
        let before = self.mask;
        for reason in REASONS {
            if self.has(reason) && self.until[reason as usize] != u32::MAX && now >= self.until[reason as usize] {
                self.clear(reason);
            }
        }
        self.mask != before
    }
    /// GameLogic uses ANY intersection with a module's exception mask, not an
    /// all-reasons subset test. AI accepts HELD, physics accepts all reasons.
    pub fn allows_module(&self, exceptions: u32) -> bool {
        self.mask == 0 || self.mask & exceptions != 0
    }
    pub fn names(&self) -> Vec<DisabledReason> {
        REASONS.into_iter().filter(|r| self.has(*r)).collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_timers_replace_deadlines_and_expire_at_the_boundary() {
        let mut s = DisabledStatus::default();
        s.set(DisabledReason::Emp, 10, Some(90));
        s.set(DisabledReason::Held, 10, Some(60));
        s.set(DisabledReason::Emp, 20, Some(10));
        assert!(!s.expire(29));
        assert!(s.expire(30));
        assert!(!s.has(DisabledReason::Emp));
        assert!(s.has(DisabledReason::Held));
        s.set(DisabledReason::Unmanned, 30, None);
        s.expire(100);
        assert_eq!(s.names(), [DisabledReason::Unmanned]);
        s.set(DisabledReason::Emp, 100, Some(0));
        assert!(!s.has(DisabledReason::Emp));
    }
    #[test]
    fn module_mask_uses_source_any_intersection_rule() {
        let mut s = DisabledStatus::default();
        let held = 1 << (DisabledReason::Held as u32);
        s.set(DisabledReason::Emp, 0, None);
        assert!(!s.allows_module(held));
        assert!(s.allows_module(u32::MAX));
        s.set(DisabledReason::Held, 0, None);
        assert!(s.allows_module(held));
        s.clear(DisabledReason::Held);
        assert!(!s.allows_module(held));
    }
}
