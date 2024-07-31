use std::convert::TryInto;

use sled::{Db, IVec};

pub(crate) struct SledDb {
    db: Db,
}

impl SledDb {
    pub fn new(path: &str) -> Result<Self, sled::Error> {
        Ok(Self {
            db: sled::open(path)?,
        })
    }

    pub fn insert(&self, key: &str, value: i64) -> Result<(), Box<dyn std::error::Error>> {
        self.db.insert(key.as_bytes(), &value.to_be_bytes())?;
        self.flush().expect("insert flush: failed");
        Ok(())
    }

    pub fn insert_bool(&self, key: &str, value: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.db.insert(key.as_bytes(), &[value as u8])?;
        self.flush().expect("insert flush: failed");
        Ok(())
    }

    pub fn get(&self, key: &str, default: i64) -> i64 {
        match self.db.get(key) {
            Ok(Some(value)) => i64::from_be_bytes(value.as_ref().try_into().unwrap_or([0; 8])),
            _ => default,
        }
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.db.get(key) {
            Ok(Some(value)) => {
                // Assuming we stored booleans as a single byte
                !value.is_empty() && value[0] != 0
            }
            _ => default,
        }
    }

    fn flush(&self) -> sled::Result<usize> {
        self.db.flush()
    }
}
