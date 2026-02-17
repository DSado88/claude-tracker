use chrono::Utc;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::app::AppState;

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    let mut left_spans = vec![
        Span::styled(" Claude Tracker", Style::default().fg(Color::Cyan)),
    ];

    // Status message (shown next to title)
    if let Some((msg, _)) = &app.status_message {
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(msg.clone(), Style::default().fg(Color::Yellow)));
    }

    let left_line = Line::from(left_spans);

    // Last refresh time (right-aligned)
    if let Some(last) = &app.last_poll {
        let ago = Utc::now().signed_duration_since(*last).num_seconds();
        let ago_text = if ago < 60 {
            format!("{ago}s ago")
        } else {
            format!("{}m ago", ago / 60)
        };
        let right_text = format!("Last refresh: {ago_text} ");
        let right_len = right_text.len() as u16;
        let right_line = Line::from(Span::styled(right_text, Style::default().fg(Color::DarkGray)));

        let chunks = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(right_len),
        ])
        .split(area);

        frame.render_widget(left_line, chunks[0]);
        frame.render_widget(right_line, chunks[1]);
    } else {
        frame.render_widget(left_line, area);
    }
}
