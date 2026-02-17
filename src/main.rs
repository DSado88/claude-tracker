mod api;
mod app;
mod config;
mod error;
mod event;
mod keyring_store;
mod oauth;
mod ui;

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::KeyEventKind;

use app::AppState;
use event::Event;

#[tokio::main]
async fn main() -> Result<()> {
    // Panic hook to restore terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = ratatui::restore();
        original_hook(panic_info);
    }));

    let result = run().await;

    ratatui::restore();
    result
}

async fn run() -> Result<()> {
    let cfg = config::load_or_init()?;
    let mut terminal = ratatui::init();
    let mut app = AppState::from_config(cfg, keyring_store::system_keyring());

    let mut events = event::EventHandler::new(
        Duration::from_secs(1),
        Duration::from_millis(33),
    );
    let event_tx = events.sender();

    // Initial fetch
    api::spawn_fetch_all(&app, &event_tx);

    let poll_interval = Duration::from_secs(app.poll_interval_secs);
    let mut last_poll = Instant::now();

    loop {
        let Some(evt) = events.next().await else {
            break;
        };

        match evt {
            Event::Render => {
                terminal.draw(|frame| ui::draw(frame, &app))?;
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                app::handle_key(&mut app, key, &event_tx);
            }
            Event::Tick => {
                if last_poll.elapsed() >= poll_interval {
                    api::spawn_fetch_all(&app, &event_tx);
                    last_poll = Instant::now();
                }
                app.clear_stale_messages();
            }
            Event::UsageResult {
                account_name,
                result,
            } => {
                app.apply_usage_result(&account_name, result);
            }
            Event::OAuthImportResult { result } => {
                match result {
                    Ok(data) => {
                        if let Some(idx) = app.import_oauth_account(data) {
                            api::spawn_fetch_one(&app, idx, &event_tx);
                        }
                    }
                    Err(msg) => {
                        app.set_status(format!("Import failed: {msg}"));
                    }
                }
            }
            _ => {}
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
