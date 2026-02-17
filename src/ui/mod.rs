mod accounts_table;
mod dialogs;
mod help_bar;
mod status_bar;

use ratatui::layout::{Constraint, Layout};
use ratatui::Frame;

use crate::app::{AppMode, AppState};

pub fn draw(frame: &mut Frame, app: &AppState) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // status bar
        Constraint::Min(5),   // main table
        Constraint::Length(2), // help bar
    ])
    .split(frame.area());

    status_bar::render(frame, chunks[0], app);
    accounts_table::render(frame, chunks[1], app);
    help_bar::render(frame, chunks[2], app);

    // Render modal overlays
    match &app.mode {
        AppMode::AddAccount => {
            dialogs::render_input_dialog(frame, "Add Account", &app.input_fields);
        }
        AppMode::EditAccount(_) => {
            dialogs::render_input_dialog(frame, "Edit Account", &app.input_fields);
        }
        AppMode::ConfirmDelete => {
            if let Some(account) = app.accounts.get(app.selected_index) {
                dialogs::render_confirm_dialog(
                    frame,
                    &format!("Delete '{}'?", account.config.name),
                    "y: confirm  n/Esc: cancel",
                );
            }
        }
        AppMode::ConfirmSwap => {
            if let Some(account) = app.accounts.get(app.selected_index) {
                dialogs::render_confirm_dialog(
                    frame,
                    &format!("Set '{}' as active?", account.config.name),
                    "y: confirm  n/Esc: cancel",
                );
            }
        }
        AppMode::Help => {
            dialogs::render_help_overlay(frame);
        }
        AppMode::Normal => {}
    }
}
