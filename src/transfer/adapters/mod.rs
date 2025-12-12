//! Adapters module - service adapters for Funding and Trading

pub mod traits;
pub mod mock;
pub mod funding;
pub mod trading;

pub use traits::ServiceAdapter;
pub use mock::MockAdapter;
pub use funding::FundingAdapter;
pub use trading::TradingAdapter;
