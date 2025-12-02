use anyhow::Result;
use flatbuffers::FlatBufferBuilder;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use self::wal_schema::wal_schema::{self as fbs, UlidStruct};

#[allow(dead_code, unused_imports, clippy::all, mismatched_lifetime_syntaxes)]
mod wal_schema {
    include!(concat!(env!("OUT_DIR"), "/wal_generated.rs"));
}

use crate::models::{Side, Trade as TradeModel};

#[derive(Debug, Clone)]
pub struct TradeLogEntry {
    pub trade_id: u64,
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Debug, Clone)]
pub enum LogEntry {
    PlaceOrder {
        order_id: u64,
        symbol_id: u32,
        side: Side,
        price: u64,
        quantity: u64,
        user_id: u64,
        timestamp: u64,
    },
    CancelOrder {
        order_id: u64,
        symbol_id: u32,
        timestamp: u64,
    },
    Trade {
        trade_id: u64,
        buy_order_id: u64,
        sell_order_id: u64,
        price: u64,
        quantity: u64,
    },
    TradeBatch(Vec<TradeLogEntry>),
}

fn to_fbs_ulid(id: u64) -> UlidStruct {
    // Map u64 ID to UlidStruct (hi=0, lo=id)
    UlidStruct::new(0, id)
}

pub struct Wal {
    writer: BufWriter<File>,
    pub current_seq: u64,
}

