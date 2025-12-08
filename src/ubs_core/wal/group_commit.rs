//! Group commit WAL
//!
//! Batches multiple writes before fsync to improve throughput
//! while maintaining durability guarantees.

use std::fs::{File, OpenOptions};
use std::io::{Write, Seek, SeekFrom};
use std::path::Path;

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
}

impl Default for GroupCommitConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 100,
            buffer_size: 64 * 1024,  // 64KB
            use_direct_io: false,     // Disable by default for compatibility
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
    /// Create a new WAL file
    pub fn create<P: AsRef<Path>>(path: P, config: GroupCommitConfig) -> Result<Self, WalError> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|e| WalError::IoError(e.to_string()))?;

        Ok(Self {
            file,
            buffer: AlignedBuffer::with_capacity(config.buffer_size),
            pending_count: 0,
            config,
            total_entries: 0,
            bytes_written: 0,
        })
    }

    /// Open existing WAL for append
    pub fn open<P: AsRef<Path>>(path: P, config: GroupCommitConfig) -> Result<Self, WalError> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(path)
            .map_err(|e| WalError::IoError(e.to_string()))?;

        // Seek to end
        file.seek(SeekFrom::End(0))
            .map_err(|e| WalError::IoError(e.to_string()))?;

        Ok(Self {
            file,
            buffer: AlignedBuffer::with_capacity(config.buffer_size),
            pending_count: 0,
            config,
            total_entries: 0,
            bytes_written: 0,
        })
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

    /// Flush pending entries to disk (fsync)
    pub fn flush(&mut self) -> Result<(), WalError> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Write buffer to file
        self.file
            .write_all(self.buffer.as_slice())
            .map_err(|e| WalError::IoError(e.to_string()))?;

        // Sync to disk
        self.file
            .sync_all()
            .map_err(|e| WalError::IoError(e.to_string()))?;

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
    use super::*;
    use super::super::entry::WalEntryType;
    use tempfile::tempdir;

    #[test]
    fn test_wal_create_and_append() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wal");

        let config = GroupCommitConfig {
            max_batch_size: 10,
            buffer_size: 4096,
            use_direct_io: false,
        };

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
            max_batch_size: 5,  // Flush after 5 entries
            buffer_size: 4096,
            use_direct_io: false,
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
