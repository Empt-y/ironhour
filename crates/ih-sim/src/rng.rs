//! Logic RNG, translated from `Common/RandomValue.cpp`. No client/audio state is
//! shared. Arithmetic wraps as on the original 32-bit unsigned implementation.
#[derive(Clone, Debug)]
pub struct LogicRng {
    pub state: [u32; 6],
}
impl LogicRng {
    pub fn new(seed: u32) -> Self {
        Self {
            state: [0xf22d0e56u32, 0x883126e9, 0xc624dd2f, 0x0702c49c, 0x9e353f7d, 0x6fdf3b64]
                .map(|n| n.wrapping_add(seed)),
        }
    }
    pub fn next_u32(&mut self) -> u32 {
        let mut ax = self.state[5].wrapping_add(self.state[4]);
        let mut carry = u32::from(ax < self.state[5] || ax < self.state[4]);
        self.state[4] = ax;
        for i in (0..4).rev() {
            ax = ax.wrapping_add(self.state[i]).wrapping_add(carry);
            // ADC(ax, ax, seed[i], c) compares its A alias after SUM was assigned.
            // Therefore (SUM < A) is false here; compare the remaining operand.
            carry = u32::from(ax < self.state[i]);
            self.state[i] = ax;
        }
        for i in (0..6).rev() {
            self.state[i] = self.state[i].wrapping_add(1);
            if self.state[i] != 0 {
                break;
            }
            if i == 0 {
                ax = ax.wrapping_add(1);
            }
        }
        ax
    }
    pub fn integer(&mut self, lo: i32, hi: i32) -> i32 {
        let delta = (hi as u32).wrapping_sub(lo as u32).wrapping_add(1);
        if delta == 0 { hi } else { (self.next_u32() % delta).wrapping_add(lo as u32) as i32 }
    }
    pub fn real(&mut self, lo: f32, hi: f32) -> f32 {
        let delta = hi - lo;
        if delta <= 0.0 { hi } else { (self.next_u32() as f32 * (1.0 / 4294967296.0)) * delta + lo }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_integer_range_and_constant_real_do_not_advance_the_stream() {
        let mut r = LogicRng::new(7);
        let before = r.state;
        assert_eq!(r.integer(i32::MIN, i32::MAX), i32::MAX);
        assert_eq!(r.real(3.0, 3.0), 3.0);
        assert_eq!(r.state, before);
        for _ in 0..100 {
            let n = r.integer(-5, 3);
            assert!((-5..=3).contains(&n));
        }
    }
    #[test]
    fn overflowing_adds_and_seed_increment_bubble_carries() {
        let mut r = LogicRng { state: [0, 0, 0, u32::MAX, 1, u32::MAX] };
        assert_eq!(r.next_u32(), 1);
        assert_eq!(r.state, [1, 1, 1, 0, 1, 0]);
    }
}
