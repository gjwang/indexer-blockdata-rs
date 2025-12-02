use serde::{Deserialize, Serialize};

pub type AssetId = u32;
pub type UserId = u64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[repr(C)]
pub struct Balance {
    pub avail: u64,
    pub frozen: u64,
    pub version: u64,
}

impl Balance {
    pub fn deposit(&mut self, amount: u64) {
        self.avail += amount;
        self.version += 1;
    }

    pub fn withdraw(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.avail < amount {
            return Err("Insufficient funds");
        }
        self.avail -= amount;
        self.version += 1;
        Ok(())
    }

    pub fn lock(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.avail < amount {
            return Err("Insufficient funds");
        }
        self.avail -= amount;
        self.frozen += amount;
        self.version += 1;
        Ok(())
    }

    pub fn unlock(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.frozen < amount {
            return Err("Insufficient frozen funds");
        }
        self.frozen -= amount;
        self.avail += amount;
        self.version += 1;
        Ok(())
    }

    pub fn spend_frozen(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.frozen < amount {
            return Err("Insufficient frozen funds");
        }
        self.frozen -= amount;
        self.version += 1;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAccount {
    pub user_id: UserId,
    pub assets: Vec<(AssetId, Balance)>,
}

impl UserAccount {
    pub fn new(user_id: UserId) -> Self {
        Self {
            user_id,
            assets: Vec::with_capacity(8),
        }
    }
    #[inline(always)]
    pub fn get_balance_mut(&mut self, asset: AssetId) -> &mut Balance {
        if let Some(index) = self.assets.iter().position(|(a, _)| *a == asset) {
            return &mut self.assets[index].1;
        }
        self.assets.push((
            asset,
            Balance {
                avail: 0,
                frozen: 0,
                version: 0,
            },
        ));
        &mut self.assets.last_mut().unwrap().1
    }
}
