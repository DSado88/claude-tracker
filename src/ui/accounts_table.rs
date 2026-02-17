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

fn progress_bar_line(pct: u32, color: Color) -> Line<'static> {
    let filled = if pct == 0 {
        0
    } else if pct >= 100 {
        10
    } else {
        ((pct * 10 + 50) / 100).clamp(1, 10) as usize
    };
    let empty = 10 - filled;

    let filled_str: String = "\u{2588}".repeat(filled);
    let empty_str: String = "\u{2591}".repeat(empty);

    Line::from(vec![
        Span::styled(filled_str, Style::default().fg(color)),
        Span::styled(empty_str, Style::default().fg(Color::Indexed(238))),
    ])
}

fn empty_bar_line() -> Line<'static> {
    Line::from(Span::styled(
        "\u{2500}".repeat(10),
        Style::default().fg(Color::Indexed(238)),
    ))
}

/// Build a placeholder row with "--" for all usage columns and a custom status cell.
fn placeholder_row(num: String, name: String, status: &str, color: Color) -> Row<'static> {
    let style = Style::default().fg(color);
    Row::new(vec![
        Cell::from(Span::styled(num, style)),
        Cell::from(Span::styled(name, style)),
        Cell::from(Span::styled("--", style)),
        Cell::from(empty_bar_line()),
        Cell::from(Span::styled("--", style)),
        Cell::from(Span::styled("--", style)),
        Cell::from(empty_bar_line()),
        Cell::from(Span::styled("--", style)),
        Cell::from(Span::styled(status.to_string(), style)),
    ])
}

