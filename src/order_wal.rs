use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread;
use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use flatbuffers::FlatBufferBuilder;
use memmap2::MmapMut;

use self::wal_schema::wal_schema::{
    Cancel, CancelArgs, EntryType, Order as FbsOrder, OrderArgs,
    OrderSide as FbsSide, Trade as FbsTrade, TradeArgs, UlidStruct, WalFrame, WalFrameArgs,
};

#[allow(dead_code, unused_imports)]
mod wal_schema {
    include!(concat!(env!("OUT_DIR"), "/wal_generated.rs"));
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WalSide { Buy, Sell }

#[derive(Debug, Clone)]
pub enum LogEntry {
    PlaceOrder {
        order_id: u64,
        symbol: String,
        side: WalSide,
        price: u64,
        quantity: u64,
        user_id: u64,
        timestamp: u64,
    },
    CancelOrder { id: u64 },
    Trade {
        match_id: u64,
        buy_order_id: u64,
        sell_order_id: u64,
        price: u64,
        quantity: u64,
    },
}

fn to_fbs_ulid(id: u64) -> UlidStruct {
    // Map u64 ID to UlidStruct (hi=0, lo=id)
    UlidStruct::new(0, id)
}

pub struct Wal {
    tx: Sender<LogEntry>,
    pub current_seq: u64,
}

impl Wal {
    pub fn open(path: &Path, start_seq: u64) -> Result<Self> {
        let path = path.to_path_buf();
        let (tx, rx) = bounded(2_000_000);

        thread::spawn(move || {
            Self::background_writer(path, rx, start_seq);
        });

        Ok(Self { tx, current_seq: start_seq })
    }

    fn background_writer(path: PathBuf, rx: Receiver<LogEntry>, mut current_seq: u64) {
        let file = OpenOptions::new().read(true).write(true).create(true).open(&path).unwrap();
        let mut file_len = 1024 * 1024 * 1024;
        if file.metadata().unwrap().len() < file_len { file.set_len(file_len).unwrap(); }
        let mut mmap_opt = Some(unsafe { MmapMut::map_mut(&file).unwrap() });
        let mut builder = FlatBufferBuilder::new();

        let mut cursor = 0;
        {
            let mmap = mmap_opt.as_ref().unwrap();
            while cursor + 4 < file_len as usize {
                let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
                let len = u32::from_le_bytes(len_bytes) as usize;
                if len == 0 { break; }
                cursor += 4 + len;
            }
        }

        const SYNC_BATCH: usize = 5000;
        let mut unsynced_writes = 0;

        loop {
            match rx.recv() {
                Ok(entry) => {
                    current_seq += 1;
                    builder.reset();

                    let (entry_type, entry_offset) = match entry {
                        LogEntry::PlaceOrder { order_id, symbol, side, price, quantity, user_id, timestamp } => {
                            let f_sym = builder.create_string(&symbol);
                            let f_side = match side {
                                WalSide::Buy => FbsSide::Buy,
                                WalSide::Sell => FbsSide::Sell
                            };
                            let f_ulid = to_fbs_ulid(order_id);
                            let order = FbsOrder::create(&mut builder, &OrderArgs {
                                id: Some(&f_ulid),
                                symbol: Some(f_sym),
                                side: f_side,
                                price,
                                quantity,
                                user_id,
                                timestamp,
                            });
                            (EntryType::Order, order.as_union_value())
                        }
                        LogEntry::CancelOrder { id } => {
                            let f_ulid = to_fbs_ulid(id);
                            let cancel = Cancel::create(&mut builder, &CancelArgs { id: Some(&f_ulid) });
                            (EntryType::Cancel, cancel.as_union_value())
                        }
                        LogEntry::Trade { match_id, buy_order_id, sell_order_id, price, quantity } => {
                            let buy_id = to_fbs_ulid(buy_order_id);
                            let sell_id = to_fbs_ulid(sell_order_id);
                            let trade = FbsTrade::create(&mut builder, &TradeArgs {
                                match_id,
                                buy_order_id: Some(&buy_id),
                                sell_order_id: Some(&sell_id),
                                price,
                                quantity,
                            });
                            (EntryType::Trade, trade.as_union_value())
                        }
                    };

                    let frame = WalFrame::create(&mut builder, &WalFrameArgs {
                        seq: current_seq,
                        entry_type,
                        entry: Some(entry_offset),
                    });

                    builder.finish(frame, None);
                    let buf = builder.finished_data();
                    let size = buf.len();

                    // Auto-Grow
                    if cursor + 4 + size >= file_len as usize {
                        mmap_opt.as_ref().unwrap().flush().unwrap();
                        mmap_opt = None;
                        let new_len = file_len + (1024 * 1024 * 1024);
                        file.set_len(new_len).unwrap();
                        file_len = new_len;
                        mmap_opt = Some(unsafe { MmapMut::map_mut(&file).unwrap() });
                    }

                    let mmap = mmap_opt.as_mut().unwrap();
                    mmap[cursor..cursor + 4].copy_from_slice(&(size as u32).to_le_bytes());
                    cursor += 4;
                    mmap[cursor..cursor + size].copy_from_slice(buf);
                    cursor += size;

                    unsynced_writes += 1;
                    if unsynced_writes >= SYNC_BATCH {
                        let _ = mmap.flush_async();
                        unsynced_writes = 0;
                    }
                }
                Err(_) => {
                    if let Some(m) = mmap_opt.as_ref() { let _ = m.flush(); }
                    break;
                }
            }
        }
    }

    pub fn append(&mut self, entry: LogEntry) {
        let _ = self.tx.send(entry);
        self.current_seq += 1;
    }
}
