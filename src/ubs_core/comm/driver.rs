//! Embedded Media Driver for Development
//!
//! Runs the Aeron Media Driver inside the same process.
//! Use this for development/testing only.
//! For production, run media_driver as a separate process.

use std::ffi::CString;
use std::sync::Arc;
use std::thread::JoinHandle;

use rusteron_media_driver::{AeronCError, AeronDriver, AeronDriverContext};

/// Default Aeron directory - all processes must use this to share IPC
pub const AERON_DIR: &str = "/tmp/aeron-trading";

/// Embedded media driver handle
///
/// The driver runs in a background thread and stops when this handle is dropped.
pub struct EmbeddedDriver {
    _context: AeronDriverContext,
    _handle: JoinHandle<Result<(), AeronCError>>,
    _stop: Arc<std::sync::atomic::AtomicBool>,
    dir: String,
}

impl EmbeddedDriver {
    /// Launch an embedded media driver at the default directory
    ///
    /// # Example
    /// ```ignore
    /// let driver = EmbeddedDriver::launch()?;
    /// // Driver runs until `driver` is dropped
    /// ```
    pub fn launch() -> Result<Self, String> {
        Self::launch_at(AERON_DIR)
    }

    /// Launch an embedded media driver at a specific directory
    pub fn launch_at(dir: &str) -> Result<Self, String> {
        let context =
            AeronDriverContext::new().map_err(|e| format!("Driver context error: {:?}", e))?;

        // Set the directory for shared memory
        let dir_cstr = CString::new(dir).map_err(|_| "Invalid directory path")?;
        context.set_dir(&dir_cstr).map_err(|e| format!("Failed to set dir: {:?}", e))?;

        // Delete stale data on start to avoid conflicts with previous runs
        context.set_dir_delete_on_start(true)
            .map_err(|e| format!("Failed to set delete_on_start: {:?}", e))?;

        let (stop, handle) = AeronDriver::launch_embedded(context.clone(), false);

        log::info!("Aeron Media Driver launched at {}", dir);

        Ok(Self { _context: context, _handle: handle, _stop: stop, dir: dir.to_string() })
    }

    /// Get the directory where the driver is running
    pub fn dir(&self) -> &str {
        &self.dir
    }
}

impl Drop for EmbeddedDriver {
    fn drop(&mut self) {
        log::info!("Aeron Media Driver stopping...");
        self._stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    // Note: Integration tests require running media driver
    // Skip in unit tests to avoid resource conflicts
}
