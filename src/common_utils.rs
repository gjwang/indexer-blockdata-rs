use chrono::Utc;

/// Get current date as days since Unix epoch (UTC)
pub fn get_current_date() -> i32 {
    let now_ts = Utc::now().timestamp();
    (now_ts / 86400) as i32
}
