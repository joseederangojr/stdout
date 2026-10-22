use crate::app::{Mode, Step};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

const ONBOARDING_WIDTH: u16 = 70;
const ONBOARDING_HEIGHT: u16 = 12;

pub enum View<'a> {
    SignIn {
        active_step: Step,
        mode: Mode,
        name_device_complete: bool,
        browser_approval_complete: bool,
        device_name_input: &'a str,
        device_name_suggestion: &'a str,
        authorization_url: &'a str,
        fingerprint: &'a str,
        callback_input: &'a str,
        message: Option<&'a str>,
    },
    SignedIn,
}

#[derive(Clone, Copy)]
struct SignInData<'a> {
    active_step: Step,
    mode: Mode,
    name_device_complete: bool,
    browser_approval_complete: bool,
    device_name_input: &'a str,
    device_name_suggestion: &'a str,
    authorization_url: &'a str,
    fingerprint: &'a str,
    callback_input: &'a str,
    message: Option<&'a str>,
}

pub fn render(frame: &mut Frame<'_>, view: &View<'_>) {
    match view {
        View::SignIn {
            active_step,
            mode,
            name_device_complete,
            browser_approval_complete,
            device_name_input,
            device_name_suggestion,
            authorization_url,
            fingerprint,
            callback_input,
            message,
        } => render_sign_in(
            frame,
            SignInData {
                active_step: *active_step,
                mode: *mode,
                name_device_complete: *name_device_complete,
                browser_approval_complete: *browser_approval_complete,
                device_name_input,
                device_name_suggestion,
                authorization_url,
                fingerprint,
                callback_input,
                message: *message,
            },
        ),
        View::SignedIn => render_dividers(frame),
    }
}

