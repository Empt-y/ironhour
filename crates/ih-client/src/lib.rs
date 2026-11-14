//! The client side of the game: loaded maps, their scene, idle drawables and the
//! tactical camera. Full condition-state drawables come with the simulation.

pub mod camera;
pub mod defs;
pub mod drawable;
pub mod sim_draw;
pub mod world;

pub use camera::TacticalCamera;
pub use defs::Definitions;
pub use world::{World, WorldError, find_map};
