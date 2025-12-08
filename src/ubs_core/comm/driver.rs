//! Embedded Media Driver for Development
//!
//! Runs the Aeron Media Driver inside the same process.
//! Use this for development/testing only.
//! For production, run media_driver as a separate process.

use std::sync::Arc;
use std::thread::JoinHandle;

use rusteron_media_driver::{AeronCError, AeronDriver, AeronDriverContext};

/// Embedded media driver handle
///
/// The driver runs in a background thread and stops when this handle is dropped.
pub struct EmbeddedDriver {
    _context: AeronDriverContext,
    _handle: JoinHandle<Result<(), AeronCError>>,
    _stop: Arc<std::sync::atomic::AtomicBool>,
}

impl EmbeddedDriver {
    /// Launch an embedded media driver
    ///
    /// # Example
    /// ```ignore
    /// let driver = EmbeddedDriver::launch()?;
    /// // Driver runs until `driver` is dropped
    /// ```
    pub fn launch() -> Result<Self, String> {
        let context =
            AeronDriverContext::new().map_err(|e| format!("Driver context error: {:?}", e))?;

        let (stop, handle) = AeronDriver::launch_embedded(context.clone(), false);

        log::info!("Aeron Media Driver launched (embedded mode)");
        log::info!("Shared memory: /tmp/aeron-*");

        Ok(Self { _context: context, _handle: handle, _stop: stop })
    }

    /// Launch with custom context
    pub fn launch_with_context(context: AeronDriverContext) -> Result<Self, String> {
        let (stop, handle) = AeronDriver::launch_embedded(context.clone(), false);

        log::info!("Aeron Media Driver launched (embedded mode)");

        Ok(Self { _context: context, _handle: handle, _stop: stop })
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
