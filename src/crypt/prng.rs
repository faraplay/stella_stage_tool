use rand_mt::Mt64;

pub(crate) struct MyPrng {
    pub(crate) mt64: Mt64,
    pub(crate) lcg: u8,
    pub(crate) lfsr: u8,
}

impl MyPrng {
    pub(crate) fn new(seed: u64) -> MyPrng {
        let mt64 = Mt64::new(seed);
        let lcg = (seed >> 8) as u8;
        let low_bits = seed as u8;
        let lfsr = if low_bits == 0xFF { 0x55 } else { low_bits };
        MyPrng { mt64, lcg, lfsr }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let next_mt64 = self.mt64.next_u64();
        self.lcg = self.lcg.wrapping_mul(5).wrapping_add(1);
        let new_bit = ((self.lfsr >> 4) ^ !(self.lfsr >> 7)) & 1;
        self.lfsr = (self.lfsr << 1) | new_bit;
        next_mt64 + (self.lcg as u64) + (self.lfsr as u64)
    }

    pub(crate) fn next_interweaved(&mut self) -> u32 {
        let value64 = self.next_u64();
        let lower = value64 as u32;
        let upper = (value64 >> 32) as u32;
        pub(crate) const UPPER_MASK: u32 = 0x55555555;
        (lower & !UPPER_MASK) | (upper & UPPER_MASK)
    }
}
