use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

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
    widgets::{Block, Borders, Paragraph},
};
use snap_coin::full_node::{SharedBlockchain, node_state::SharedNodeState};

/// Returns the latest log file path in `node_path/logs/`
fn latest_log_file(node_path: &str) -> Option<PathBuf> {
    let logs_dir = format!("{}/logs", node_path);
    let mut entries: Vec<_> = fs::read_dir(&logs_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|f| f.to_str())
                .map(|s| s.starts_with("snap-coin-node_") && s.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();

    entries.sort_by_key(|e| {
        fs::metadata(e.path())
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    entries.reverse();

    entries.first().map(|e| e.path())
}

#[derive(Copy, Clone, PartialEq)]
enum Focus {
    Stats,
    Peers,
    Logs,
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

    let mut focus = Focus::Logs;

    let mut stats_scroll_x = 0u16;
    let mut peers_scroll_x = 0u16;
    let mut logs_scroll_y = 0u16;
    let mut logs_scroll_x = 0u16;

    let mut auto_scroll_logs = true;

    let mut cached_log = String::new();
    let mut last_log_read = Instant::now();
    let mut last_mempool_size = 0usize;

    loop {
        let (height, last_block, peers, syncing) = {
            let height = blockchain.block_store().get_height();
            let last_block = blockchain.block_store().get_last_block_hash().dump_base36();
            let syncing = *node_state.is_syncing.read().await;

            let peers = node_state
                .connected_peers
                .read()
                .await
                .values()
                .map(|p| (p.address, p.is_client))
                .collect::<Vec<_>>();

            (height, last_block, peers, syncing)
        };

        if last_log_read.elapsed() > Duration::from_millis(300) {
            if let Some(latest) = latest_log_file(&node_path) {
                let new_log = fs::read_to_string(latest).unwrap_or_default();
                if new_log.len() != cached_log.len() {
                    cached_log = new_log;
                    if auto_scroll_logs {
                        logs_scroll_y = cached_log.lines().count().saturating_sub(1) as u16;
                    }
                }
            }

            last_mempool_size = node_state.mempool.mempool_size().await;
            last_log_read = Instant::now();
        }

        let client_scores = {
            let guard = node_state.client_health_scores.read().await;
            guard.clone()
        };

        terminal.draw(|f| {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Min(1),
                ])
                .split(f.area());

            let stats_title = if focus == Focus::Stats {
                "*NODE STATUS*"
            } else {
                "NODE STATUS"
            };

            let stats = format!(
                "Port: {} | Height: {} | Last: {} | Syncing: {} | Mempool: {}",
                node_port, height, last_block, syncing, last_mempool_size
            );

            f.render_widget(
                Paragraph::new(stats)
                    .scroll((0, stats_scroll_x))
                    .block(Block::default().title(stats_title).borders(Borders::ALL)),
                layout[0],
            );

            let peers_title = if focus == Focus::Peers {
                "*PEERS*"
            } else {
                "PEERS"
            };

            let peers_line = peers
                .iter()
                .map(|(a, c)| {
                    let score = client_scores.get(&a.ip()).unwrap_or(&0u8);
                    if *c {
                        format!("{}({})* ", a, score)
                    } else {
                        format!("{}({}) ", a, score)
                    }
                })
                .collect::<String>();

            f.render_widget(
                Paragraph::new(peers_line)
                    .scroll((0, peers_scroll_x))
                    .block(Block::default().title(peers_title).borders(Borders::ALL)),
                layout[1],
            );

            let logs_title = if focus == Focus::Logs {
                "*LOGS*"
            } else {
                "LOGS"
            };

            f.render_widget(
                Paragraph::new(cached_log.as_str())
                    .scroll((logs_scroll_y, logs_scroll_x))
                    .block(Block::default().title(logs_title).borders(Borders::ALL)),
                layout[2],
            );
        })?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,

                    KeyCode::Tab => {
                        focus = match focus {
                            Focus::Stats => Focus::Peers,
                            Focus::Peers => Focus::Logs,
                            Focus::Logs => Focus::Stats,
                        };
                    }

                    KeyCode::Up => {
                        if focus == Focus::Logs {
                            logs_scroll_y = logs_scroll_y.saturating_sub(1);
                            auto_scroll_logs = false;
                        }
                    }

                    KeyCode::Down => {
                        if focus == Focus::Logs {
                            logs_scroll_y =
                                (logs_scroll_y + 1).min(cached_log.lines().count() as u16);
                        }
                    }

                    KeyCode::Left => match focus {
                        Focus::Stats => stats_scroll_x = stats_scroll_x.saturating_sub(1),
                        Focus::Peers => peers_scroll_x = peers_scroll_x.saturating_sub(1),
                        Focus::Logs => logs_scroll_x = logs_scroll_x.saturating_sub(1),
                    },

                    KeyCode::Right => match focus {
                        Focus::Stats => stats_scroll_x += 1,
                        Focus::Peers => peers_scroll_x += 1,
                        Focus::Logs => logs_scroll_x += 1,
                    },

                    KeyCode::Char('c') if focus == Focus::Logs => {
                        if let Some(latest) = latest_log_file(&node_path) {
                            let _ = fs::write(latest, "");
                            info!("Log cleared");
                            cached_log.clear();
                            logs_scroll_y = 0;
                            auto_scroll_logs = true;
                        }
                    }

                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, LeaveAlternateScreen)?;
    Ok(())
}
