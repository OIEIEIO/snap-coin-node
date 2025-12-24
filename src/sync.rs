use anyhow::anyhow;
use log::info;
use snap_coin::{full_node::SharedBlockchain, node::{message::{Command, Message}, peer::PeerHandle}};

pub async fn sync_blockchain(
    peer: PeerHandle,
    blockchain: SharedBlockchain
) -> Result<(), anyhow::Error> {
    info!("Starting initial block download");
    info!("Fetching block hashes");
    let local_height = blockchain.block_store().get_height();
    let remote_height = match peer.request(
        Message::new(Command::Ping {
            height: local_height,
        }),
    )
    .await?
    .command
    {
        Command::Pong { height } => height,
        _ => return Err(anyhow!("Could not fetch peer height to sync blockchain")),
    };

    let hashes = match peer.request(
        Message::new(Command::GetBlockHashes {
            start: local_height,
            end: remote_height,
        }),
    )
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

    for hash in hashes {
        let block = match peer.request(
            Message::new(Command::GetBlock { block_hash: hash }),
        )
        .await?
        .command
        {
            Command::GetBlockResponse { block } => block,
            _ => {
                return Err(anyhow!("Could not fetch peer block {}", hash.dump_base36()));
            }
        };

        if let Some(block) = block {
            blockchain.add_block(block.clone())?;
        } else {
            return Err(anyhow!("Could not fetch peer block {}", hash.dump_base36()));
        }
    }

    Ok(())
}