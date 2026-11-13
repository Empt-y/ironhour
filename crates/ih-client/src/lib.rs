//! The client side of the game: loaded maps, the scene that draws them, and (later)
//! drawables and the tactical camera.

pub mod defs;
pub mod drawable;
pub mod world;

pub use defs::Definitions;
pub use world::{World, WorldError, find_map};