impl Wal {
    pub fn new(path: &Path) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            current_seq: 0,
        })
    }

    pub fn log_place_order(
        &mut self,
        order_id: u64,
        user_id: u64,
        symbol_id: u32,
        side: Side,
        price: u64,
        quantity: u64,
    ) -> Result<(), std::io::Error> {
        let mut builder = FlatBufferBuilder::new();

        let f_side = match side {
            Side::Buy => fbs::OrderSide::Buy,
            Side::Sell => fbs::OrderSide::Sell,
        };

        let order_id_ulid = to_fbs_ulid(order_id);
        // let f_symbol = builder.create_string(symbol); // Removed string creation

        let args = fbs::OrderArgs {
            order_id: Some(&order_id_ulid),
            user_id,
            symbol_id, // Direct u32
            side: f_side,
            price,
            quantity,
            timestamp: 0,
        };
        let place_order = fbs::Order::create(&mut builder, &args);

        let log_entry_args = fbs::WalFrameArgs {
            entry_type: fbs::EntryType::Order,
            entry: Some(place_order.as_union_value()),
            seq: 0,
        };
        let frame = fbs::WalFrame::create(&mut builder, &log_entry_args);
        builder.finish(frame, None);

        let buf = builder.finished_data();
        let len = buf.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(buf)?;
        self.writer.flush()?;

        Ok(())
    }

    pub fn log_cancel_order(&mut self, order_id: u64) -> Result<(), std::io::Error> {
        let mut builder = FlatBufferBuilder::new();

        let order_id_ulid = to_fbs_ulid(order_id);

        let args = fbs::CancelArgs {
            order_id: Some(&order_id_ulid),
        };
        let cancel_order = fbs::Cancel::create(&mut builder, &args);

        let log_entry_args = fbs::WalFrameArgs {
            entry_type: fbs::EntryType::Cancel,
            entry: Some(cancel_order.as_union_value()),
            seq: 0,
        };
        let frame = fbs::WalFrame::create(&mut builder, &log_entry_args);
        builder.finish(frame, None);

        let buf = builder.finished_data();
        let len = buf.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(buf)?;
        self.writer.flush()?;

        Ok(())
    }

    pub fn log_trade(
        &mut self,
        trade_id: u64,
        buy_order_id: u64,
        sell_order_id: u64,
        price: u64,
        quantity: u64,
    ) -> Result<(), std::io::Error> {
        let mut builder = FlatBufferBuilder::new();

        let buy_id = to_fbs_ulid(buy_order_id);
        let sell_id = to_fbs_ulid(sell_order_id);

        let args = fbs::TradeArgs {
            trade_id: trade_id,
            buy_order_id: Some(&buy_id),
            sell_order_id: Some(&sell_id),
            price,
            quantity,
        };
        let trade = fbs::Trade::create(&mut builder, &args);

        let log_entry_args = fbs::WalFrameArgs {
            entry_type: fbs::EntryType::Trade,
            entry: Some(trade.as_union_value()),
            seq: 0,
        };
        let frame = fbs::WalFrame::create(&mut builder, &log_entry_args);
        builder.finish(frame, None);

        let buf = builder.finished_data();
        let len = buf.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(buf)?;
        self.writer.flush()?;

        Ok(())
    }

    pub fn log_trade_batch(&mut self, trades: &[TradeModel]) -> Result<(), std::io::Error> {
        let mut builder = FlatBufferBuilder::new();

        let mut trade_offsets = Vec::with_capacity(trades.len());

        for trade in trades {
            let buy_id = to_fbs_ulid(trade.buy_order_id);
            let sell_id = to_fbs_ulid(trade.sell_order_id);

            let args = fbs::TradeArgs {
                trade_id: trade.trade_id,
                buy_order_id: Some(&buy_id),
                sell_order_id: Some(&sell_id),
                price: trade.price,
                quantity: trade.quantity,
            };
            trade_offsets.push(fbs::Trade::create(&mut builder, &args));
        }

        let trades_vec = builder.create_vector(&trade_offsets);
        let batch_args = fbs::TradeBatchArgs {
            trades: Some(trades_vec),
        };
        let batch = fbs::TradeBatch::create(&mut builder, &batch_args);

        let log_entry_args = fbs::WalFrameArgs {
            entry_type: fbs::EntryType::TradeBatch,
            entry: Some(batch.as_union_value()),
            seq: 0,
        };
        let frame = fbs::WalFrame::create(&mut builder, &log_entry_args);
        builder.finish(frame, None);

        let buf = builder.finished_data();
        let len = buf.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(buf)?;
        self.writer.flush()?;

        Ok(())
    }
    pub fn flush(&mut self) -> Result<(), std::io::Error> {
        self.writer.flush()
    }

    pub fn log_place_order_no_flush(
        &mut self,
        order_id: u64,
        user_id: u64,
        symbol_id: u32,
        side: Side,
        price: u64,
        quantity: u64,
    ) -> Result<(), std::io::Error> {
        let mut builder = FlatBufferBuilder::new();

        let f_side = match side {
            Side::Buy => fbs::OrderSide::Buy,
            Side::Sell => fbs::OrderSide::Sell,
        };

        let order_id_ulid = to_fbs_ulid(order_id);

        let args = fbs::OrderArgs {
            order_id: Some(&order_id_ulid),
            user_id,
            symbol_id, // Direct u32
            side: f_side,
            price,
            quantity,
            timestamp: 0,
        };
        let place_order = fbs::Order::create(&mut builder, &args);

        let log_entry_args = fbs::WalFrameArgs {
            entry_type: fbs::EntryType::Order,
            entry: Some(place_order.as_union_value()),
            seq: 0,
        };
        let frame = fbs::WalFrame::create(&mut builder, &log_entry_args);
        builder.finish(frame, None);

        let buf = builder.finished_data();
        let len = buf.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(buf)?;
        // No flush here
        Ok(())
    }

    pub fn log_cancel_order_no_flush(&mut self, order_id: u64) -> Result<(), std::io::Error> {
        let mut builder = FlatBufferBuilder::new();

        let order_id_ulid = to_fbs_ulid(order_id);

        let args = fbs::CancelArgs {
            order_id: Some(&order_id_ulid),
        };
        let cancel_order = fbs::Cancel::create(&mut builder, &args);

        let log_entry_args = fbs::WalFrameArgs {
            entry_type: fbs::EntryType::Cancel,
            entry: Some(cancel_order.as_union_value()),
            seq: 0,
        };
        let frame = fbs::WalFrame::create(&mut builder, &log_entry_args);
        builder.finish(frame, None);

        let buf = builder.finished_data();
        let len = buf.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(buf)?;
        // No flush here
        Ok(())
    }

    pub fn log_trade_batch_no_flush(&mut self, trades: &[TradeModel]) -> Result<(), std::io::Error> {
        let mut builder = FlatBufferBuilder::new();

        let mut trade_offsets = Vec::with_capacity(trades.len());

        for trade in trades {
            let buy_id = to_fbs_ulid(trade.buy_order_id);
            let sell_id = to_fbs_ulid(trade.sell_order_id);

            let args = fbs::TradeArgs {
                trade_id: trade.trade_id,
                buy_order_id: Some(&buy_id),
                sell_order_id: Some(&sell_id),
                price: trade.price,
                quantity: trade.quantity,
            };
            trade_offsets.push(fbs::Trade::create(&mut builder, &args));
        }

        let trades_vec = builder.create_vector(&trade_offsets);
        let batch_args = fbs::TradeBatchArgs {
            trades: Some(trades_vec),
        };
        let batch = fbs::TradeBatch::create(&mut builder, &batch_args);

        let log_entry_args = fbs::WalFrameArgs {
            entry_type: fbs::EntryType::TradeBatch,
            entry: Some(batch.as_union_value()),
            seq: 0,
        };
        let frame = fbs::WalFrame::create(&mut builder, &log_entry_args);
        builder.finish(frame, None);

        let buf = builder.finished_data();
        let len = buf.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(buf)?;
        // No flush here
        Ok(())
    }
}

