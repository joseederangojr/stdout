use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

pub enum View<'a> {
    SignIn {
        authorization_url: &'a str,
        fingerprint: &'a str,
        callback_input: &'a str,
        message: Option<&'a str>,
    },
    SignedIn,
}

pub fn render(frame: &mut Frame<'_>, view: &View<'_>) {
    match view {
        View::SignIn {
            authorization_url,
            fingerprint,
            callback_input,
            message,
        } => render_sign_in(
            frame,
            authorization_url,
            fingerprint,
            callback_input,
            *message,
        ),
        View::SignedIn => render_dividers(frame),
    }
}

fn render_sign_in(
    frame: &mut Frame<'_>,
    authorization_url: &str,
    fingerprint: &str,
    callback_input: &str,
    message: Option<&str>,
) {
    let [title, intro, identity, url, help, input, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new("stdout · sign in").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        title,
    );
    frame.render_widget(
        Paragraph::new("Approve this trusted device in your dev-null.sh browser session."),
        intro,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Device key: ", Style::default().fg(Color::DarkGray)),
            Span::raw(fingerprint),
        ])),
        identity,
    );
    frame.render_widget(
        Paragraph::new(authorization_url).wrap(Wrap { trim: true }),
        url,
    );
    frame.render_widget(
        Paragraph::new("o open browser · paste callback URI + Enter · r restart · Esc quit")
            .style(Style::default().fg(Color::DarkGray)),
        help,
    );
    let input_status = if callback_input.is_empty() {
        "paste full callback URI here"
    } else {
        "callback URI ready · press Enter"
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("callback > ", Style::default().fg(Color::Cyan)),
            Span::raw(input_status),
        ])),
        input,
    );
    if let Some(message) = message {
        frame.render_widget(
            Paragraph::new(message).style(Style::default().fg(Color::Yellow)),
            status,
        );
    }
}

fn render_dividers(frame: &mut Frame<'_>) {
    let area = frame.area();
    if area.width < 3 {
        return;
    }

    let [navigation, feed, _details] = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Percentage(60),
        Constraint::Percentage(20),
    ])
    .areas(area);

    render_divider(frame, navigation.x + navigation.width, area);
    render_divider(frame, feed.x + feed.width, area);
}

fn render_divider(frame: &mut Frame<'_>, x: u16, area: Rect) {
    let lines = vec![Line::from("│"); area.height as usize];
    frame.render_widget(Paragraph::new(lines), Rect::new(x, area.y, 1, area.height));
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, Terminal};

    use super::{render, View};

    #[test]
    fn signed_in_view_renders_only_the_two_dividers() {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, &View::SignedIn))
            .unwrap();

        let expected: Vec<String> = (0..24)
            .map(|_| format!("{}│{}│{}", " ".repeat(20), " ".repeat(59), " ".repeat(19)))
            .collect();
        terminal.backend().assert_buffer_lines(expected);
    }
}
