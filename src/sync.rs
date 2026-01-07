use anyhow::anyhow;
use futures::stream::{FuturesUnordered, StreamExt};
use log::info;
use snap_coin::{
    full_node::SharedBlockchain,
    node::{
        message::{Command, Message},
        peer::PeerHandle,
    },
};

pub async fn sync_blockchain(
    peer: PeerHandle,
    blockchain: SharedBlockchain,
) -> Result<(), anyhow::Error> {
    info!("Starting initial block download");

    let local_height = blockchain.block_store().get_height();
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

    info!("[SYNC] Fetched block hashes");

    // Use FuturesUnordered to buffer block downloads
    let mut block_futures = FuturesUnordered::new();
    const BUFFER_SIZE: usize = 10; // max number of blocks to fetch concurrently

    for hash in hashes {
        let peer = peer.clone();
        // Start the request immediately
        block_futures.push(async move {
            let block = match peer
                .request(Message::new(Command::GetBlock { block_hash: hash }))
                .await
            {
                Ok(resp) => match resp.command {
                    Command::GetBlockResponse { block } => Ok((hash, block)),
                    _ => Err(anyhow!(
                        "Unexpected response for block {}",
                        hash.dump_base36()
                    )),
                },
                Err(e) => Err(anyhow!(
                    "Failed to fetch block {}: {:?}",
                    hash.dump_base36(),
                    e
                )),
            };
            block
        });

        // Keep the buffer size limited
        if block_futures.len() >= BUFFER_SIZE {
            if let Some(result) = block_futures.next().await {
                match result {
                    Ok((_hash, Some(block))) => {
                        blockchain.add_block(block)?
                    },
                    Ok((hash, None)) => {
                        return Err(anyhow!("Peer returned empty block {}", hash.dump_base36()));
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    // Finish remaining futures
    while let Some(result) = block_futures.next().await {
        match result {
            Ok((_hash, Some(block))) => blockchain.add_block(block)?,
            Ok((hash, None)) => {
                return Err(anyhow!("Peer returned empty block {}", hash.dump_base36()));
            }
            Err(e) => return Err(e),
        }
    }

    Ok(())
}