fn render_sign_in(frame: &mut Frame<'_>, sign_in: SignInData<'_>) {
    let [content_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());

    let onboarding_area = centered_rect(
        content_area,
        content_area.width.min(ONBOARDING_WIDTH),
        content_area.height.min(ONBOARDING_HEIGHT),
    );

    let [title_area, body_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(onboarding_area);

    frame.render_widget(
        Paragraph::new("stdout · sign in")
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        title_area,
    );

    let [steps_area, details_area] =
        Layout::horizontal([Constraint::Length(30), Constraint::Min(10)]).areas(body_area);

    render_stepper(
        frame,
        steps_area,
        sign_in.active_step,
        sign_in.name_device_complete,
        sign_in.browser_approval_complete,
    );
    render_step_details(frame, details_area, sign_in);
    render_footer(frame, footer_area, sign_in.mode, sign_in.active_step);
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[derive(Clone, Copy)]
struct StepHitTestLayout {
    stepper_area: Rect,
    name_row: u16,
    browser_row: u16,
    finish_row: u16,
}

pub(crate) fn step_at_mouse_position(
    width: u16,
    height: u16,
    column: u16,
    row: u16,
) -> Option<Step> {
    let layout = step_hit_test_layout(Rect::new(0, 0, width, height));
    if !rect_contains(layout.stepper_area, column, row) {
        return None;
    }

    if row == layout.name_row {
        Some(Step::NameDevice)
    } else if row == layout.browser_row {
        Some(Step::BrowserApproval)
    } else if row == layout.finish_row {
        Some(Step::Finish)
    } else {
        None
    }
}

fn step_hit_test_layout(area: Rect) -> StepHitTestLayout {
    let [content_area, _] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);

    let onboarding_area = centered_rect(
        content_area,
        content_area.width.min(ONBOARDING_WIDTH),
        content_area.height.min(ONBOARDING_HEIGHT),
    );

    let [_title_area, body_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(onboarding_area);
    let [steps_area, _details_area] =
        Layout::horizontal([Constraint::Length(30), Constraint::Min(10)]).areas(body_area);
    let [_rail_area, labels_area] =
        Layout::horizontal([Constraint::Length(2), Constraint::Min(8)]).areas(steps_area);
    let [label_lines_area, _] =
        Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(labels_area);

    StepHitTestLayout {
        stepper_area: steps_area,
        name_row: label_lines_area.y,
        browser_row: label_lines_area.y.saturating_add(2),
        finish_row: label_lines_area.y.saturating_add(4),
    }
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    let max_x = area.x.saturating_add(area.width);
    let max_y = area.y.saturating_add(area.height);
    column >= area.x && column < max_x && row >= area.y && row < max_y
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StepState {
    Complete,
    Active,
    Pending,
}

fn render_stepper(
    frame: &mut Frame<'_>,
    area: Rect,
    active_step: Step,
    name_device_complete: bool,
    browser_approval_complete: bool,
) {
    let [rail_area, labels_area] =
        Layout::horizontal([Constraint::Length(2), Constraint::Min(8)]).areas(area);
    let [rail_lines_area, _] =
        Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(rail_area);
    let [label_lines_area, _] =
        Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(labels_area);

    let name_state = step_state(active_step, Step::NameDevice, name_device_complete);
    let browser_state = step_state(
        active_step,
        Step::BrowserApproval,
        browser_approval_complete,
    );
    let finish_state = step_state(active_step, Step::Finish, false);

    let name_segment_state = if name_device_complete {
        StepState::Complete
    } else {
        StepState::Pending
    };
    let browser_segment_state = if browser_approval_complete {
        StepState::Complete
    } else {
        StepState::Pending
    };

    let rail_lines = vec![
        Line::styled(node_symbol(name_state), step_style(name_state)),
        Line::styled(
            segment_symbol(name_device_complete),
            step_style(name_segment_state),
        ),
        Line::styled(node_symbol(browser_state), step_style(browser_state)),
        Line::styled(
            segment_symbol(browser_approval_complete),
            step_style(browser_segment_state),
        ),
        Line::styled(node_symbol(finish_state), step_style(finish_state)),
    ];

    frame.render_widget(Paragraph::new(rail_lines), rail_lines_area);

    let label_lines = vec![
        step_label_line("Name device", name_state),
        Line::raw(""),
        step_label_line("Browser approval", browser_state),
        Line::raw(""),
        step_label_line("Finish", finish_state),
    ];

    frame.render_widget(Paragraph::new(label_lines), label_lines_area);
}

fn render_step_details(frame: &mut Frame<'_>, area: Rect, sign_in: SignInData<'_>) {
    match sign_in.active_step {
        Step::NameDevice => {
            let input = if sign_in.device_name_input.is_empty() {
                sign_in.device_name_suggestion
            } else {
                sign_in.device_name_input
            };
            let suffix = if sign_in.mode == Mode::Insert {
                "█"
            } else {
                ""
            };
            let mut lines = vec![
                Line::raw("Set a name for this trusted device."),
                Line::from(vec![
                    Span::styled(
                        "name > ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("{input}{suffix}")),
                ]),
            ];
            if let Some(status) = sign_in.message {
                lines.push(Line::raw(format!("status: {status}")));
            }
            lines.push(Line::from(vec![
                Span::styled("device key: ", Style::default().fg(Color::DarkGray)),
                Span::raw(sign_in.fingerprint),
            ]));

            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        }
        Step::BrowserApproval => {
            let status = sign_in.message.unwrap_or("Awaiting browser approval.");
            let lines = vec![
                Line::raw("Approve this device in your browser session."),
                Line::raw(format!("url: {}", sign_in.authorization_url)),
                Line::raw(format!("status: {status}")),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        }
        Step::Finish => {
            let suffix = if sign_in.mode == Mode::Insert {
                "█"
            } else {
                ""
            };
            let callback_status = if sign_in.callback_input.is_empty() {
                "paste full devnull:// callback URI"
            } else {
                sign_in.callback_input
            };
            let mut lines = vec![
                Line::raw("Paste callback URI to finish sign-in."),
                Line::from(vec![
                    Span::styled(
                        "callback > ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("{callback_status}{suffix}")),
                ]),
            ];
            if let Some(status) = sign_in.message {
                lines.push(Line::raw(format!("status: {status}")));
            }
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        }
    }
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, mode: Mode, active_step: Step) {
    let mode_label = match mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Visual => "VISUAL",
    };

    let hints = match (mode, active_step) {
        (Mode::Normal, Step::NameDevice) => "j/k step · i edit · Enter continue · q quit",
        (Mode::Normal, Step::BrowserApproval) => {
            "j/k step · o open · y copy · r restart · Enter continue · q quit"
        }
        (Mode::Normal, Step::Finish) => "j/k step · i edit · Enter submit · q quit",
        (Mode::Insert, Step::NameDevice) => {
            "type name · Backspace delete · Enter continue · Esc normal"
        }
        (Mode::Insert, Step::BrowserApproval) => "Enter continue · Esc normal",
        (Mode::Insert, Step::Finish) => {
            "paste callback · Backspace delete · Enter submit · Esc normal"
        }
        (Mode::Visual, _) => "Esc normal",
    };

    frame.render_widget(
        Paragraph::new(format!("{mode_label} · {hints}"))
            .style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn step_label_line(label: &str, state: StepState) -> Line<'static> {
    let marker = match state {
        StepState::Complete => "✓",
        StepState::Active => ">",
        StepState::Pending => " ",
    };

    Line::styled(
        format!("[{marker}] {label}"),
        step_style(state).add_modifier(if state == StepState::Active {
            Modifier::BOLD
        } else {
            Modifier::empty()
        }),
    )
}

fn node_symbol(state: StepState) -> &'static str {
    match state {
        StepState::Complete => "●",
        StepState::Active => "◆",
        StepState::Pending => "○",
    }
}

fn segment_symbol(complete: bool) -> &'static str {
    if complete {
        "│"
    } else {
        "┆"
    }
}

fn step_state(active_step: Step, step: Step, is_complete: bool) -> StepState {
    if step == active_step {
        StepState::Active
    } else if is_complete {
        StepState::Complete
    } else {
        StepState::Pending
    }
}

fn step_style(state: StepState) -> Style {
    match state {
        StepState::Complete => Style::default().fg(Color::Green),
        StepState::Active => Style::default().fg(Color::Cyan),
        StepState::Pending => Style::default().fg(Color::DarkGray),
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
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    use super::{render, step_at_mouse_position, View};
    use crate::app::{Mode, Step};

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

    #[test]
    fn sign_in_group_is_centered_with_left_rail_and_no_borders() {
        let lines =
            render_sign_in_lines(Step::NameDevice, Mode::Insert, false, false, "", "", None);

        let (x, y) =
            find_position(&lines, "[>] Name device").expect("name step label should exist");
        assert!(x > 5);
        assert!(y > 3 && y < 12);
        assert!(lines
            .iter()
            .any(|line| line.contains("[ ] Browser approval")));
        assert!(lines.iter().any(|line| line.contains("[ ] Finish")));
        assert!(lines.iter().all(|line| !line.contains('┌')));
        assert!(lines.iter().all(|line| !line.contains('┐')));
        assert!(lines.iter().all(|line| !line.contains('└')));
        assert!(lines.iter().all(|line| !line.contains('┘')));
    }

    #[test]
    fn name_device_step_renders_focused_input_and_insert_footer() {
        let lines =
            render_sign_in_lines(Step::NameDevice, Mode::Insert, false, false, "", "", None);

        assert!(lines
            .iter()
            .any(|line| line.contains("name > stdout-laptop█")));
        assert!(!lines
            .iter()
            .any(|line| line.contains("status: Press Enter")));
        assert!(lines[23].contains("INSERT"));
        assert!(lines[23].contains("Enter continue"));
        assert!(!lines[23].contains("o open"));
    }

    #[test]
    fn browser_step_renders_url_status_and_contextual_footer_hints() {
        let lines = render_sign_in_lines(
            Step::BrowserApproval,
            Mode::Normal,
            true,
            false,
            "work-laptop",
            "",
            Some("Awaiting browser callback."),
        );

        assert!(lines.iter().any(|line| line.contains("url:")));
        assert!(lines
            .iter()
            .any(|line| line.contains("https://dev-null.sh")));
        assert!(lines.iter().any(|line| line.contains("code=")));
        assert!(lines
            .iter()
            .any(|line| line.contains("status: Awaiting browser callback.")));
        assert!(!lines
            .iter()
            .any(|line| line.contains("hints: y copy URL · o open browser")));
        assert!(!lines.iter().any(|line| line.contains("callback >")));
        assert!(lines[23].contains("NORMAL"));
        assert!(lines[23].contains("o open"));
        assert!(lines[23].contains("y copy"));
    }

    #[test]
    fn finish_step_renders_callback_input_and_submit_footer() {
        let lines = render_sign_in_lines(
            Step::Finish,
            Mode::Insert,
            true,
            false,
            "work-laptop",
            "",
            None,
        );

        assert!(lines
            .iter()
            .any(|line| line.contains("callback > paste full devnull://")));
        assert!(lines.iter().any(|line| line.contains("callback URI█")));
        assert!(!lines
            .iter()
            .any(|line| line.contains("status: Press Enter")));
        assert!(lines[23].contains("INSERT"));
        assert!(lines[23].contains("Enter submit"));
        assert!(!lines[23].contains("y copy"));
    }

    #[test]
    fn navigation_alone_keeps_rail_dotted() {
        let lines = render_sign_in_lines(Step::Finish, Mode::Normal, false, false, "", "", None);

        assert_eq!(count_char(&lines, '│'), 0);
        assert_eq!(count_char(&lines, '┆'), 2);
    }

    #[test]
    fn completed_steps_make_segments_solid() {
        let name_complete_lines = render_sign_in_lines(
            Step::BrowserApproval,
            Mode::Normal,
            true,
            false,
            "",
            "",
            None,
        );
        assert_eq!(count_char(&name_complete_lines, '│'), 1);
        assert_eq!(count_char(&name_complete_lines, '┆'), 1);

        let browser_complete_lines =
            render_sign_in_lines(Step::Finish, Mode::Normal, true, true, "", "", None);
        assert_eq!(count_char(&browser_complete_lines, '│'), 2);
        assert_eq!(count_char(&browser_complete_lines, '┆'), 0);
    }

    #[test]
    fn visual_mode_footer_shows_only_visual_hint() {
        let lines = render_sign_in_lines(
            Step::BrowserApproval,
            Mode::Visual,
            true,
            false,
            "",
            "",
            None,
        );

        assert!(lines[23].contains("VISUAL"));
        assert!(lines[23].contains("Esc normal"));
        assert!(!lines[23].contains("j/k"));
        assert!(!lines[23].contains("o open"));
    }

    #[test]
    fn step_hit_test_matches_centered_rows() {
        assert_eq!(
            step_at_mouse_position(80, 24, 10, 6),
            Some(Step::NameDevice)
        );
        assert_eq!(
            step_at_mouse_position(80, 24, 10, 8),
            Some(Step::BrowserApproval)
        );
        assert_eq!(step_at_mouse_position(80, 24, 10, 10), Some(Step::Finish));
    }

    #[test]
    fn step_hit_test_ignores_outside_stepper_area() {
        assert_eq!(step_at_mouse_position(80, 24, 1, 6), None);
        assert_eq!(step_at_mouse_position(80, 24, 10, 7), None);
    }

    #[test]
    fn step_hit_test_tracks_resized_terminal_centering() {
        assert_eq!(
            step_at_mouse_position(100, 30, 20, 11),
            Some(Step::BrowserApproval)
        );
        assert_eq!(step_at_mouse_position(100, 30, 10, 11), None);
    }

    fn render_sign_in_lines(
        active_step: Step,
        mode: Mode,
        name_device_complete: bool,
        browser_approval_complete: bool,
        device_name_input: &str,
        callback_input: &str,
        message: Option<&str>,
    ) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let view = View::SignIn {
            active_step,
            mode,
            name_device_complete,
            browser_approval_complete,
            device_name_input,
            device_name_suggestion: "stdout-laptop",
            authorization_url: "https://dev-null.sh/auth/authorize?code=abc123",
            fingerprint: "d7:c2:aa:19",
            callback_input,
            message,
        };
        terminal.draw(|frame| render(frame, &view)).unwrap();

        buffer_lines(terminal.backend().buffer())
    }

    fn buffer_lines(buffer: &Buffer) -> Vec<String> {
        let width = buffer.area.width;
        let height = buffer.area.height;

        (0..height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..width {
                    line.push_str(buffer[(x, y)].symbol());
                }
                line
            })
            .collect()
    }

    fn find_position(lines: &[String], needle: &str) -> Option<(usize, usize)> {
        lines
            .iter()
            .enumerate()
            .find_map(|(y, line)| line.find(needle).map(|x| (x, y)))
    }

    fn count_char(lines: &[String], target: char) -> usize {
        lines
            .iter()
            .map(|line| {
                line.chars()
                    .filter(|character| *character == target)
                    .count()
            })
            .sum()
    }
}
