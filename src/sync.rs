use std::sync::atomic::AtomicUsize;

use anyhow::anyhow;
use futures::{StreamExt, TryStreamExt, stream};
use log::info;
use snap_coin::{
    full_node::SharedBlockchain,
    node::{
        message::{Command, Message},
        peer::PeerHandle,
    },
};

const IBD_SAFE_SKIP_TX_HASHING: usize = 500;

pub async fn ibd_blockchain(
    peer: PeerHandle,
    blockchain: SharedBlockchain,
    full_ibd: bool
) -> Result<(), anyhow::Error> {
    info!("Starting initial block download");

    let local_height = blockchain.block_store().get_height();

    // ---- Fetch remote height ----
    let remote_height = match peer
        .request(Message::new(Command::Ping {
            height: local_height,
        }))
        .await?
        .command
    {
        Command::Pong { height } => height,
        _ => return Err(anyhow!("Could not fetch peer height to sync blockchain")),
    };

    if remote_height <= local_height {
        info!("[SYNC] Already synced");
        return Ok(());
    }

    // ---- Fetch block hashes ----
    let hashes = match peer
        .request(Message::new(Command::GetBlockHashes {
            start: local_height,
            end: remote_height,
        }))
        .await?
        .command
    {
        Command::GetBlockHashesResponse { block_hashes } => block_hashes,
        _ => {
            return Err(anyhow!(
                "Could not fetch peer block hashes to sync blockchain"
            ));
        }
    };

    info!("[SYNC] Fetched {} block hashes", hashes.len());

    const BUFFER_SIZE: usize = 10;

    let left = AtomicUsize::new(remote_height);
    
    // ---- Download concurrently, apply sequentially ----
    stream::iter(hashes)
        .map(|hash| {
            let peer = peer.clone();

            async move {
                let resp = peer
                    .request(Message::new(Command::GetBlock { block_hash: hash }))
                    .await?;

                match resp.command {
                    Command::GetBlockResponse { block } => block
                        .ok_or_else(|| anyhow!("Peer returned empty block {}", hash.dump_base36())),
                    _ => Err(anyhow!(
                        "Unexpected response for block {}",
                        hash.dump_base36()
                    )),
                }
            }
        })
        .buffered(BUFFER_SIZE) // 👈 keeps order, runs concurrently
        .try_for_each(|block| async {
            let left_to_add = left.load(std::sync::atomic::Ordering::SeqCst);
            blockchain.add_block(block, left_to_add > IBD_SAFE_SKIP_TX_HASHING && !full_ibd)?;
            if left_to_add > 0 {
                left.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        })
        .await?;

    info!("[SYNC] Blockchain synced successfully");

    Ok(())
}
