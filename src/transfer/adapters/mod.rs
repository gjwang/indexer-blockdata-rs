//! Adapters module - service adapters for Funding and Trading

pub mod traits;
pub mod mock;
pub mod funding;
pub mod trading;

pub use traits::ServiceAdapter;
pub use mock::MockAdapter;

// Placeholder adapters (for when TigerBeetle is not available)
pub use funding::FundingAdapter;
pub use trading::TradingAdapter;

// TigerBeetle-backed adapters
pub use funding::TbFundingAdapter;
pub use trading::TbTradingAdapter;
