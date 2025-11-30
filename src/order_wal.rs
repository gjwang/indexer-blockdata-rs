use std::fs::{OpenOptions};
use std::io::Write;
use std::mem;
use std::path::{Path, PathBuf};
use std::thread;
use crossbeam_channel::{bounded, Receiver, Sender};
use memmap2::MmapMut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum WalSide { Buy = 0, Sell = 1 }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[repr(C)]
pub struct RawOrder {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u64,
    pub side: WalSide,
    pub price: u64,
    pub quantity: u64,
    pub timestamp: u64,
    pub _pad: [u8; 15], // Align to 64 bytes
}

const BATCH_SIZE: usize = 1024;

struct WalBatch {
    count: usize,
    entries: Box<[RawOrder; BATCH_SIZE]>,
}

pub struct OrderWal {
    tx: Sender<WalBatch>,
    local_batch: Box<[RawOrder; BATCH_SIZE]>,
    local_count: usize,
}

impl OrderWal {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let path = path.to_path_buf();
        let (tx, rx) = bounded(2000);

        thread::spawn(move || Self::background_writer(path, rx));

        Ok(Self {
            tx,
            local_batch: Box::new([unsafe { mem::zeroed() }; BATCH_SIZE]),
            local_count: 0,
        })
    }

    fn background_writer(path: PathBuf, rx: Receiver<WalBatch>) {
        let file = OpenOptions::new().read(true).write(true).create(true).open(&path).unwrap();
        let mut len = 1024 * 1024 * 1024; // 1GB
        if file.metadata().unwrap().len() < len { file.set_len(len).unwrap(); }

        let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
        let mut cursor = 0;

        // Skip existing
        while cursor + mem::size_of::<RawOrder>() < len as usize {
            let slice = &mmap[cursor..cursor+mem::size_of::<RawOrder>()];
            // Simple check for zeroed memory to find end
            if slice.iter().all(|&x| x == 0) {
                break;
            }
            cursor += mem::size_of::<RawOrder>();
        }

        while let Ok(batch) = rx.recv() {
            let size = batch.count * mem::size_of::<RawOrder>();

            // Auto-Grow
            if cursor + size >= len as usize {
                mmap.flush().unwrap();
                let new_len = len * 2;
                file.set_len(new_len as u64).unwrap();
                len = new_len;
                mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
            }

            // Raw Memcpy
            unsafe {
                let src = batch.entries.as_ptr() as *const u8;
                let dst = mmap.as_mut_ptr().add(cursor);
                std::ptr::copy_nonoverlapping(src, dst, size);
            }
            cursor += size;
        }
    }

    #[inline(always)]
    pub fn append(&mut self, order: RawOrder) {
        unsafe {
            *self.local_batch.get_unchecked_mut(self.local_count) = order;
        }
        self.local_count += 1;

        if self.local_count == BATCH_SIZE {
            let _ = self.tx.send(WalBatch {
                count: BATCH_SIZE,
                entries: self.local_batch.clone(),
            });
            self.local_count = 0;
        }
    }

    pub fn flush(&mut self) {
        if self.local_count > 0 {
            let _ = self.tx.send(WalBatch {
                count: self.local_count,
                entries: self.local_batch.clone(),
            });
            self.local_count = 0;
        }
    }
}
