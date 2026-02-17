use chrono::Utc;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::app::AppState;

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    let mut spans = vec![
        Span::styled(" Claude Tracker", Style::default().fg(Color::Cyan)),
    ];

    // Last refresh time
    if let Some(last) = &app.last_poll {
        let ago = Utc::now().signed_duration_since(*last).num_seconds();
        let ago_text = if ago < 60 {
            format!("{ago}s ago")
        } else {
            format!("{}m ago", ago / 60)
        };
        // Right-align: pad with spaces
        let used = 16 + ago_text.len() + 18; // rough estimate
        let padding = if area.width as usize > used {
            area.width as usize - used
        } else {
            1
        };
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(
            format!("Last refresh: {ago_text}"),
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Status message
    if let Some((msg, _)) = &app.status_message {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(msg.clone(), Style::default().fg(Color::Yellow)));
    }

    frame.render_widget(Line::from(spans), area);
}
