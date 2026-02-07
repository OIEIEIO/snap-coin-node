use std::{
    fs::{self, File},
    process::exit,
};

use bincode::{Decode, Encode};
use snap_coin::core::{
    block_store::BlockIndex,
    blockchain::{Blockchain, BlockchainError},
    difficulty::DifficultyState,
};

use crate::deprecated_block_store::BlockStore;

#[derive(Encode, Decode, Debug, Clone)]
struct BlockchainData {
    difficulty_state: DifficultyState,
    block_store: BlockStore,
}

fn load_blockchain_data(blockchain_path: &str) -> Result<BlockchainData, BlockchainError> {
    let mut file = File::open(format!("{}blockchain.dat", blockchain_path))
        .map_err(|e| BlockchainError::Io(e.to_string()))?;
    Ok(
        bincode::decode_from_std_read(&mut file, bincode::config::standard())
            .map_err(|e| BlockchainError::BincodeDecode(e.to_string()))?,
    )
}

pub async fn upgrade(node_path: &str) -> Result<(), anyhow::Error> {
    if fs::exists(node_path.to_string() + "/blockchain/blockchain/blockchain.dat").unwrap() {
        println!("Upgrading blockchain...");
        let data = load_blockchain_data(&(node_path.to_string() + "/blockchain/blockchain/"))?;

        let block_index = BlockIndex::load(
            &(node_path.to_string() + "/blockchain/blockchain/blocks/block-index"),
        );

        for (hash, height) in data.block_store.block_index.write().unwrap().by_hash.iter() {
            block_index
                .by_hash
                .insert(hash.dump_buf(), &height.to_be_bytes())
                .unwrap();
        }

        for (height, hash) in data
            .block_store
            .block_index
            .write()
            .unwrap()
            .by_height
            .iter()
        {
            block_index
                .by_height
                .insert(height.to_be_bytes(), &hash.dump_buf())
                .unwrap();
        }

        // Flush database
        block_index.db.flush().unwrap();

        fs::remove_file(node_path.to_string() + "/blockchain/blockchain/blockchain.dat").unwrap();

        // save new blockchain meta
        Blockchain::save_blockchain_data(
            &(node_path.to_string() + "/blockchain/blockchain/"),
            data.difficulty_state,
            *data.block_store.height.read().unwrap(),
            *data.block_store.last_block.read().unwrap(),
        )?;

        println!(
            "Upgraded blockchain data file to database. Please restart. This should not happen again."
        );
        exit(0);
    }
    Ok(())
}
