use std::{fs, path::PathBuf, time::Duration};

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use log::info;
use ratatui::{
    Terminal,
    layout::{Constraint, Direction, Layout},
    prelude::CrosstermBackend,
    widgets::{Borders, Paragraph},
};
use snap_coin::full_node::{SharedBlockchain, node_state::SharedNodeState};

/// Returns the latest log file path in `node_path/logs/` matching the pattern `snap-coin-node_*.log`
fn latest_log_file(node_path: &str) -> Option<PathBuf> {
    let logs_dir = format!("{}/logs", node_path);
    let mut entries: Vec<_> = fs::read_dir(&logs_dir)
        .ok()?
        .filter_map(|res| res.ok())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|f| f.to_str())
                .map(|s| s.starts_with("snap-coin-node_") && s.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();

    // Sort by modified time (descending)
    entries.sort_by_key(|e| {
        fs::metadata(e.path())
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    entries.reverse();

    entries.first().map(|e| e.path())
}

pub async fn run_tui(
    node_state: SharedNodeState,
    blockchain: SharedBlockchain,
    node_port: u16,
    node_path: String,
) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut log_scroll = 0u16;

    // Cached log refresh timer
    let mut last_log_read = std::time::Instant::now();
    let mut cached_log = String::new();

    loop {
        // --- READ NODE STATE SAFELY (NO ASYNC IN DRAW LOOP) ---
        let node_state = {
            // Blockchain
            let height = blockchain.block_store().get_height();
            let syncing = node_state.is_syncing.read().await;
            let last_block = blockchain.block_store().get_last_block_hash().dump_base36();

            // Peer snapshot (NO CLONING PEER)
            let mut peer_snaps = Vec::new();
            for peer in node_state.connected_peers.read().await.values() {
                peer_snaps.push((peer.address, peer.is_client));
            }

            (height, last_block, peer_snaps, syncing)
        };

        let (height, last_block, peer_snaps, syncing) = node_state;

        // --- READ LOG (INFREQUENTLY, NON-BLOCKING) ---
        if last_log_read.elapsed() > Duration::from_millis(300) {
            if let Some(latest) = latest_log_file(&node_path) {
                cached_log = fs::read_to_string(latest).unwrap_or_default();
            }

            last_log_read = std::time::Instant::now();
        }

        terminal.draw(|f| {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Length(3), // bar 1 (increased height for border)
                        Constraint::Length(3), // bar 2
                        Constraint::Min(1),    // log area
                    ]
                    .as_ref(),
                )
                .split(f.area());

            // TOP BAR
            let bar1 = Paragraph::new(format!(
                "P: {} | H: {} | L: {} | S: {}",
                node_port, height, last_block, syncing
            ))
            .block(
                ratatui::widgets::Block::default()
                    .title("NODE STATUS")
                    .borders(Borders::ALL),
            );
            f.render_widget(bar1, layout[0]);

            // PEERS BAR
            let mut peers_line = String::new();
            for (addr, is_client) in &peer_snaps {
                if *is_client {
                    peers_line.push_str(&format!("{}* ", addr));
                } else {
                    peers_line.push_str(&format!("{} ", addr));
                }
            }

            let bar2 = Paragraph::new(peers_line).block(
                ratatui::widgets::Block::default()
                    .title("PEERS")
                    .borders(Borders::ALL),
            );
            f.render_widget(bar2, layout[1]);

            // LOG AREA
            let log_widget = Paragraph::new(cached_log.as_str())
                .block(
                    ratatui::widgets::Block::default()
                        .title("LOGS")
                        .borders(Borders::ALL),
                )
                .scroll((log_scroll, 0));

            f.render_widget(log_widget, layout[2]);
        })?;

        // --- INPUT ---
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Char('q') => break,

                    KeyCode::Up => log_scroll = log_scroll.saturating_sub(1),
                    KeyCode::Down => log_scroll = log_scroll.saturating_add(1),

                    KeyCode::Char('c') => {
                        if let Some(latest) = latest_log_file(&node_path) {
                            let _ = fs::write(latest, "");
                            info!("Log cleared");
                            cached_log.clear();
                            log_scroll = 0;
                        }
                    }

                    _ => {}
                }
            }
        }
    }

    // --- CLEAN EXIT ---
    disable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, LeaveAlternateScreen)?;
    Ok(())
}
