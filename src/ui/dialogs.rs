use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::InputFields;

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

pub fn render_input_dialog(frame: &mut Frame, title: &str, fields: &InputFields) {
    let area = centered_rect(50, 11, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1), // name label
        Constraint::Length(1), // name input
        Constraint::Length(1), // session_key label
        Constraint::Length(1), // session_key input
        Constraint::Length(1), // org_id label
        Constraint::Length(1), // org_id input
        Constraint::Length(1), // spacer
        Constraint::Length(1), // help text
    ])
    .split(inner);

    let labels = ["Name:", "Session Key:", "Org ID:"];
    let values = [&fields.name, &fields.session_key, &fields.org_id];

    for (i, (label, value)) in labels.iter().zip(values.iter()).enumerate() {
        let label_style = Style::default().fg(Color::DarkGray);
        let input_style = if i == fields.focused_field {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };

        let display_value = if i == 1 && !value.is_empty() {
            // Mask session key, show last 8 chars
            let visible = if value.len() > 8 {
                &value[value.len() - 8..]
            } else {
                value.as_str()
            };
            format!("{}...{}", &"*".repeat(8), visible)
        } else {
            value.to_string()
        };

        let cursor = if i == fields.focused_field { "_" } else { "" };

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", label),
                label_style,
            ))),
            chunks[i * 2],
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}{}", display_value, cursor),
                input_style,
            ))),
            chunks[i * 2 + 1],
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Tab: next field  Enter: save  Esc: cancel",
            Style::default().fg(Color::DarkGray),
        ))),
        chunks[7],
    );
}

pub fn render_confirm_dialog(frame: &mut Frame, message: &str, hint: &str) {
    let area = centered_rect(40, 5, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", message),
            Style::default().fg(Color::White),
        ))),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", hint),
            Style::default().fg(Color::DarkGray),
        ))),
        chunks[1],
    );
}

pub fn render_help_overlay(frame: &mut Frame) {
    let area = centered_rect(45, 14, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let help_lines = vec![
        " j/k or Up/Down    Navigate accounts",
        " r                 Refresh all",
        " R                 Refresh selected",
        " s or Enter        Swap to selected",
        " a                 Add account",
        " e                 Edit account",
        " d/x               Delete account",
        " ?                 Toggle help",
        " q / Ctrl+C        Quit",
        "",
        " Press any key to close",
    ];

    let text: Vec<Line> = help_lines
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(Color::Gray),
            ))
        })
        .collect();

    frame.render_widget(Paragraph::new(text), inner);
}
