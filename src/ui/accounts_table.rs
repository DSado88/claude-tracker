use chrono::Utc;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::app::{AccountStatus, AppState};

fn utilization_color(pct: u32) -> Color {
    match pct {
        0..=10 => Color::Indexed(22),
        11..=20 => Color::Indexed(28),
        21..=30 => Color::Indexed(34),
        31..=40 => Color::Indexed(100),
        41..=50 => Color::Indexed(142),
        51..=60 => Color::Indexed(178),
        61..=70 => Color::Indexed(172),
        71..=80 => Color::Indexed(166),
        81..=90 => Color::Indexed(160),
        _ => Color::Indexed(124),
    }
}

fn progress_bar(pct: u32) -> String {
    let filled = if pct == 0 {
        0
    } else if pct >= 100 {
        10
    } else {
        ((pct * 10 + 50) / 100).clamp(0, 10)
    };
    let empty = 10 - filled;
    let mut bar = String::new();
    for _ in 0..filled {
        bar.push('\u{2593}'); // ▓
    }
    for _ in 0..empty {
        bar.push('\u{2591}'); // ░
    }
    bar
}

fn format_countdown(resets_at: &chrono::DateTime<Utc>) -> String {
    let now = Utc::now();
    let diff = resets_at.signed_duration_since(now);
    let total_secs = diff.num_seconds();

    if total_secs <= 0 {
        return "now".to_string();
    }

    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;

    if hours > 0 {
        format!("{}h {:02}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    let header = Row::new(vec![
        Cell::from(" # "),
        Cell::from("Name"),
        Cell::from("Usage"),
        Cell::from("Bar"),
        Cell::from("Resets In"),
        Cell::from("Status"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .accounts
        .iter()
        .enumerate()
        .map(|(i, account)| {
            let is_selected = i == app.selected_index;
            let is_active = i == app.active_account_index;

            let prefix = if is_selected { ">" } else { " " };
            let num = format!("{}{}", prefix, i + 1);

            let name = if is_active {
                format!("{} *", account.config.name)
            } else {
                account.config.name.clone()
            };

            let (usage_text, bar_text, resets_text, status_text, row_color) =
                match &account.status {
                    AccountStatus::Idle => (
                        "--".to_string(),
                        "----------".to_string(),
                        "--".to_string(),
                        "Idle".to_string(),
                        Color::DarkGray,
                    ),
                    AccountStatus::Fetching => (
                        "...".to_string(),
                        "----------".to_string(),
                        "--".to_string(),
                        "...".to_string(),
                        Color::DarkGray,
                    ),
                    AccountStatus::Ok => {
                        if let Some(usage) = &account.usage {
                            let color = utilization_color(usage.utilization);
                            let resets = usage
                                .resets_at
                                .as_ref()
                                .map(format_countdown)
                                .unwrap_or_else(|| "--".to_string());
                            (
                                format!("{}%", usage.utilization),
                                progress_bar(usage.utilization),
                                resets,
                                "OK".to_string(),
                                color,
                            )
                        } else {
                            (
                                "--".to_string(),
                                "----------".to_string(),
                                "--".to_string(),
                                "OK".to_string(),
                                Color::DarkGray,
                            )
                        }
                    }
                    AccountStatus::Error(msg) => {
                        let short = if msg.len() > 20 {
                            format!("{}...", &msg[..17])
                        } else {
                            msg.clone()
                        };
                        (
                            "--".to_string(),
                            "----------".to_string(),
                            "--".to_string(),
                            short,
                            Color::Red,
                        )
                    }
                };

            let style = if is_selected {
                Style::default()
                    .fg(row_color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(row_color)
            };

            Row::new(vec![
                Cell::from(num),
                Cell::from(name),
                Cell::from(usage_text),
                Cell::from(bar_text),
                Cell::from(resets_text),
                Cell::from(status_text),
            ])
            .style(style)
        })
        .collect();

    let empty_msg = if app.accounts.is_empty() {
        vec![Row::new(vec![Cell::from(Line::from(vec![
            Span::styled(
                "  No accounts configured. Press 'a' to add one.",
                Style::default().fg(Color::DarkGray),
            ),
        ]))])]
    } else {
        vec![]
    };

    let display_rows = if app.accounts.is_empty() {
        empty_msg
    } else {
        rows
    };

    let widths = [
        Constraint::Length(4),
        Constraint::Length(16),
        Constraint::Length(7),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Min(8),
    ];

    let table = Table::new(display_rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::NONE));

    let mut state = TableState::default();
    if !app.accounts.is_empty() {
        state.select(Some(app.selected_index));
    }

    frame.render_stateful_widget(table, area, &mut state);
}
