use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub status: i32,
    pub msg: String,
    pub data: T,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            status: 0,
            msg: "ok".to_string(),
            data,
        }
    }

    pub fn error(status: i32, msg: String, data: T) -> Self {
        Self { status, msg, data }
    }
}
