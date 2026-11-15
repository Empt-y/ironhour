//! ironhour command-log input for headless runs and client movement demos.
use crate::{Command, SimError, Simulation, templates::Templates};
use crate::{
    ai::{AiState, Outcome},
    disabled::DisabledReason,
};
use ih_formats::map::Map;
use serde::Deserialize;
use std::sync::Arc;
fn default_seed() -> u32 {
    1
}
fn default_frames() -> u32 {
    600
}
fn default_true() -> bool {
    true
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub map: String,
    #[serde(default = "default_seed")]
    pub seed: u32,
    #[serde(default = "default_frames")]
    pub frames: u32,
    #[serde(default)]
    pub require_idle_at_end: bool,
    /// Headless runner assertion: completed entries and exits at opposite ends.
    #[serde(default)]
    pub require_bridge_traversals: u32,
    #[serde(default = "default_true")]
    pub require_no_overlaps: bool,
    #[serde(default = "default_true")]
    pub require_no_failed_orders: bool,
    #[serde(default)]
    pub require_traffic_repaths: u32,
    #[serde(default)]
    pub spawn: Vec<Spawn>,
    #[serde(default)]
    pub commands: Vec<TimedCommand>,
    #[serde(default)]
    pub check: Vec<Check>,
}
fn default_tolerance() -> f32 {
    0.5
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub frame: u32,
    pub object: u32,
    pub position: Option<[f32; 2]>,
    pub angle: Option<f32>,
    #[serde(default = "default_tolerance")]
    pub tolerance: f32,
    pub ai_state: Option<AiState>,
    pub outcome: Option<Outcome>,
    pub disabled: Option<Vec<DisabledReason>>,
    pub completed_orders: Option<u32>,
    pub stationary: Option<bool>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    pub template: String,
    pub position: [f32; 2],
    #[serde(default)]
    pub owner: u32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct TimedCommand {
    pub frame: u32,
    #[serde(flatten)]
    pub command: Command,
}
impl Scenario {
    pub fn instantiate(&self, map: Arc<Map>, templates: Arc<Templates>) -> Result<Simulation, SimError> {
        let mut sim = Simulation::new(map, templates, self.seed);
        for s in &self.spawn {
            sim.spawn(&s.template, s.position, s.owner)?;
        }
        // Scenario spawns keep IDs 1..n; the map's own objects follow them.
        sim.load_map_objects()?;
        for c in &self.commands {
            if c.frame >= self.frames {
                return Err(SimError(format!("command at {} is beyond scenario's {} frames", c.frame, self.frames)));
            }
            sim.enqueue(c.frame, c.command.clone())?;
        }
        for c in &self.check {
            if c.frame > self.frames
                || !c.tolerance.is_finite()
                || c.tolerance < 0.0
                || c.position.is_some_and(|p| !p.into_iter().all(f32::is_finite))
                || c.angle.is_some_and(|a| !a.is_finite())
            {
                return Err(SimError(format!("invalid checkpoint at frame {}", c.frame)));
            }
        }
        Ok(sim)
    }
    /// Checks describe snapshots after `frame` steps. Commands stamped with that
    /// frame run during the following step. Interactive clients may omit checks.
    pub fn verify_frame(&self, sim: &Simulation) -> Result<(), SimError> {
        for c in self.check.iter().filter(|c| c.frame == sim.frame) {
            let o = sim
                .objects
                .get(&c.object)
                .ok_or_else(|| SimError(format!("frame {} checkpoint: object {} missing", c.frame, c.object)))?;
            let mismatch = c.ai_state.is_some_and(|s| s != o.ai.state)
                || c.outcome.is_some_and(|s| s != o.ai.outcome)
                || c.completed_orders.is_some_and(|n| n != o.ai.completed_orders)
                || c.disabled
                    .as_ref()
                    .is_some_and(|v| v.iter().fold(0u32, |mask, r| mask | (1 << (*r as u32))) != o.disabled.mask)
                || c.position
                    .is_some_and(|p| ih_math::length([p[0] - o.position[0], p[1] - o.position[1]]) > c.tolerance)
                || c.angle.is_some_and(|a| ih_math::angle_delta(o.angle, a).abs() > c.tolerance);
            let stationary = o.position == o.previous_position;
            let mismatch = mismatch || c.stationary.is_some_and(|expected| expected != stationary);
            if mismatch {
                return Err(SimError(format!(
                    "frame {} checkpoint object {} failed: expected {c:?}; observed position {:?}, angle {}, state {:?}, outcome {:?}, disabled {:?}, completed {}",
                    c.frame,
                    c.object,
                    o.position,
                    o.angle,
                    o.ai.state,
                    o.ai.outcome,
                    o.disabled.names(),
                    o.ai.completed_orders
                )));
            }
        }
        Ok(())
    }
}