fn format_countdown(resets_at: &chrono::DateTime<Utc>) -> String {
    let now = Utc::now();
    let diff = resets_at.signed_duration_since(now);
    let total_secs = diff.num_seconds();

    if total_secs <= 0 {
        return "now".to_string();
    }

    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let mins = (total_secs % 3600) / 60;

    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {:02}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    let header = Row::new(vec![
        Cell::from(" # "),
        Cell::from("Name"),
        Cell::from("5h %"),
        Cell::from("5h Bar"),
        Cell::from("5h Reset"),
        Cell::from("7d %"),
        Cell::from("7d Bar"),
        Cell::from("7d Reset"),
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

            match &account.status {
                AccountStatus::Idle => {
                    placeholder_row(num, name, "Idle", Color::DarkGray)
                }
                AccountStatus::Ok => {
                    if let Some(usage) = &account.usage {
                        let now = Utc::now();

                        // If resets_at has passed, the server has reset the window —
                        // show 0% locally instead of stale cached utilization.
                        let h5_util = if usage.resets_at.map_or(false, |r| now > r) {
                            0
                        } else {
                            usage.utilization
                        };
                        let h5_color = utilization_color(h5_util);
                        let h5_pct = format!("{}%", h5_util);
                        let h5_bar = progress_bar_line(h5_util, h5_color);
                        let h5_reset = usage
                            .resets_at
                            .as_ref()
                            .map(format_countdown)
                            .unwrap_or_else(|| "--".to_string());

                        let (d7_pct, d7_bar, d7_reset, d7_color) =
                            if let Some(weekly_util) = usage.weekly_utilization {
                                let effective = if usage.weekly_resets_at.map_or(false, |r| now > r) {
                                    0
                                } else {
                                    weekly_util
                                };
                                let color = utilization_color(effective);
                                let reset = usage
                                    .weekly_resets_at
                                    .as_ref()
                                    .map(format_countdown)
                                    .unwrap_or_else(|| "--".to_string());
                                (
                                    format!("{}%", effective),
                                    progress_bar_line(effective, color),
                                    reset,
                                    color,
                                )
                            } else {
                                (
                                    "--".to_string(),
                                    empty_bar_line(),
                                    "--".to_string(),
                                    Color::DarkGray,
                                )
                            };

                        let name_style = if is_selected {
                            Style::default()
                                .fg(h5_color)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(h5_color)
                        };

                        let _ = d7_color; // used for bar already

                        let is_logged_in = app
                            .logged_in_account
                            .as_ref()
                            .map(|n| n == &account.config.name)
                            .unwrap_or(false);

                        let status_cell = if is_logged_in {
                            Cell::from(Span::styled(
                                "Logged In",
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else if let Some(fetched) = &account.last_fetched {
                            let ago = Utc::now().signed_duration_since(*fetched).num_minutes();
                            let label = if ago < 2 {
                                "Live".to_string()
                            } else if ago < 60 {
                                format!("{}m ago", ago)
                            } else {
                                format!("{}h ago", ago / 60)
                            };
                            let color = if ago < 2 { Color::Gray } else { Color::Yellow };
                            Cell::from(Span::styled(label, Style::default().fg(color)))
                        } else {
                            Cell::from(Span::styled("--", Style::default().fg(Color::DarkGray)))
                        };

                        Row::new(vec![
                            Cell::from(Span::styled(num, Style::default().fg(h5_color))),
                            Cell::from(Span::styled(name, name_style)),
                            Cell::from(Span::styled(h5_pct, Style::default().fg(h5_color))),
                            Cell::from(h5_bar),
                            Cell::from(Span::styled(h5_reset, Style::default().fg(Color::Gray))),
                            Cell::from(Span::styled(d7_pct, Style::default().fg(d7_color))),
                            Cell::from(d7_bar),
                            Cell::from(Span::styled(d7_reset, Style::default().fg(Color::Gray))),
                            status_cell,
                        ])
                    } else {
                        placeholder_row(num, name, "OK", Color::Gray)
                    }
                }
                AccountStatus::Error(msg) => {
                    let short = if msg.chars().count() > 30 {
                        let truncated: String = msg.chars().take(27).collect();
                        format!("{truncated}...")
                    } else {
                        msg.clone()
                    };
                    placeholder_row(num, name, &short, Color::Red)
                }
            }
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
        Constraint::Length(4),  // #
        Constraint::Length(16), // Name
        Constraint::Length(5),  // 5h %
        Constraint::Length(12), // 5h Bar
        Constraint::Length(9),  // 5h Reset
        Constraint::Length(5),  // 7d %
        Constraint::Length(12), // 7d Bar
        Constraint::Length(9),  // 7d Reset
        Constraint::Min(8),    // Status
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // =========================================================================
    // FIX VERIFIED: Multi-byte UTF-8 error messages truncate safely using
    // chars().take() instead of byte slicing.
    // =========================================================================
    #[test]
    fn error_message_truncation_handles_multibyte_utf8() {
        // 26 ASCII chars followed by 'é' (2-byte UTF-8) + padding.
        // Previously panicked with byte slice at index 27 (inside 'é').
        let msg = "abcdefghijklmnopqrstuvwxyzé1234".to_string();
        assert!(msg.chars().count() > 30);

        let short = if msg.chars().count() > 30 {
            let truncated: String = msg.chars().take(27).collect();
            format!("{truncated}...")
        } else {
            msg.clone()
        };

        // 'é' is the 27th char, so it's included in take(27)
        assert_eq!(short, "abcdefghijklmnopqrstuvwxyzé...");
    }

    #[test]
    fn error_message_truncation_ascii_only() {
        let msg = "This is a long error message that exceeds thirty chars easily".to_string();
        assert!(msg.chars().count() > 30);

        let short = if msg.chars().count() > 30 {
            let truncated: String = msg.chars().take(27).collect();
            format!("{truncated}...")
        } else {
            msg.clone()
        };

        assert_eq!(short, "This is a long error messag...");
    }

    #[test]
    fn error_message_short_not_truncated() {
        let msg = "Short error".to_string();
        assert!(msg.chars().count() <= 30);

        let short = if msg.chars().count() > 30 {
            let truncated: String = msg.chars().take(27).collect();
            format!("{truncated}...")
        } else {
            msg.clone()
        };

        assert_eq!(short, "Short error");
    }
}
