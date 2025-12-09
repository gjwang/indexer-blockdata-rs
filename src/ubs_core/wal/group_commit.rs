//! Group commit WAL
//!
//! Batches multiple writes before fsync to improve throughput
//! while maintaining durability guarantees.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
#[cfg(unix)]
use std::os::unix::fs::FileExt as UnixFileExt;

use fs2::FileExt;
use super::aligned_buffer::AlignedBuffer;
use super::entry::{WalEntry, WalError};

/// Configuration for group commit
#[derive(Debug, Clone)]
pub struct GroupCommitConfig {
    /// Maximum entries to batch before flush
    pub max_batch_size: usize,
    /// Buffer size (should be multiple of alignment)
    pub buffer_size: usize,
    /// Whether to use O_DIRECT (requires aligned buffer)
    pub use_direct_io: bool,
    /// Pre-allocate file size (bytes). 0 = no pre-allocation.
    /// Pre-allocation makes fsync faster for overwrites.
    pub pre_alloc_size: u64,
}

impl Default for GroupCommitConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 100,
            buffer_size: 64 * 1024, // 64KB
            use_direct_io: false,   // Disable by default for compatibility
            pre_alloc_size: 64 * 1024 * 1024, // 64MB pre-allocation
        }
    }
}

/// WAL with group commit (batch fsync)
pub struct GroupCommitWal {
    file: File,
    buffer: AlignedBuffer,
    pending_count: usize,
    config: GroupCommitConfig,
    total_entries: u64,
    bytes_written: u64,
}

