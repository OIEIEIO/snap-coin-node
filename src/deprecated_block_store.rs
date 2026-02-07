use std::{collections::HashMap, sync::RwLock};

use bincode::{Decode, Encode};

use snap_coin::crypto::Hash;

#[derive(Debug, Encode, Decode, Clone)]
pub struct BlockIndex {
    pub by_hash: HashMap<Hash, usize>,
    pub by_height: HashMap<usize, Hash>,
}

#[derive(Debug, Encode, Decode)]
pub struct BlockStore {
    pub store_path: String,
    pub block_index: RwLock<BlockIndex>, // RwLock's are justified, because they only get written to on block add or pop
    pub height: RwLock<usize>,
    pub last_block: RwLock<Hash>,
}

impl Clone for BlockStore {
    /// WARNING: SLOW
    fn clone(&self) -> Self {
        Self {
            store_path: self.store_path.clone(),
            block_index: RwLock::new(self.block_index.read().unwrap().clone()),
            height: RwLock::new(*self.height.read().unwrap()),
            last_block: RwLock::new(*self.last_block.read().unwrap()),
        }
    }
}
