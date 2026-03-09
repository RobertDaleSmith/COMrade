use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{App, ConnStatus};
use super::line_assembler::LineKind;
use comrade_protocol::{DataBits, Parity, StopBits};

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // Log pane
            Constraint::Length(1), // Status bar
            Constraint::Length(3), // Input bar
        ])
        .split(f.area());

    draw_log(f, app, chunks[0]);
    draw_status_bar(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);
}

fn draw_log(f: &mut Frame, app: &App, area: Rect) {
    let inner_height = area.height.saturating_sub(2) as usize; // borders take 2 rows
    let total = app.lines.len();

    // Calculate visible range.
    let end = total.saturating_sub(app.scroll_offset);
    let start = end.saturating_sub(inner_height);

    let lines: Vec<Line> = app.lines[start..end]
        .iter()
        .map(|line| {
            let ts = line.timestamp.format("%H:%M:%S%.3f").to_string();
            let (ts_style, text_style) = match line.kind {
                LineKind::Received => (
                    Style::default().fg(Color::DarkGray),
                    Style::default().fg(Color::White),
                ),
                LineKind::Sent => (
                    Style::default().fg(Color::DarkGray),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                LineKind::System => (
                    Style::default().fg(Color::DarkGray),
                    Style::default().fg(Color::Yellow),
                ),
            };
            Line::from(vec![
                Span::styled(ts, ts_style),
                Span::raw("  "),
                Span::styled(&line.text, text_style),
            ])
        })
        .collect();

    let title = if app.scroll_offset > 0 {
        format!(" COMrade [+{}] ", app.scroll_offset)
    } else {
        " COMrade ".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Blue));

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let (port_text, config_text, status_text, status_color) = match &app.status {
        ConnStatus::Connecting => (
            app.port_path.clone(),
            String::new(),
            "CONNECTING".to_string(),
            Color::Yellow,
        ),
        ConnStatus::Connected { port, config } => (
            port.clone(),
            format!(
                " {} {}{}{}",
                config.baud_rate,
                data_bits_char(&config.data_bits),
                parity_char(&config.parity),
                stop_bits_char(&config.stop_bits),
            ),
            "CONNECTED".to_string(),
            Color::Green,
        ),
        ConnStatus::Disconnected { reason } => (
            app.port_path.clone(),
            String::new(),
            format!("DISCONNECTED: {reason}"),
            Color::Red,
        ),
    };

    let rx_text = format!("RX: {} bytes", format_bytes(app.rx_bytes));

    let line = Line::from(vec![
        Span::styled(" ", Style::default().bg(Color::DarkGray)),
        Span::styled(
            port_text,
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            config_text,
            Style::default().bg(Color::DarkGray).fg(Color::White),
        ),
        Span::styled(
            " | ",
            Style::default().bg(Color::DarkGray).fg(Color::Gray),
        ),
        Span::styled(
            status_text,
            Style::default()
                .bg(Color::DarkGray)
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " | ",
            Style::default().bg(Color::DarkGray).fg(Color::Gray),
        ),
        Span::styled(
            rx_text,
            Style::default().bg(Color::DarkGray).fg(Color::White),
        ),
        // Fill rest with background.
        Span::styled(
            " ".repeat(area.width as usize),
            Style::default().bg(Color::DarkGray),
        ),
    ]);

    let paragraph = Paragraph::new(line);
    f.render_widget(paragraph, area);
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let text = app.input.text();

    let input_line = Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Green)),
        Span::raw(&text),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let paragraph = Paragraph::new(input_line).block(block);
    f.render_widget(paragraph, area);

    // Place the cursor.
    if app.cursor_visible {
        // +1 for border, +2 for "> " prompt
        let cursor_x = area.x + 1 + 2 + app.input.cursor() as u16;
        let cursor_y = area.y + 1; // +1 for border
        f.set_cursor_position((cursor_x, cursor_y));
    }
}

fn data_bits_char(db: &DataBits) -> &'static str {
    match db {
        DataBits::Five => "5",
        DataBits::Six => "6",
        DataBits::Seven => "7",
        DataBits::Eight => "8",
    }
}

fn parity_char(p: &Parity) -> &'static str {
    match p {
        Parity::None => "N",
        Parity::Odd => "O",
        Parity::Even => "E",
    }
}

fn stop_bits_char(sb: &StopBits) -> &'static str {
    match sb {
        StopBits::One => "1",
        StopBits::Two => "2",
    }
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n}")
    } else if n < 1024 * 1024 {
        format!("{:.1}K", n as f64 / 1024.0)
    } else {
        format!("{:.1}M", n as f64 / (1024.0 * 1024.0))
    }
}
