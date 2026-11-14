//! Versioned canonical little-endian state digest for determinism checks. This
//! is ironhour's FNV-1a 64-bit digest, not the retail network CRC/save format.
pub struct StateHash(pub u64);
impl Default for StateHash {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}
impl StateHash {
    pub fn bytes(&mut self, b: &[u8]) {
        for &byte in b {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn float(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    pub fn text(&mut self, v: &str) {
        self.u64(v.len() as u64);
        self.bytes(v.as_bytes());
    }
}