impl GroupCommitWal {
    /// Create a new WAL file with proper pre-allocation and zero-fill
    ///
    /// The zero-fill is critical for fast sync_data on APFS:
    /// - allocate() marks blocks as "Unwritten" (security feature)
    /// - Writing to unwritten blocks requires metadata updates
    /// - Zero-fill converts blocks to "Written" at creation time
    /// - Runtime writes become pure overwrites - no metadata updates!
    pub fn create<P: AsRef<Path>>(path: P, config: GroupCommitConfig) -> Result<Self, WalError> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| WalError::IoError(e.to_string()))?;

        // Enable direct I/O if requested
        if config.use_direct_io {
            Self::enable_direct_io(&file)?;
        }

        // Pre-allocate and zero-fill for fast runtime sync
        if config.pre_alloc_size > 0 {
            // STEP 1: Reserve space (fast, but leaves blocks "Unwritten")
            file.allocate(config.pre_alloc_size)
                .map_err(|e| WalError::IoError(format!("fallocate failed: {}", e)))?;

            // STEP 2: Zero-fill to convert "Unwritten" -> "Written"
            // This is slow (~100ms for 64MB) but happens only at creation
            let zero_buf = vec![0u8; 1024 * 1024]; // 1MB buffer
            let mut written = 0u64;
            while written < config.pre_alloc_size {
                let bytes_to_write = std::cmp::min(
                    zero_buf.len() as u64,
                    config.pre_alloc_size - written,
                );
                file.write_all(&zero_buf[0..bytes_to_write as usize])
                    .map_err(|e| WalError::IoError(format!("zero-fill failed: {}", e)))?;
                written += bytes_to_write;
            }

            // STEP 3: Sync ONCE to persist the "zero state"
            file.sync_all().map_err(|e| WalError::IoError(e.to_string()))?;

            // STEP 4: Reset cursor to start for runtime writes
            file.seek(SeekFrom::Start(0))
                .map_err(|e| WalError::IoError(e.to_string()))?;
        }

        Ok(Self {
            file,
            buffer: AlignedBuffer::with_capacity(config.buffer_size),
            pending_count: 0,
            config,
            total_entries: 0,
            bytes_written: 0,
        })
    }

    /// Open existing WAL for append (with optional pre-allocation)
    pub fn open<P: AsRef<Path>>(path: P, config: GroupCommitConfig) -> Result<Self, WalError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| WalError::IoError(e.to_string()))?;

        // Enable direct I/O if requested
        if config.use_direct_io {
            Self::enable_direct_io(&file)?;
        }

        // Pre-allocate file using fallocate (reserves actual disk blocks)
        // On macOS: fcntl(F_PREALLOCATE)
        // CRITICAL: Also set_len() so overwrites don't change file size (avoids APFS metadata updates)
        if config.pre_alloc_size > 0 {
            let metadata = file.metadata().map_err(|e| WalError::IoError(e.to_string()))?;
            if metadata.len() < config.pre_alloc_size {
                // Step 1: Reserve physical blocks
                file.allocate(config.pre_alloc_size)
                    .map_err(|e| WalError::IoError(format!("fallocate failed: {}", e)))?;
                // Step 2: Set logical size so file doesn't "grow" on writes
                file.set_len(config.pre_alloc_size)
                    .map_err(|e| WalError::IoError(format!("set_len failed: {}", e)))?;
            }
        }

        // Find current write position by scanning for end of data
        let cursor = Self::find_write_position(&file)?;

        Ok(Self {
            file,
            buffer: AlignedBuffer::with_capacity(config.buffer_size),
            pending_count: 0,
            config,
            total_entries: 0,
            bytes_written: cursor as u64,
        })
    }

    /// Find the end of written data in pre-allocated file
    fn find_write_position(file: &File) -> Result<usize, WalError> {
        use std::io::Read;

        let metadata = file.metadata().map_err(|e| WalError::IoError(e.to_string()))?;
        if metadata.len() == 0 {
            return Ok(0);
        }

        // For simplicity, seek to end for now (assumes append mode)
        // In production, would scan for first zero-length entry
        let mut file_ref = file;
        let pos = file_ref.seek(SeekFrom::End(0)).map_err(|e| WalError::IoError(e.to_string()))?;

        // If file is pre-allocated but empty, start at 0
        // Check first 4 bytes - if zero, file is empty
        file_ref.seek(SeekFrom::Start(0)).map_err(|e| WalError::IoError(e.to_string()))?;
        let mut header = [0u8; 4];
        if file_ref.read(&mut header).unwrap_or(0) == 4 {
            let len = u32::from_le_bytes(header);
            if len == 0 {
                return Ok(0); // Pre-allocated but empty
            }
        }

        Ok(pos as usize)
    }

    /// Enable direct I/O (bypasses page cache)
    /// On macOS: F_NOCACHE, on Linux: would need O_DIRECT at open time
    #[cfg(target_os = "macos")]
    fn enable_direct_io(file: &File) -> Result<(), WalError> {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        // F_NOCACHE = 48 on macOS
        const F_NOCACHE: i32 = 48;
        let result = unsafe {
            extern "C" { fn fcntl(fd: i32, cmd: i32, ...) -> i32; }
            fcntl(fd, F_NOCACHE, 1)
        };
        if result == -1 {
            return Err(WalError::IoError("Failed to enable F_NOCACHE".into()));
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn enable_direct_io(_file: &File) -> Result<(), WalError> {
        // On Linux, O_DIRECT must be set at open time via custom_flags
        // This is a no-op here; proper impl would use OpenOptionsExt
        Ok(())
    }

    /// Append an entry to the WAL
    /// Entry is buffered; call flush() or commit() to persist
    pub fn append(&mut self, entry: &WalEntry) -> Result<(), WalError> {
        let serialized = entry.serialize();

        // Check if we need to flush first
        if serialized.len() > self.buffer.remaining() {
            self.flush()?;
        }

        // If still doesn't fit, entry is too large
        if serialized.len() > self.buffer.capacity() {
            return Err(WalError::IoError(format!(
                "Entry too large: {} > buffer capacity {}",
                serialized.len(),
                self.buffer.capacity()
            )));
        }

        // Write to buffer
        if !self.buffer.write(&serialized) {
            return Err(WalError::IoError("Buffer write failed".into()));
        }

        self.pending_count += 1;
        self.total_entries += 1;

        // Auto-flush if batch size reached
        if self.pending_count >= self.config.max_batch_size {
            self.flush()?;
        }

        Ok(())
    }

    /// Flush pending entries to disk (fdatasync - faster than fsync)
    /// Uses pwrite (write_all_at) for atomic positional writes
    #[cfg(unix)]
    pub fn flush(&mut self) -> Result<(), WalError> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Use pwrite for atomic positional write (no cursor lock contention)
        self.file
            .write_all_at(self.buffer.as_slice(), self.bytes_written)
            .map_err(|e| WalError::IoError(e.to_string()))?;

        // Sync data to disk (fdatasync - doesn't sync metadata, faster)
        self.file.sync_data().map_err(|e| WalError::IoError(e.to_string()))?;

        self.bytes_written += self.buffer.len() as u64;
        self.buffer.clear();
        self.pending_count = 0;

        Ok(())
    }

    /// Flush pending entries to disk (non-Unix fallback)
    #[cfg(not(unix))]
    pub fn flush(&mut self) -> Result<(), WalError> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Seek + write fallback for non-Unix
        self.file
            .seek(SeekFrom::Start(self.bytes_written))
            .map_err(|e| WalError::IoError(e.to_string()))?;

        self.file
            .write_all(self.buffer.as_slice())
            .map_err(|e| WalError::IoError(e.to_string()))?;

        self.file.sync_data().map_err(|e| WalError::IoError(e.to_string()))?;

        self.bytes_written += self.buffer.len() as u64;
        self.buffer.clear();
        self.pending_count = 0;

        Ok(())
    }

    /// Commit (same as flush, but more semantically clear)
    pub fn commit(&mut self) -> Result<(), WalError> {
        self.flush()
    }

    /// Get pending entry count
    pub fn pending_count(&self) -> usize {
        self.pending_count
    }

    /// Get total entries written
    pub fn total_entries(&self) -> u64 {
        self.total_entries
    }

    /// Get total bytes written
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

