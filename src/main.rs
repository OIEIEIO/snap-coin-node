use std::{net::IpAddr, time::Duration};

use anyhow::anyhow;
use clap::Parser;
use log::info;
use tokio::{net::lookup_host, time::sleep};

use snap_coin::{
    api::api_server::{self},
    build_block,
    crypto::{Hash, randomx_optimized_mode},
    economics::DEV_WALLET,
    full_node::{
        accept_block, auto_peer::start_auto_peer, auto_reconnect::start_auto_reconnect,
        connect_peer, create_full_node, ibd::ibd_blockchain, p2p_server::start_p2p_server,
    },
};

use crate::tui::run_tui;

mod deprecated_block_store;
mod tui;
mod upgrade;

#[derive(Parser, Debug)]
#[command(name = "snap-coin-node", version)]
struct Args {
    /// Comma-separated list of peer addresses
    #[arg(long, value_delimiter = ',', short = 'P')]
    peers: Vec<String>,

    /// Comma-separated list of reserved IP addresses that the node will not attempt to connect to
    #[arg(long, value_delimiter = ',', short = 'r')]
    reserved_ips: Vec<String>,

    /// IP address to advertise to peers
    #[arg(long, short = 'A')]
    advertise: Option<String>,

    /// Path to the node data directory
    #[arg(long, default_value = "./node-mainnet", short = 'd')]
    node_path: String,

    /// Disable the API server
    #[arg(long)]
    no_api: bool,

    /// API server port
    #[arg(long, default_value_t = 3003, short = 'a')]
    api_port: u16,

    /// Node P2P port
    #[arg(long, default_value_t = 8998, short = 'p')]
    node_port: u16,

    /// Create and submit a genesis block on startup
    #[arg(long)]
    create_genesis: bool,

    /// Run without TUI
    #[arg(long, short = 'H')]
    headless: bool,

    /// Skip initial block download
    #[arg(long)]
    no_ibd: bool,

    /// Validate all transaction hashes during IBD (slower)
    #[arg(long)]
    full_ibd: bool,

    /// Disable automatic peer discovery
    #[arg(long)]
    no_auto_peer: bool,

    /// Enable RandomX optimized (turbo) mode for IBD
    #[arg(long, short = 'T')]
    ibd_turbo: bool,

    /// IBD hashing thread count (default is all available threads). This affects IBD only
    #[arg(long, default_value_t = 0, short = 't')]
    ibd_threads: usize,

    /// Enable tokio-console debug subscriber
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = Args::parse();

    if args.debug {
        use tracing_subscriber::prelude::*;
        if tracing_subscriber::registry()
            .with(console_subscriber::spawn())
            .try_init()
            .is_err()
        {}
    }

    if !args.full_ibd {
        println!(
            "IBD in normal mode. Will not validate transaction hashes that are > 500 blocks away from head."
        );
    }

    let ibd_threads = if args.ibd_threads != 0 {
        args.ibd_threads
    } else {
        std::thread::available_parallelism()?.get()
    };

    if !args.full_ibd {
        println!("IBD will hash on {ibd_threads} threads.");
    }

    if args.ibd_turbo {
        randomx_optimized_mode(true);
        Hash::new(b"INIT"); // Get RandomX initialized
    } else {
        println!("RandomX started in light mode.");
    }

    upgrade::upgrade(&args.node_path).await?;

    let mut resolved_peers = Vec::new();

    for seed in &args.peers {
        match lookup_host(seed).await {
            Ok(addrs) => {
                if let Some(addr) = addrs.into_iter().next() {
                    resolved_peers.push(addr);
                }
            }
            Err(_) => return Err(anyhow!("Failed to resolve or parse seed peer: {seed}")),
        }
    }

    let mut parsed_reserved_ips: Vec<IpAddr> = vec![];
    for reserved_ip in args.reserved_ips {
        parsed_reserved_ips.push(reserved_ip.parse().expect("Reserved ip is invalid"));
    }

    let advertised_ip = if let Some(addr_str) = args.advertise {
        Some(lookup_host(addr_str).await?.next().unwrap())
    } else {
        None
    };

    let start_api = !args.no_api;

    // Create a node and connect it's initial peers to it
    let (blockchain, node_state, latest_log_file) =
        create_full_node(&args.node_path, !args.headless, advertised_ip);
    for initial_peer in &resolved_peers {
        connect_peer(*initial_peer, &blockchain, &node_state).await?;
    }

    *node_state.is_syncing.write().await = true;

    // If no flags against it, start the Snap Coin API server
    if start_api {
        sleep(Duration::from_secs(1)).await;
        let api_server =
            api_server::Server::new(args.api_port as u32, blockchain.clone(), node_state.clone());
        api_server.listen().await?;
    }

    // If the --create-genesis flag passed, create and submit a genesis block
    if args.create_genesis {
        let mut genesis = build_block(&*blockchain, &vec![], DEV_WALLET).await?;
        #[allow(deprecated)]
        genesis.compute_pow()?;
        accept_block(&blockchain, &node_state, genesis).await?;
    }

    // If an initial peer was passed, and no flags against it, connect to the first connected peer, and IBD from it
    if !resolved_peers.is_empty() && !args.no_ibd {
        let blockchain = blockchain.clone();
        let node_state = node_state.clone();
        tokio::spawn(async move {
            sleep(Duration::from_secs(1)).await;
            info!(
                "Blockchain sync status {:?}",
                ibd_blockchain(node_state.clone(), blockchain, args.full_ibd, ibd_threads).await
            );
            *node_state.is_syncing.write().await = false;
        });
    } else {
        *node_state.is_syncing.write().await = false;
    }

    if resolved_peers.len() != 0 {
        let resolved_peers = resolved_peers.clone();

        // Peer complete disconnection watchdog
        let blockchain = blockchain.clone();
        let node_state = node_state.clone();

        let _ = start_auto_reconnect(
            node_state,
            blockchain,
            resolved_peers,
            args.full_ibd,
            ibd_threads,
        );
    }

    if !args.no_auto_peer {
        // No need to capture this join handle
        let _ = start_auto_peer(node_state.clone(), blockchain.clone(), parsed_reserved_ips);
    }

    let p2p_server_handle =
        start_p2p_server(args.node_port, blockchain.clone(), node_state.clone()).await?;

    if args.headless {
        info!("{:?}", p2p_server_handle.await);
    } else {
        run_tui(node_state, blockchain, args.node_port, latest_log_file).await?;
    }

    Ok(())
}
