//! Ephemeral presentation notifications. Like FX, these are not game state,
//! never consume RNG, and are deliberately excluded from the state hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    ObjectDied { object: u32, player: u32, killer: Option<u32>, building: bool, sold: bool },
    SuppliesDeposited { player: u32, amount: u32 },
    InsufficientFunds { player: u32 },
    UpgradeComplete { player: u32 },
    PowerUsed { player: u32, power: String },
    CashStolen { player: u32 },
}