impl Drop for GroupCommitWal {
    fn drop(&mut self) {
        // Best effort flush on drop
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::super::entry::WalEntryType;
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_wal_create_and_append() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        let config =
            GroupCommitConfig { max_batch_size: 10, buffer_size: 4096, use_direct_io: false, pre_alloc_size: 0 };

        let mut wal = GroupCommitWal::create(&path, config).unwrap();

        let entry = WalEntry::new(WalEntryType::Deposit, vec![1, 2, 3, 4]);
        wal.append(&entry).unwrap();

        assert_eq!(wal.pending_count(), 1);
        assert_eq!(wal.total_entries(), 1);

        wal.flush().unwrap();

        assert_eq!(wal.pending_count(), 0);
        assert!(wal.bytes_written() > 0);
    }

    #[test]
    fn test_wal_auto_flush() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test2.wal");

        let config = GroupCommitConfig {
            max_batch_size: 5, // Flush after 5 entries
            buffer_size: 4096,
            use_direct_io: false,
            pre_alloc_size: 0,
        };

        let mut wal = GroupCommitWal::create(&path, config).unwrap();

        // Write 5 entries - should auto-flush
        for i in 0..5 {
            let entry = WalEntry::new(WalEntryType::Trade, vec![i]);
            wal.append(&entry).unwrap();
        }

        // Should have auto-flushed
        assert_eq!(wal.pending_count(), 0);
        assert_eq!(wal.total_entries(), 5);
        assert!(wal.bytes_written() > 0);
    }

    #[test]
    fn test_wal_multiple_batches() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test3.wal");

        let config = GroupCommitConfig::default();
        let mut wal = GroupCommitWal::create(&path, config).unwrap();

        // Write 250 entries
        for i in 0..250 {
            let entry = WalEntry::new(WalEntryType::OrderLock, vec![(i % 256) as u8]);
            wal.append(&entry).unwrap();
        }

        wal.flush().unwrap();
        assert_eq!(wal.total_entries(), 250);
    }
}
