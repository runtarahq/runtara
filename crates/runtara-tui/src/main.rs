// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara TUI - Terminal UI for monitoring Runtara instances and images.

mod app;
mod ui;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

use app::{App, ViewMode};

#[derive(Parser, Debug)]
#[command(name = "runtara-tui")]
#[command(about = "Terminal UI for monitoring Runtara instances and images")]
struct Args {
    /// Runtara environment server address
    #[arg(
        short,
        long,
        env = "RUNTARA_ENV_ADDR",
        default_value = "127.0.0.1:8002"
    )]
    server: String,

    /// Skip TLS certificate verification (default: true for local dev)
    #[arg(long, env = "RUNTARA_SKIP_CERT_VERIFICATION", default_value = "true")]
    skip_cert_verification: bool,

    /// Refresh interval in seconds
    #[arg(short, long, default_value = "5")]
    refresh: u64,

    /// Tenant ID filter (optional)
    #[arg(short, long)]
    tenant: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run
    let mut app = App::new(
        &args.server,
        args.skip_cert_verification,
        args.tenant,
        Duration::from_secs(args.refresh),
    );

    let res = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {err:?}");
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    // Initial data fetch
    app.refresh().await;

    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Poll for events with timeout for auto-refresh
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Handle keys based on current view mode
                    match app.view_mode {
                        ViewMode::List => match key.code {
                            KeyCode::Char('q') => return Ok(()),
                            KeyCode::Esc => return Ok(()),
                            KeyCode::Char('r') => app.refresh().await,
                            KeyCode::Tab => app.next_tab(),
                            KeyCode::BackTab => app.previous_tab(),
                            KeyCode::Down | KeyCode::Char('j') => app.next_item(),
                            KeyCode::Up | KeyCode::Char('k') => app.previous_item(),
                            KeyCode::Char('1') => app.set_tab(0),
                            KeyCode::Char('2') => app.set_tab(1),
                            KeyCode::Char('3') => app.set_tab(2),
                            KeyCode::Char('4') => app.set_tab(3),
                            KeyCode::Char('f') => app.cycle_status_filter(),
                            KeyCode::Char('g') => {
                                if app.tab == app::Tab::Metrics {
                                    app.toggle_metrics_granularity();
                                    app.refresh().await;
                                }
                            }
                            KeyCode::Enter => {
                                if app.tab == app::Tab::Instances {
                                    app.open_instance_detail().await;
                                }
                            }
                            _ => {}
                        },
                        ViewMode::InstanceDetail => match key.code {
                            KeyCode::Esc => app.go_back(),
                            KeyCode::Char('c') => app.open_checkpoints_list().await,
                            KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                            KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                            _ => {}
                        },
                        ViewMode::CheckpointsList => match key.code {
                            KeyCode::Esc => app.go_back(),
                            KeyCode::Enter => app.open_checkpoint_detail().await,
                            KeyCode::Down | KeyCode::Char('j') => app.next_checkpoint(),
                            KeyCode::Up | KeyCode::Char('k') => app.previous_checkpoint(),
                            _ => {}
                        },
                        ViewMode::CheckpointDetail => match key.code {
                            KeyCode::Esc => app.go_back(),
                            KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                            KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                            _ => {}
                        },
                    }
                }
            }
        }

        // Auto-refresh check
        if app.should_refresh() {
            app.refresh().await;
        }
    }
}
