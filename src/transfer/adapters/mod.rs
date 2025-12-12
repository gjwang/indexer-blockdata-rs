//! Adapters module - service adapters for Funding and Trading

pub mod traits;
pub mod mock;
pub mod funding;
pub mod trading;
pub mod atomic;

pub use traits::ServiceAdapter;
pub use mock::MockAdapter;

// Placeholder adapters (for when TigerBeetle is not available)
pub use funding::FundingAdapter;
pub use trading::TradingAdapter;

// TigerBeetle-backed adapters
pub use funding::TbFundingAdapter;
pub use trading::TbTradingAdapter;

// Atomic transfer adapter (uses TigerBeetle's native atomicity)
pub use atomic::AtomicTransferAdapter;

// UBSCore-backed adapter (via Aeron - for production)
#[cfg(feature = "aeron")]
pub use trading::UbsTradingAdapter;
