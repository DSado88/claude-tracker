use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" j/k", Style::default().fg(Color::White)),
        Span::styled(": navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled("r", Style::default().fg(Color::White)),
        Span::styled(": refresh  ", Style::default().fg(Color::DarkGray)),
        Span::styled("s", Style::default().fg(Color::White)),
        Span::styled(": set active  ", Style::default().fg(Color::DarkGray)),
        Span::styled("i", Style::default().fg(Color::White)),
        Span::styled(": import  ", Style::default().fg(Color::DarkGray)),
        Span::styled("a", Style::default().fg(Color::White)),
        Span::styled(": add  ", Style::default().fg(Color::DarkGray)),
        Span::styled("d", Style::default().fg(Color::White)),
        Span::styled(": delete  ", Style::default().fg(Color::DarkGray)),
        Span::styled("e", Style::default().fg(Color::White)),
        Span::styled(": edit  ", Style::default().fg(Color::DarkGray)),
        Span::styled("?", Style::default().fg(Color::White)),
        Span::styled(": help  ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::White)),
        Span::styled(": quit", Style::default().fg(Color::DarkGray)),
    ]);

    frame.render_widget(line, area);
}
