//! VIP fee table for UBSCore
//!
//! Simple, stable fee rates. No currency conversion here.
//! Complex fee logic (BNB discounts) handled in Settlement.

/// VIP fee rates table
/// Rates in 10^6 precision (1000 = 0.1%)
pub struct VipFeeTable {
    // (maker_rate, taker_rate) for each VIP level
    rates: [(u64, u64); 10],
}

impl VipFeeTable {
    pub fn new(rates: [(u64, u64); 10]) -> Self {
        Self { rates }
    }

    /// Default VIP rates (similar to Binance)
    pub fn default_rates() -> Self {
        Self::new([
            (1000, 1500), // VIP 0: 0.10%, 0.15%
            (900, 1400),  // VIP 1
            (800, 1300),  // VIP 2
            (700, 1200),  // VIP 3
            (600, 1100),  // VIP 4
            (500, 1000),  // VIP 5
            (400, 900),   // VIP 6
            (300, 800),   // VIP 7
            (200, 600),   // VIP 8
            (100, 400),   // VIP 9: 0.01%, 0.04%
        ])
    }

    /// Get fee rate for VIP level
    pub fn get_rate(&self, vip_level: u8, is_maker: bool) -> u64 {
        let level = (vip_level as usize).min(9);
        if is_maker {
            self.rates[level].0
        } else {
            self.rates[level].1
        }
    }
}

impl Default for VipFeeTable {
    fn default() -> Self {
        Self::default_rates()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vip0_taker_rate() {
        let table = VipFeeTable::default_rates();
        assert_eq!(table.get_rate(0, false), 1500);
    }

    #[test]
    fn test_vip9_maker_rate() {
        let table = VipFeeTable::default_rates();
        assert_eq!(table.get_rate(9, true), 100);
    }

    #[test]
    fn test_vip_level_capped() {
        let table = VipFeeTable::default_rates();
        assert_eq!(table.get_rate(100, true), 100); // Capped to VIP 9
    }
}
