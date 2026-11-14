//! Simulation math boundary. Operations use f32 and software transcendentals,
//! replacing WWMath/Trig.cpp for self-determinism rather than retail replay parity.
pub use libm::{atan2f as atan2, cosf as cos, sinf as sin, sqrtf as sqrt};
pub const TAU: f32 = std::f32::consts::TAU;
pub fn length(v: [f32; 2]) -> f32 {
    sqrt(v[0] * v[0] + v[1] * v[1])
}
pub fn normalized(v: [f32; 2]) -> [f32; 2] {
    let n = length(v);
    if n > 0.0 { [v[0] / n, v[1] / n] } else { [0.0; 2] }
}
pub fn angle_delta(from: f32, to: f32) -> f32 {
    let mut v = (to - from) % TAU;
    if v > std::f32::consts::PI {
        v -= TAU;
    }
    if v < -std::f32::consts::PI {
        v += TAU;
    }
    v
}
