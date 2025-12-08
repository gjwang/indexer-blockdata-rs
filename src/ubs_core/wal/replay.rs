//! WAL replay for recovery
//!
//! Reads and validates WAL entries for recovery after crash.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use super::entry::{WalEntry, WalError, HEADER_SIZE};

/// WAL replay iterator
pub struct WalReplay {
    reader: BufReader<File>,
    buffer: Vec<u8>,
    entries_read: u64,
    bytes_read: u64,
    errors: Vec<(u64, WalError)>, // (offset, error)
}

impl WalReplay {
    /// Open WAL file for replay
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, WalError> {
        let file = File::open(path).map_err(|e| WalError::IoError(e.to_string()))?;

        Ok(Self {
            reader: BufReader::new(file),
            buffer: vec![0u8; 64 * 1024], // 64KB read buffer
            entries_read: 0,
            bytes_read: 0,
            errors: Vec::new(),
        })
    }

    /// Read all entries, applying a callback to each
    pub fn replay<F>(&mut self, mut callback: F) -> Result<ReplayStats, WalError>
    where
        F: FnMut(&WalEntry) -> Result<(), WalError>,
    {
        let mut read_buffer = Vec::new();
        let mut total_bytes = 0u64;

        // Read file in chunks
        loop {
            let bytes_read =
                self.reader.read(&mut self.buffer).map_err(|e| WalError::IoError(e.to_string()))?;

            if bytes_read == 0 {
                break; // EOF
            }

            read_buffer.extend_from_slice(&self.buffer[..bytes_read]);
            total_bytes += bytes_read as u64;
        }

        // Parse entries from buffer
        let mut offset = 0;
        while offset < read_buffer.len() {
            let remaining = &read_buffer[offset..];

            if remaining.len() < HEADER_SIZE {
                // Partial entry at end - might be corruption or incomplete write
                if remaining.iter().all(|&b| b == 0) {
                    // Zero padding - normal
                    break;
                }
                self.errors.push((offset as u64, WalError::TooShort));
                break;
            }

            match WalEntry::deserialize(remaining) {
                Ok((entry, consumed)) => {
                    if let Err(e) = callback(&entry) {
                        self.errors.push((offset as u64, e));
                    }
                    self.entries_read += 1;
                    self.bytes_read += consumed as u64;
                    offset += consumed;
                }
                Err(WalError::TooShort) => {
                    // Might be zero padding at end
                    if remaining.iter().all(|&b| b == 0) {
                        break;
                    }
                    self.errors.push((offset as u64, WalError::TooShort));
                    break;
                }
                Err(e) => {
                    self.errors.push((offset as u64, e));
                    // Try to skip to next entry (heuristic: read length field)
                    let len = u32::from_le_bytes([
                        remaining[0],
                        remaining[1],
                        remaining[2],
                        remaining[3],
                    ]) as usize;
                    if len > 0 && len < remaining.len() {
                        offset += len;
                    } else {
                        break; // Can't recover
                    }
                }
            }
        }

        Ok(ReplayStats {
            entries_read: self.entries_read,
            bytes_read: self.bytes_read,
            errors: self.errors.len(),
        })
    }

    /// Get errors encountered during replay
    pub fn errors(&self) -> &[(u64, WalError)] {
        &self.errors
    }
}

/// Statistics from WAL replay
#[derive(Debug, Clone)]
pub struct ReplayStats {
    pub entries_read: u64,
    pub bytes_read: u64,
    pub errors: usize,
}

#[cfg(test)]
mod tests {
    use super::super::entry::WalEntryType;
    use super::super::group_commit::{GroupCommitConfig, GroupCommitWal};
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_replay_empty_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.wal");

        // Create empty WAL
        let config = GroupCommitConfig::default();
        let wal = GroupCommitWal::create(&path, config).unwrap();
        drop(wal);

        // Replay
        let mut replay = WalReplay::open(&path).unwrap();
        let mut count = 0;
        let stats = replay
            .replay(|_entry| {
                count += 1;
                Ok(())
            })
            .unwrap();

        assert_eq!(count, 0);
        assert_eq!(stats.entries_read, 0);
    }

    #[test]
    fn test_replay_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("entries.wal");

        // Write some entries
        let config = GroupCommitConfig::default();
        let mut wal = GroupCommitWal::create(&path, config).unwrap();

        for i in 0..10 {
            let entry = WalEntry::new(WalEntryType::Deposit, vec![i as u8]);
            wal.append(&entry).unwrap();
        }
        wal.flush().unwrap();
        drop(wal);

        // Replay and verify
        let mut replay = WalReplay::open(&path).unwrap();
        let mut entries = Vec::new();
        let stats = replay
            .replay(|entry| {
                entries.push(entry.clone());
                Ok(())
            })
            .unwrap();

        assert_eq!(stats.entries_read, 10);
        assert_eq!(entries.len(), 10);
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.entry_type, WalEntryType::Deposit);
            assert_eq!(entry.payload, vec![i as u8]);
        }
    }

    #[test]
    fn test_replay_mixed_types() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mixed.wal");

        let config = GroupCommitConfig::default();
        let mut wal = GroupCommitWal::create(&path, config).unwrap();

        wal.append(&WalEntry::new(WalEntryType::Deposit, vec![1])).unwrap();
        wal.append(&WalEntry::new(WalEntryType::OrderLock, vec![2])).unwrap();
        wal.append(&WalEntry::new(WalEntryType::Trade, vec![3])).unwrap();
        wal.append(&WalEntry::new(WalEntryType::Fee, vec![4])).unwrap();
        wal.flush().unwrap();
        drop(wal);

        let mut replay = WalReplay::open(&path).unwrap();
        let mut types = Vec::new();
        replay
            .replay(|entry| {
                types.push(entry.entry_type);
                Ok(())
            })
            .unwrap();

        assert_eq!(
            types,
            vec![
                WalEntryType::Deposit,
                WalEntryType::OrderLock,
                WalEntryType::Trade,
                WalEntryType::Fee,
            ]
        );
    }
}
