//! Memory-mapped WAL with async flush
//!
//! Uses mmap + flush_async() for low-latency writes.
//! The OS handles flushing to disk asynchronously.
//!
//! Trade-off:
//! - Very low latency (~10-100µs per write)
//! - Data in page cache survives process crash
//! - Data NOT guaranteed to survive power loss/kernel crash

use memmap2::{MmapMut, MmapOptions};
use std::fs::{File, OpenOptions};
use std::io::Result;
use std::path::Path;

use super::entry::{WalEntry, WalError};

const DEFAULT_FILE_SIZE: usize = 64 * 1024 * 1024; // 64MB

/// Memory-mapped WAL with async flush
pub struct MmapWal {
    file: File,
    mmap: MmapMut,
    cursor: usize,
    total_entries: u64,
}

impl MmapWal {
    /// Create or open a mmap WAL
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;

        // Ensure file is at least DEFAULT_FILE_SIZE
        let metadata = file.metadata()?;
        if metadata.len() < DEFAULT_FILE_SIZE as u64 {
            file.set_len(DEFAULT_FILE_SIZE as u64)?;
        }

        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Find cursor position (scan for first zero-length entry)
        let cursor = Self::find_cursor(&mmap);

        Ok(Self {
            file,
            mmap,
            cursor,
            total_entries: 0,
        })
    }

    /// Find the end of written data
    fn find_cursor(mmap: &MmapMut) -> usize {
        let mut pos = 0;
        while pos + 4 <= mmap.len() {
            let len = u32::from_le_bytes(mmap[pos..pos + 4].try_into().unwrap()) as usize;
            if len == 0 {
                return pos;
            }
            // Skip: len(4) + crc(4) + type(1) + payload(len)
            let entry_size = 4 + 4 + 1 + len;
            if pos + entry_size > mmap.len() {
                return pos;
            }
            pos += entry_size;
        }
        pos
    }

    /// Append an entry (just memory copy, very fast)
    pub fn append(&mut self, entry: &WalEntry) -> std::result::Result<(), WalError> {
        let serialized = entry.serialize();

        // Check space
        if self.cursor + serialized.len() > self.mmap.len() {
            return Err(WalError::IoError("WAL full".into()));
        }

        // Memory copy (sub-microsecond)
        self.mmap[self.cursor..self.cursor + serialized.len()].copy_from_slice(&serialized);
        self.cursor += serialized.len();
        self.total_entries += 1;

        Ok(())
    }

    /// Async flush - returns immediately, OS handles persistence
    /// Data survives process crash, but NOT power loss
    pub fn flush_async(&self) -> Result<()> {
        self.mmap.flush_async()
    }

    /// Sync flush - blocks until data is on disk
    /// Data survives power loss
    pub fn flush(&self) -> Result<()> {
        self.mmap.flush()
    }

    /// Get total entries written
    pub fn total_entries(&self) -> u64 {
        self.total_entries
    }

    /// Get bytes written
    pub fn bytes_written(&self) -> usize {
        self.cursor
    }
}