pub struct WalReader {
    reader: BufReader<File>,
}

impl WalReader {
    pub fn new(path: &Path) -> Result<Self, std::io::Error> {
        let file = File::open(path)?;
        Ok(Self {
            reader: BufReader::new(file),
        })
    }

    pub fn read_entry(&mut self) -> Result<Option<LogEntry>, std::io::Error> {
        let mut len_buf = [0u8; 4];
        if let Err(e) = self.reader.read_exact(&mut len_buf) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf)?;

        let frame = fbs::root_as_wal_frame(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

        match frame.entry_type() {
            fbs::EntryType::Order => {
                let order = frame.entry_as_order().unwrap();
                let order_id = order.order_id().unwrap().lo();

                Ok(Some(LogEntry::PlaceOrder {
                    order_id,
                    user_id: order.user_id(),
                    symbol_id: order.symbol_id(),
                    side: match order.side() {
                        fbs::OrderSide::Buy => Side::Buy,
                        fbs::OrderSide::Sell => Side::Sell,
                        _ => return Ok(None),
                    },
                    price: order.price(),
                    quantity: order.quantity(),
                    timestamp: order.timestamp(),
                }))
            }
            fbs::EntryType::Cancel => {
                let cancel = frame.entry_as_cancel().unwrap();
                let order_id = cancel.order_id().unwrap().lo();

                Ok(Some(LogEntry::CancelOrder {
                    order_id,
                    symbol_id: 0, // Default to 0 as it's not in the Cancel table
                    timestamp: 0,
                }))
            }
            fbs::EntryType::Trade => {
                let trade = frame.entry_as_trade().unwrap();
                let buy_order_id = trade.buy_order_id().unwrap().lo();
                let sell_order_id = trade.sell_order_id().unwrap().lo();

                Ok(Some(LogEntry::Trade {
                    trade_id: trade.trade_id(),
                    buy_order_id,
                    sell_order_id,
                    price: trade.price(),
                    quantity: trade.quantity(),
                }))
            }
            fbs::EntryType::TradeBatch => {
                let batch = frame.entry_as_trade_batch().unwrap();
                let trades = batch.trades().unwrap();
                let mut entries = Vec::with_capacity(trades.len());

                for trade in trades {
                    let buy_order_id = trade.buy_order_id().unwrap().lo();
                    let sell_order_id = trade.sell_order_id().unwrap().lo();
                    entries.push(TradeLogEntry {
                        trade_id: trade.trade_id(),
                        buy_order_id,
                        sell_order_id,
                        price: trade.price(),
                        quantity: trade.quantity(),
                    });
                }
                Ok(Some(LogEntry::TradeBatch(entries)))
            }
            _ => Ok(None),
        }
    }
}
