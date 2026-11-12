//! #45 return data. Strict Borsh decoding rejects truncation and trailing bytes.
use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Clone, Debug, BorshDeserialize, BorshSerialize, PartialEq, Eq)]
pub struct LiquidationHealth {
    pub collateral_value: u64,
    pub debt: u64,
    pub maintenance_bps: u16,
}
impl LiquidationHealth {
    /// Pinned Dusk emergency threshold: equity <= 70% of maintenance.
    /// This prefilter never replaces the program's execution checks.
    pub fn emergency_allowed(&self) -> bool {
        (1..10_000).contains(&self.maintenance_bps)
            && self.debt > 0
            && (self.debt >= self.collateral_value
                || u128::from(self.collateral_value - self.debt) * 10_000 * 100
                    <= u128::from(self.collateral_value) * u128::from(self.maintenance_bps) * 70)
    }

    pub fn eligible(&self) -> bool {
        self.debt > 0
            && (self.collateral_value as u128)
                * (10_000u128.saturating_sub(self.maintenance_bps as u128))
                <= self.debt as u128 * 10_000
    }
}

#[cfg(test)]
mod tests {
    use super::LiquidationHealth;

    #[test]
    fn emergency_requires_deeper_distress_including_the_exact_boundary() {
        for (debt, expected) in [
            (0, false),
            (94_400, false),
            (95_099, false),
            (95_100, true),
            (100_000, true),
        ] {
            assert_eq!(
                LiquidationHealth {
                    collateral_value: 100_000,
                    debt,
                    maintenance_bps: 700
                }
                .emergency_allowed(),
                expected
            );
        }
        assert!(
            LiquidationHealth {
                collateral_value: u64::MAX,
                debt: u64::MAX,
                maintenance_bps: 700
            }
            .emergency_allowed()
        );
        assert!(
            !LiquidationHealth {
                collateral_value: u64::MAX,
                debt: 1,
                maintenance_bps: 700
            }
            .emergency_allowed()
        );
        for maintenance_bps in [0, 10_000] {
            assert!(
                !LiquidationHealth {
                    collateral_value: 0,
                    debt: 1,
                    maintenance_bps
                }
                .emergency_allowed()
            );
        }
    }

    #[test]
    fn exact_maintenance_boundary_is_eligible_like_the_program() {
        assert!(
            LiquidationHealth {
                collateral_value: 100,
                debt: 90,
                maintenance_bps: 1_000,
            }
            .eligible()
        );
    }

    #[test]
    fn debt_free_and_healthy_positions_are_not_eligible() {
        assert!(
            !LiquidationHealth {
                collateral_value: 100,
                debt: 0,
                maintenance_bps: 1_000,
            }
            .eligible()
        );
        assert!(
            !LiquidationHealth {
                collateral_value: 100,
                debt: 89,
                maintenance_bps: 1_000,
            }
            .eligible()
        );
    }
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize, PartialEq, Eq)]
pub struct LiquidationRates {
    pub buyer_discount_bps: u16,
    pub emergency_reward_bps: u16,
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize, PartialEq, Eq)]
pub struct LiquidationFeeAllocation {
    pub total: u64,
    pub protocol: u64,
    pub junior: u64,
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize, PartialEq, Eq)]
pub struct LiquidationLossAllocation {
    pub principal_repaid: u64,
    pub interest_paid: u64,
    pub principal_written_off: u64,
    pub interest_canceled: u64,
    pub surplus: u64,
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize, PartialEq, Eq)]
pub struct FlashLiquidationQuote {
    pub full: bool,
    pub collateral_debit: u64,
    pub collateral_credit: u64,
    pub payment: u64,
    pub repayment: u64,
    pub owner_surplus: u64,
    pub fee: LiquidationFeeAllocation,
    pub debt_shares_to_burn: u128,
    pub health_before: LiquidationHealth,
    pub health_after: LiquidationHealth,
    pub rates: LiquidationRates,
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize, PartialEq, Eq)]
pub struct HlpFlashQuote {
    pub target_asset: u8,
    pub collateral_debit: u64,
    pub collateral_credit: u64,
    pub borrowed_claim: u64,
    pub payment: u64,
    pub reserve_credit: u64,
    pub discount_bps: u16,
    pub full: bool,
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize, PartialEq, Eq)]
pub struct EmergencyLiquidationQuote {
    pub reference_price_nad: u64,
    pub reference_depth_nad: u128,
    pub full: bool,
    pub collateral_debit: u64,
    pub collateral_credit: u64,
    pub swap_output: u64,
    pub repayment: u64,
    pub shares_to_burn: u128,
    pub reward_credit: u64,
    pub reward_debit: u64,
    pub fee: LiquidationFeeAllocation,
    pub junior_fee_debit: u64,
    pub revenue_fee_debit: u64,
    pub owner_debit: u64,
    pub owner_credit: u64,
    pub loss: LiquidationLossAllocation,
    pub health_before: LiquidationHealth,
    pub health_after: LiquidationHealth,
    pub rates: LiquidationRates,
}
