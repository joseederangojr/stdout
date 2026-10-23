use crate::{
    app::{ComposeKind, LogInteraction, Mode, ReportReason, Step},
    navigation::{Keymap, LogTab, Page},
};
use std::ops::Range;

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

const ONBOARDING_WIDTH: u16 = 70;
const ONBOARDING_HEIGHT: u16 = 12;
const COLUMN_PADDING: u16 = 3;
const TOP_PADDING: u16 = 3;
const CARD_SIDE_PADDING: u16 = 2;
const CARD_HINTS: &str = "j/k select · s star · p pastel · r reply · q quote · y copy · R report · o open · Enter detail · 1-3 tabs";

/// Presentation-only data; callers supply real feed entries when available.
pub(crate) struct LogCommit<'a> {
    pub(crate) author: &'a str,
    pub(crate) handle: &'a str,
    pub(crate) time: &'a str,
    pub(crate) pending: bool,
    pub(crate) hash: &'a str,
    pub(crate) body: &'a str,
    pub(crate) thread: Option<&'a str>,
}

enum LogFeed<'a> {
    Legacy(&'a [String]),
    Cards(&'a [LogCommit<'a>], Option<usize>),
}

pub(crate) enum View<'a> {
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
    SignedIn {
        current_page: Page,
        keymap: &'a Keymap,
        log_tab: LogTab,
        log_scroll: u16,
        feed_entries: &'a [String],
    },
    SignedInCards {
        current_page: Page,
        keymap: &'a Keymap,
        log_tab: LogTab,
        log_scroll: u16,
        commits: &'a [LogCommit<'a>],
        focused_commit: Option<usize>,
    },
    SignedInInteraction {
        current_page: Page,
        keymap: &'a Keymap,
        log_tab: LogTab,
        log_scroll: u16,
        commits: &'a [LogCommit<'a>],
        focused_commit: Option<usize>,
        interaction: &'a LogInteraction,
    },
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
        View::SignedIn {
            current_page,
            keymap,
            log_tab,
            log_scroll,
            feed_entries,
        } => render_signed_in(
            frame,
            *current_page,
            keymap,
            *log_tab,
            *log_scroll,
            LogFeed::Legacy(feed_entries),
            false,
        ),
        View::SignedInCards {
            current_page,
            keymap,
            log_tab,
            log_scroll,
            commits,
            focused_commit,
        } => render_signed_in(
            frame,
            *current_page,
            keymap,
            *log_tab,
            *log_scroll,
            LogFeed::Cards(commits, *focused_commit),
            *current_page == Page::Log && focused_commit.is_some_and(|index| index < commits.len()),
        ),
        View::SignedInInteraction {
            current_page,
            keymap,
            log_tab,
            log_scroll,
            commits,
            focused_commit,
            interaction,
        } => {
            render_signed_in(
                frame,
                *current_page,
                keymap,
                *log_tab,
                *log_scroll,
                LogFeed::Cards(commits, *focused_commit),
                false,
            );
            if *current_page == Page::Log {
                render_log_interaction(frame, interaction);
            }
        }
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

struct SignedInLayout {
    sidebar: Rect,
    brand: Rect,
    tagline: Rect,
    tabs: Rect,
    feed: Rect,
    footer: Rect,
}

fn signed_in_layout(area: Rect) -> Option<SignedInLayout> {
    if area.width < 3 {
        return None;
    }
    let footer_height = area.height.min(2);
    let footer = Rect::new(
        area.x,
        area.bottom().saturating_sub(footer_height),
        area.width,
        footer_height,
    );
    let content_area = Rect::new(area.x, area.y, area.width, area.height - footer_height);
    let [_left_gutter, navigation, feed, _details] = Layout::horizontal([
        Constraint::Percentage(15),
        Constraint::Percentage(15),
        Constraint::Percentage(40),
        Constraint::Percentage(30),
    ])
    .areas(content_area);
    let content = Rect::new(
        feed.x.saturating_add(COLUMN_PADDING),
        feed.y.saturating_add(TOP_PADDING),
        feed.width.saturating_sub(COLUMN_PADDING * 2),
        feed.height.saturating_sub(TOP_PADDING),
    );
    let [brand, tagline, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(content);
    let tabs_height = if body.width < 39 {
        LogTab::ALL.len() as u16
    } else {
        1
    };
    let [_gap, tabs, _tab_gap, feed] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(tabs_height),
        Constraint::Length(2),
        Constraint::Min(0),
    ])
    .areas(body);
    let sidebar_offset = tabs.bottom().saturating_sub(navigation.y);
    let navigation_padding = if navigation.width < 18 {
        1
    } else {
        COLUMN_PADDING
    };
    let sidebar = Rect::new(
        navigation.x.saturating_add(navigation_padding),
        navigation.y.saturating_add(sidebar_offset),
        navigation.width.saturating_sub(navigation_padding * 2),
        navigation.height.saturating_sub(sidebar_offset),
    );
    Some(SignedInLayout {
        sidebar,
        brand,
        tagline,
        tabs,
        feed,
        footer,
    })
}

/// Geometry in screen cells; row ranges include component padding and form two-row gaps.
pub(crate) struct CardFeedLayout {
    pub(crate) viewport: Rect,
    pub(crate) rows: Vec<Range<usize>>,
    footer: Rect,
}

fn card_content_width(width: u16) -> u16 {
    width.saturating_sub(CARD_SIDE_PADDING * 2)
}

impl CardFeedLayout {
    fn new(area: Rect, commits: &[LogCommit<'_>], focused: Option<usize>) -> Self {
        let status_reserved = area.height > 1;
        let viewport = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(u16::from(status_reserved)),
        );
        let footer = Rect::new(
            area.x,
            viewport.bottom(),
            area.width,
            area.bottom().saturating_sub(viewport.bottom()),
        );
        let mut start = 0;
        let rows = commits
            .iter()
            .enumerate()
            .map(|(index, commit)| {
                let end = start
                    + card_lines(
                        commit,
                        focused == Some(index),
                        card_content_width(area.width),
                    )
                    .len();
                let row = start..end;
                // The previous card's bottom pad and this card's top pad form the two-row gap.
                start = end;
                row
            })
            .collect();
        Self {
            viewport,
            rows,
            footer,
        }
    }

    pub(crate) fn status_row(&self, empty: bool) -> u16 {
        if empty {
            self.viewport.y
        } else if self.footer.height > 0 {
            self.footer.bottom() - 1
        } else {
            self.viewport.bottom().saturating_sub(1)
        }
    }

    pub(crate) fn contains(&self, column: u16, row: u16) -> bool {
        rect_contains(self.viewport, column, row)
    }

    pub(crate) fn clamp_scroll(&self, scroll: u16) -> u16 {
        let end = self.rows.last().map_or(0, |row| row.end);
        (scroll as usize)
            .min(end.saturating_sub(self.viewport.height as usize))
            .min(u16::MAX as usize) as u16
    }

    pub(crate) fn scroll_to_show(&self, scroll: u16, focused: usize) -> u16 {
        let scroll = self.clamp_scroll(scroll) as usize;
        let Some(row) = self.rows.get(focused) else {
            return scroll as u16;
        };
        let height = self.viewport.height as usize;
        if height == 0 {
            return scroll as u16;
        }
        let target = if row.start < scroll || row.len() > height {
            row.start
        } else if row.end > scroll + height {
            row.end - height
        } else {
            scroll
        };
        self.clamp_scroll(target.min(u16::MAX as usize) as u16)
    }

    pub(crate) fn card_at(&self, column: u16, row: u16, scroll: u16) -> Option<usize> {
        if !self.contains(column, row) {
            return None;
        }
        let offset = self.clamp_scroll(scroll) as usize + (row - self.viewport.y) as usize;
        self.rows.iter().position(|range| range.contains(&offset))
    }

    pub(crate) fn link_at<'a>(
        &self,
        column: u16,
        row: u16,
        scroll: u16,
        commits: &[LogCommit<'a>],
    ) -> Option<&'a str> {
        let index = self.card_at(column, row, scroll)?;
        let commit = commits.get(index)?;
        let offset = self.clamp_scroll(scroll) as usize + (row - self.viewport.y) as usize;
        let card_width = card_content_width(self.viewport.width);
        let body_start = card_body_start(commit, card_width, self.rows[index].start);
        let body_row = offset.checked_sub(body_start)?;
        let line = commit
            .body
            .split('\n')
            .flat_map(|text| styled_body_lines(text, card_width.saturating_sub(2) as usize))
            .nth(body_row)?;
        let component_x = self.viewport.x.saturating_add(CARD_SIDE_PADDING);
        let component_right = self.viewport.right().saturating_sub(CARD_SIDE_PADDING);
        if column < component_x || column >= component_right {
            return None;
        }
        let cell = (column - component_x) as usize;
        line.links
            .into_iter()
            .find(|(range, _)| range.contains(&cell))
            .map(|(_, url)| url)
    }
}

/// Shared renderer/input geometry; no synthetic feed entries required.
pub(crate) fn card_feed_layout(
    width: u16,
    height: u16,
    commits: &[LogCommit<'_>],
    focused: Option<usize>,
) -> Option<CardFeedLayout> {
    signed_in_layout(Rect::new(0, 0, width, height))
        .map(|layout| CardFeedLayout::new(layout.feed, commits, focused))
}

fn render_signed_in(
    frame: &mut Frame<'_>,
    current_page: Page,
    keymap: &Keymap,
    log_tab: LogTab,
    log_scroll: u16,
    feed_entries: LogFeed<'_>,
    show_hotkeys: bool,
) {
    let Some(layout) = signed_in_layout(frame.area()) else {
        return;
    };
    render_sidebar(frame, layout.sidebar, current_page, keymap);
    if layout.brand.width == 0 || layout.brand.height == 0 {
        return;
    }
    let brand_white = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let brand_orange = Style::default()
        .fg(Color::Rgb(255, 165, 0))
        .add_modifier(Modifier::BOLD);
    let brand = Line::from(vec![
        Span::styled("/", brand_white),
        Span::styled("/", brand_orange),
        Span::raw(" "),
        Span::styled("/", brand_white),
        Span::styled("dev", brand_white),
        Span::styled("/", brand_orange),
        Span::styled("null", brand_white),
    ]);
    frame.render_widget(Paragraph::new(brand), layout.brand);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "A social network for devs",
            Style::default().fg(Color::DarkGray),
        ))),
        layout.tagline,
    );

    if current_page == Page::Log {
        render_log(
            frame,
            layout.tabs,
            layout.feed,
            log_tab,
            log_scroll,
            feed_entries,
        );
    }
    render_hotkey_footer(frame, layout.footer, show_hotkeys);
}

fn render_hotkey_footer(frame: &mut Frame<'_>, area: Rect, visible: bool) {
    if !visible || area.width == 0 || area.height == 0 {
        return;
    }
    let mut lines = wrap_card_hints(area.width as usize);
    if lines.len() > area.height as usize {
        lines.truncate(area.height as usize);
        if let Some(last) = lines.last_mut() {
            while Line::from(format!("{last}…")).width() > area.width as usize {
                last.pop();
            }
            last.push('…');
        }
    }
    frame.render_widget(
        Paragraph::new(lines.join("\n"))
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_log(
    frame: &mut Frame<'_>,
    tabs_area: Rect,
    feed_area: Rect,
    active_tab: LogTab,
    scroll: u16,
    entries: LogFeed<'_>,
) {
    if tabs_area.width == 0 {
        return;
    }

    let compact = tabs_area.width < 39;
    if compact {
        for (index, tab) in LogTab::ALL
            .into_iter()
            .enumerate()
            .take(tabs_area.height as usize)
        {
            render_log_tab(
                frame,
                Rect::new(tabs_area.x, tabs_area.y + index as u16, tabs_area.width, 1),
                tab,
                active_tab,
            );
        }
    } else {
        let tab_columns: [Rect; 3] = Layout::horizontal([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .areas(tabs_area);
        for (tab, tab_area) in LogTab::ALL.into_iter().zip(tab_columns) {
            render_log_tab(frame, tab_area, tab, active_tab);
        }
    }

    match entries {
        LogFeed::Legacy(entries) => {
            let mut lines = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 {
                    lines.push(Line::from(""));
                }
                lines.extend(entry.lines().map(Line::from));
            }
            let max_scroll = lines.len().saturating_sub(feed_area.height as usize);
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((scroll.min(max_scroll.min(u16::MAX as usize) as u16), 0)),
                feed_area,
            );
        }
        LogFeed::Cards(commits, focused) => {
            render_log_cards(frame, feed_area, scroll, commits, focused)
        }
    }
}

fn render_log_cards(
    frame: &mut Frame<'_>,
    area: Rect,
    scroll: u16,
    commits: &[LogCommit<'_>],
    focused: Option<usize>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let focused = focused.filter(|&index| index < commits.len());
    let layout = CardFeedLayout::new(area, commits, focused);

    let scroll = layout.clamp_scroll(scroll) as usize;
    let bottom = scroll + layout.viewport.height as usize;
    for (index, (commit, row)) in commits.iter().zip(&layout.rows).enumerate() {
        let start = row.start;
        let end = row.end;
        let selected = focused == Some(index);
        if end > scroll && start < bottom {
            let visible_start = start.max(scroll);
            let visible_end = end.min(bottom);
            let horizontal_padding = CARD_SIDE_PADDING.min(layout.viewport.width / 2);
            let visible_y = layout.viewport.y + (visible_start - scroll) as u16;
            let visible_height = (visible_end - visible_start) as u16;
            let component = Rect::new(
                layout.viewport.x + horizontal_padding,
                visible_y,
                layout.viewport.width.saturating_sub(horizontal_padding * 2),
                visible_height,
            );
            let background = Style::default().bg(Color::Rgb(34, 38, 43));
            if selected {
                frame.buffer_mut().set_style(
                    Rect::new(
                        layout.viewport.x,
                        visible_y,
                        layout.viewport.width,
                        visible_height,
                    ),
                    background,
                );
            }
            let style = if selected {
                background
            } else {
                Style::default()
            };
            let paragraph =
                Paragraph::new(card_lines(commit, selected, card_content_width(area.width)))
                    .style(style);
            frame.render_widget(
                paragraph.scroll(((visible_start - start) as u16, 0)),
                component,
            );
        }
        if end >= bottom {
            break;
        }
    }
}

fn render_log_interaction(frame: &mut Frame<'_>, interaction: &LogInteraction) {
    let Some(layout) = signed_in_layout(frame.area()) else {
        return;
    };
    let area = centered_rect(
        layout.feed,
        layout.feed.width.min(58),
        layout.feed.height.min(10),
    );
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    let min_height = match interaction {
        LogInteraction::Report { .. } => 9,
        LogInteraction::Compose { .. } => 7,
        LogInteraction::Detail { .. } | LogInteraction::PastelConfirm { .. } => 6,
    };
    if area.width < 30 || area.height < min_height {
        frame.render_widget(Paragraph::new("Resize to edit · Esc cancel"), area);
        return;
    }
    let inner_width = area.width.saturating_sub(2) as usize;
    if let LogInteraction::Detail {
        handle,
        body,
        scroll,
        ..
    } = interaction
    {
        let [heading, body_area, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(format!("Commit by @{handle}")).block(
                Block::default()
                    .title(" Commit detail ")
                    .borders(Borders::TOP),
            ),
            heading,
        );
        frame.render_widget(
            Paragraph::new(body.as_str())
                .wrap(Wrap { trim: false })
                .scroll((*scroll, 0)),
            body_area,
        );
        frame.render_widget(
            Paragraph::new("j/k scroll · r reply · q quote\nR report · Esc close"),
            footer,
        );
        return;
    }
    let (title, lines) = match interaction {
        LogInteraction::PastelConfirm { handle, error, .. } => {
            let mut lines = vec![
                Line::raw(format!(
                    "Send a Pastel to @{}?",
                    tail_text(handle, inner_width.saturating_sub(10))
                )),
                Line::raw("Uses one from your limited daily quota."),
                Line::raw("Enter send · Esc cancel"),
            ];
            if let Some(error) = error {
                lines.push(Line::raw(format!("! {error}")));
            }
            (" Confirm Pastel ", lines)
        }
        LogInteraction::Detail { .. } => unreachable!(),
        LogInteraction::Compose {
            kind,
            handle,
            body,
            error,
            ..
        } => {
            let title = match kind {
                ComposeKind::Reply => " Reply ",
                ComposeKind::Quote => " Quote ",
            };
            let destination = format!("To @{}", tail_text(handle, inner_width.saturating_sub(4)));
            let visible = tail_text(body, inner_width.saturating_sub(4));
            let mut lines = vec![
                Line::raw(destination),
                Line::raw(format!(" > {visible}█")),
                Line::raw(format!("{} / 4000 characters", body.chars().count())),
            ];
            lines.push(Line::raw("Enter submit · Esc cancel"));
            if let Some(error) = error {
                lines.push(Line::raw(format!("! {error}")));
            }
            (title, lines)
        }
        LogInteraction::Report {
            handle,
            reason,
            confirming,
            error,
            ..
        } => {
            let mut lines = vec![Line::raw(format!(
                "Report @{}",
                tail_text(handle, inner_width.saturating_sub(8))
            ))];
            for (index, option) in ReportReason::ALL.iter().enumerate() {
                let mark = if Some(*option) == *reason { "x" } else { " " };
                lines.push(Line::raw(format!(
                    " {mark}  {}. {}",
                    index + 1,
                    option.label()
                )));
            }
            lines.push(Line::raw(if *confirming {
                "Confirm report: Enter send · Esc cancel"
            } else {
                "1-4 reason · Enter confirm · Esc cancel"
            }));
            if let Some(error) = error {
                lines.push(Line::raw(format!("! {error}")));
            }
            (" Report commit ", lines)
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White).bg(Color::Black)),
        area,
    );
}

fn tail_text(text: &str, width: usize) -> &str {
    for (start, _) in text.char_indices() {
        if Line::from(&text[start..]).width() <= width {
            return &text[start..];
        }
    }
    ""
}

fn wrap_card_hints(width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for hint in CARD_HINTS.split(" · ") {
        let next = if current.is_empty() {
            hint.to_owned()
        } else {
            format!("{current} · {hint}")
        };
        if Line::from(next.as_str()).width() > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            if Line::from(hint).width() > width {
                lines.extend(wrap_card_line(hint, width));
            } else {
                current = hint.to_owned();
            }
        } else {
            current = next;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn card_header_lines<'a>(commit: &'a LogCommit<'_>, selected: bool, width: u16) -> Vec<Line<'a>> {
    let mut header = vec![
        Span::styled(
            if selected { "› " } else { "  " },
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            commit.author,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("@{} · {}", commit.handle, commit.time),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    if commit.pending {
        header.push(Span::styled(
            " · pending",
            Style::default().fg(Color::Yellow),
        ));
    }

    let hash = Span::styled(commit.hash, Style::default().fg(Color::DarkGray));
    let left = Line::from(header);
    let left_width = left.width();
    let hash_width = Line::from(hash.clone()).width();
    if left_width + hash_width < width as usize {
        let mut spans = left.spans;
        spans.push(Span::raw(
            " ".repeat(width as usize - left_width - hash_width),
        ));
        spans.push(hash);
        return vec![Line::from(spans)];
    }

    let left_text = left
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let mut lines: Vec<_> = wrap_card_line(&left_text, width as usize)
        .into_iter()
        .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::DarkGray))))
        .collect();
    let hash_padding = (width as usize).saturating_sub(hash_width);
    lines.push(Line::from(vec![Span::raw(" ".repeat(hash_padding)), hash]));
    lines
}

fn card_body_start(commit: &LogCommit<'_>, width: u16, start: usize) -> usize {
    start + 1 + card_header_lines(commit, false, width).len() + 1
}

fn card_lines<'a>(commit: &'a LogCommit<'_>, selected: bool, width: u16) -> Vec<Line<'a>> {
    let mut lines = vec![Line::raw("")]; // component top padding
    lines.extend(card_header_lines(commit, selected, width));
    lines.push(Line::raw("")); // body top padding
    for body_line in commit.body.split('\n') {
        lines.extend(
            styled_body_lines(body_line, width.saturating_sub(2) as usize)
                .into_iter()
                .map(|line| line.rendered),
        );
    }
    lines.push(Line::raw("")); // body bottom padding
    if commit.thread.is_some() {
        lines.push(Line::raw("  🧵"));
    }
    if selected {
        let left = "☆ 🌸 ❝ Quote ↩ Replies";
        let right = "⧉ ⚑";
        if width as usize >= Line::from(left).width() + Line::from(right).width() + 2 {
            lines.push(Line::from(format!(
                "{left}{}{right}",
                " ".repeat(width as usize - Line::from(left).width() - Line::from(right).width())
            )));
        } else {
            lines.extend(
                wrap_card_line(left, width as usize)
                    .into_iter()
                    .map(Line::from),
            );
            let right_width = Line::from(right).width();
            let padding = (width as usize).saturating_sub(right_width);
            lines.push(Line::from(format!("{}{right}", " ".repeat(padding))));
        }
    }
    lines.push(Line::raw("")); // component bottom padding
    lines
}

struct StyledBodyLine<'a> {
    rendered: Line<'static>,
    links: Vec<(Range<usize>, &'a str)>,
}

pub(crate) fn safe_body_url(raw: &str) -> bool {
    if raw.chars().any(char::is_control) {
        return false;
    }
    let Some((scheme, after_scheme)) = raw.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    !authority.contains('@')
        && url::Url::parse(raw).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

fn visible_urls(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut links = Vec::new();
    let mut consumed_end = 0;
    for (start, character) in text.char_indices() {
        if !matches!(character, 'h' | 'H') {
            continue;
        }
        if start < consumed_end {
            continue;
        }
        if start > 0
            && text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let rest = &text[start..];
        if !rest
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
            && !rest
                .get(..8)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
        {
            continue;
        }
        let stop = rest
            .find(|c: char| c.is_whitespace() || "<>\"'".contains(c))
            .unwrap_or(rest.len());
        if rest[stop..].chars().next().is_some_and(char::is_control) {
            continue;
        }
        consumed_end = start + stop;
        let mut end = consumed_end;
        while end > start && {
            let candidate = &text[start..end];
            let last = candidate.chars().next_back().unwrap();
            ".,;:!?".contains(last)
                || [(')', '('), (']', '['), ('}', '{')]
                    .iter()
                    .any(|&(close, open)| {
                        last == close
                            && candidate.matches(close).count() > candidate.matches(open).count()
                    })
        } {
            end -= text[..end].chars().next_back().unwrap().len_utf8();
        }
        let candidate = &text[start..end];
        if safe_body_url(candidate) {
            links.push((start..end, candidate));
        }
    }
    links
}

fn styled_body_lines<'a>(text: &'a str, width: usize) -> Vec<StyledBodyLine<'a>> {
    let urls = visible_urls(text);
    let mut cursor = 0;
    wrap_card_line(text, width)
        .into_iter()
        .map(|wrapped| {
            let start = cursor
                + text[cursor..]
                    .find(&wrapped)
                    .expect("wrapped text comes from body");
            cursor = start + wrapped.len();
            let mut spans = vec![Span::raw("  ")];
            let mut links = Vec::new();
            let mut prev = 0;
            for (range, url) in &urls {
                let from = range
                    .start
                    .max(start)
                    .saturating_sub(start)
                    .min(wrapped.len());
                let to = range
                    .end
                    .min(cursor)
                    .saturating_sub(start)
                    .min(wrapped.len());
                if from >= to {
                    continue;
                }
                spans.push(Span::raw(wrapped[prev..from].to_owned()));
                let fragment = &wrapped[from..to];
                let cell_start = 2 + Line::from(&wrapped[..from]).width();
                let mut cell_end = cell_start;
                for grapheme in Span::raw(fragment).styled_graphemes(Style::default()) {
                    let next = cell_end + Line::from(grapheme.symbol).width();
                    if next > width + 2 {
                        break;
                    }
                    cell_end = next;
                }
                if cell_start < cell_end {
                    links.push((cell_start..cell_end, *url));
                }
                spans.push(Span::styled(
                    fragment.to_owned(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::UNDERLINED),
                ));
                prev = to;
            }
            spans.push(Span::raw(wrapped[prev..].to_owned()));
            StyledBodyLine {
                rendered: Line::from(spans),
                links,
            }
        })
        .collect()
}

fn wrap_card_line(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for (index, word) in text.split(' ').enumerate() {
        if index > 0 && !current.is_empty() {
            if Line::from(format!("{current} {word}")).width() > width {
                lines.push(std::mem::take(&mut current));
            } else {
                current.push(' ');
            }
        }
        for grapheme in Span::raw(word).styled_graphemes(Style::default()) {
            let candidate = format!("{current}{}", grapheme.symbol);
            if !current.is_empty() && Line::from(candidate.as_str()).width() > width {
                lines.push(std::mem::take(&mut current));
            }
            current.push_str(grapheme.symbol);
        }
    }
    lines.push(current);
    lines
}

fn render_log_tab(frame: &mut Frame<'_>, area: Rect, tab: LogTab, active_tab: LogTab) {
    let label = Line::from(vec![
        Span::styled(
            tab.label(),
            if tab == active_tab {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::raw(" "),
        Span::styled(
            format!("[{}]", tab.key()),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(label).alignment(Alignment::Center), area);
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, current_page: Page, keymap: &Keymap) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut y = area.y;

    let compact = area.width < 18;
    for page in Page::ALL {
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        let active = page == current_page;
        let key = keymap.binding(page);
        let key_label = format!("{} {}", key[0], key[1]);
        let label_style = if active {
            Style::default().fg(Color::White)
        } else {
            Style::default()
        };
        if compact {
            for label in sidebar_label_lines(page, active, area.width) {
                if y >= area.y.saturating_add(area.height) {
                    break;
                }
                frame.render_widget(Paragraph::new(label), Rect::new(area.x, y, area.width, 1));
                y = y.saturating_add(1);
            }
            if y >= area.y.saturating_add(area.height) {
                break;
            }

            let key_padding = (area.width as usize).saturating_sub(key_label.chars().count());
            let key_line = Line::from(vec![
                Span::raw(" ".repeat(key_padding)),
                Span::styled(key_label, Style::default().fg(Color::DarkGray)),
            ]);
            frame.render_widget(
                Paragraph::new(key_line),
                Rect::new(area.x, y, area.width, 1),
            );
            y = y.saturating_add(1);
        } else {
            let key_padding = (area.width as usize)
                .saturating_sub(page.label().chars().count() + 2 + key_label.chars().count());
            let line = Line::from(vec![
                Span::styled(
                    if active { "›" } else { " " },
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(" "),
                Span::styled(page.label(), label_style),
                Span::raw(" ".repeat(key_padding)),
                Span::styled(key_label, Style::default().fg(Color::DarkGray)),
            ]);
            frame.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
            y = y.saturating_add(1);
        }
    }
}

fn sidebar_label_lines(page: Page, active: bool, width: u16) -> Vec<Line<'static>> {
    let label = page.label();
    let label_style = if active {
        Style::default().fg(Color::White)
    } else {
        Style::default()
    };
    let marker_style = Style::default().fg(Color::DarkGray);

    if active && label.chars().count() + 2 <= width as usize {
        return vec![Line::from(vec![
            Span::styled(if active { "›" } else { " " }, marker_style),
            Span::raw(" "),
            Span::styled(label, label_style),
        ])];
    }

    let mut lines = Vec::new();
    if active {
        lines.push(Line::from(Span::styled("›", marker_style)));
    }
    let characters: Vec<_> = label.chars().collect();
    lines.extend(characters.chunks(width as usize).map(|chunk| {
        let text: String = chunk.iter().collect();
        Line::from(Span::styled(text, label_style))
    }));
    lines
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, style::Color, Terminal};

    use super::{
        card_body_start, card_content_width, card_feed_layout, card_header_lines, render,
        signed_in_layout, step_at_mouse_position, visible_urls, wrap_card_line, LogCommit, View,
        CARD_SIDE_PADDING, COLUMN_PADDING,
    };
    use crate::{
        app::{Action, App, Mode, Step},
        navigation::{Keymap, LogTab, Page},
    };

    fn test_keymap() -> Keymap {
        Keymap::default()
    }

    fn assert_blank(buffer: &Buffer, area: Rect) {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                assert_eq!(buffer[(x, y)].symbol(), " ", "at ({x}, {y})");
            }
        }
    }

    #[test]
    fn detail_dialog_shows_focused_commit_and_keyboard_actions() {
        let keymap = test_keymap();
        let mut app = App::default();
        app.begin_detail("uuid", "ada", "body from API\nsecond line");
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInInteraction {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Latest,
                        log_scroll: 0,
                        commits: &[],
                        focused_commit: None,
                        interaction: app.log_interaction().unwrap(),
                    },
                )
            })
            .unwrap();
        let content = buffer_lines(terminal.backend().buffer()).join(" ");
        for text in [
            "Commit by @ada",
            "body from API",
            "j/k scroll",
            "r reply",
            "q quote",
            "R report",
            "Esc close",
        ] {
            assert!(content.contains(text), "missing {text}");
        }
    }

    #[test]
    fn tiny_log_dialog_does_not_cover_tabs_or_sidebar() {
        let keymap = test_keymap();
        let mut app = App::default();
        app.begin_compose(crate::app::ComposeKind::Reply, "uuid", "ada");
        let mut terminal = Terminal::new(TestBackend::new(65, 12)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInInteraction {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Following,
                        log_scroll: 0,
                        commits: &[],
                        focused_commit: None,
                        interaction: app.log_interaction().unwrap(),
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer_lines(buffer)[6].contains("Following [1]")); // sticky tabs
        let layout = signed_in_layout(buffer.area).unwrap();
        assert_eq!(buffer[(layout.sidebar.x, layout.sidebar.y)].symbol(), "›"); // sidebar focus
    }

    #[test]
    fn report_dialog_shows_reason_and_explicit_confirmation_only_on_log() {
        let keymap = test_keymap();
        let mut app = App::default();
        app.begin_report("uuid", "ada");
        app.update_log(Action::NavigationKey('1'));
        app.update_log(Action::LogEnter);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let commits: [LogCommit<'_>; 0] = [];
        for page in [Page::Log, Page::Profile] {
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &View::SignedInInteraction {
                            current_page: page,
                            keymap: &keymap,
                            log_tab: LogTab::Latest,
                            log_scroll: 0,
                            commits: &commits,
                            focused_commit: None,
                            interaction: app.log_interaction().unwrap(),
                        },
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let content: String = (0..buffer.area.height)
                .flat_map(|y| {
                    (0..buffer.area.width).map(move |x| buffer[(x, y)].symbol().to_owned())
                })
                .collect();
            assert_eq!(
                content.contains("Confirm report: Enter send"),
                page == Page::Log
            );
            assert_eq!(content.contains("x  1. spam"), page == Page::Log);
        }
    }

    #[test]
    fn signed_in_view_renders_log_tabs_and_keeps_right_pane_blank() {
        let keymap = test_keymap();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedIn {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Following,
                        log_scroll: 0,
                        feed_entries: &[],
                    },
                )
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let layout = signed_in_layout(buffer.area).unwrap();
        assert_eq!((layout.brand.x, layout.brand.width), (33, 34));
        assert_eq!(layout.sidebar.width, 13);
        assert_eq!(layout.sidebar.x, 16);
        assert_blank(buffer, Rect::new(0, 0, 15, buffer.area.height));
        assert_eq!(layout.brand.x, 30 + COLUMN_PADDING);
        assert_eq!(layout.brand.right() + COLUMN_PADDING, 70);
        assert_blank(buffer, Rect::new(0, 0, buffer.area.width, 3));
        assert_eq!(buffer[(layout.sidebar.x, layout.sidebar.y)].symbol(), "›");
        assert_eq!(
            buffer[(layout.sidebar.x + 2, layout.sidebar.y)].symbol(),
            "L"
        );
        assert_eq!(
            buffer[(layout.sidebar.right() - 3, layout.sidebar.y + 1)].symbol(),
            "g"
        );
        assert!((layout.sidebar.y..layout.sidebar.bottom())
            .any(|row| buffer_lines(buffer)[row as usize].contains("↗")));
        let x = layout.brand.x;
        let y = layout.brand.y;
        assert_eq!(buffer[(x, y)].symbol(), "/");
        assert_eq!(buffer[(x, y)].style().fg, Some(Color::White));
        assert_eq!(buffer[(x + 1, y)].style().fg, Some(Color::Rgb(255, 165, 0)));
        assert_eq!(buffer[(x + 3, y)].symbol(), "/");
        assert_eq!(buffer[(x + 7, y)].style().fg, Some(Color::Rgb(255, 165, 0)));
        assert_eq!(buffer[(x, layout.tagline.y)].symbol(), "A");
        assert_eq!(
            buffer[(x, layout.tagline.y)].style().fg,
            Some(Color::DarkGray)
        );
        for (row, label) in [(0, "Following [1]"), (1, "Latest [2]"), (2, "Trending [3]")] {
            let text = (layout.tabs.x..layout.tabs.right())
                .map(|x| buffer[(x, layout.tabs.y + row)].symbol())
                .collect::<Vec<_>>()
                .join("");
            assert!(text.contains(label), "missing {label} in {text:?}");
        }
        assert_eq!(layout.tabs.height, 3); // 100 columns still requires stacked tabs
        assert_blank(
            buffer,
            Rect::new(layout.feed.x, layout.tabs.bottom(), layout.feed.width, 2),
        );
        assert_blank(buffer, layout.feed);
        assert_blank(buffer, Rect::new(70, 0, 30, buffer.area.height));
        assert!(buffer[(layout.sidebar.x, layout.sidebar.y)]
            .style()
            .fg
            .is_some());
    }

    #[test]
    fn sticky_log_tabs_survive_scrolling_and_feed_items_have_a_blank_row() {
        let keymap = test_keymap();
        let entries: Vec<_> = (0..5).map(|index| format!("commit {index}")).collect();
        let mut terminal = Terminal::new(TestBackend::new(100, 18)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedIn {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Latest,
                        log_scroll: 2,
                        feed_entries: &entries,
                    },
                )
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let layout = signed_in_layout(buffer.area).unwrap();
        assert_eq!(buffer[(layout.brand.x, layout.brand.y)].symbol(), "/");
        assert_eq!(buffer[(layout.tagline.x, layout.tagline.y)].symbol(), "A");
        assert_eq!(layout.tabs.height, 3);
        let tab_row = |y| {
            (layout.tabs.x..layout.tabs.right())
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        };
        assert!(tab_row(layout.tabs.y).contains("Following [1]"));
        assert!(tab_row(layout.tabs.y + 1).contains("Latest [2]"));
        assert!(tab_row(layout.tabs.y + 2).contains("Trending [3]"));
        let active_x = (layout.tabs.x..layout.tabs.right())
            .find(|&x| buffer[(x, layout.tabs.y + 1)].symbol() == "L")
            .unwrap();
        assert_eq!(
            buffer[(active_x, layout.tabs.y + 1)].style().fg,
            Some(Color::White)
        );
        assert_blank(
            buffer,
            Rect::new(layout.feed.x, layout.tabs.bottom(), layout.feed.width, 2),
        );
        assert_eq!(buffer[(layout.feed.x + 7, layout.feed.y)].symbol(), "1");
        assert_eq!(buffer[(layout.feed.x, layout.feed.y + 1)].symbol(), " ");
        assert_eq!(buffer[(layout.feed.x + 7, layout.feed.y + 2)].symbol(), "2");
        assert_blank(buffer, Rect::new(70, 0, 30, buffer.area.height));
    }

    #[test]
    fn narrow_log_tabs_stack_without_clipping_with_column_padding() {
        let keymap = test_keymap();
        let mut terminal = Terminal::new(TestBackend::new(65, 14)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedIn {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Latest,
                        log_scroll: 0,
                        feed_entries: &["commit 0".to_owned()],
                    },
                )
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let layout = signed_in_layout(buffer.area).unwrap();
        assert_eq!(buffer[(layout.brand.x, layout.brand.y)].symbol(), "/");
        assert_eq!(buffer[(layout.tagline.x, layout.tagline.y)].symbol(), "A");
        assert_eq!(layout.tabs.height, 3);
        for (index, expected) in ["Following [1]", "Latest [2]", "Trending [3]"]
            .into_iter()
            .enumerate()
        {
            let text = (layout.tabs.x..layout.tabs.right())
                .map(|x| buffer[(x, layout.tabs.y + index as u16)].symbol())
                .collect::<Vec<_>>()
                .join("");
            assert!(text.contains(expected), "missing {expected} in {text:?}");
        }
        assert_blank(
            buffer,
            Rect::new(layout.feed.x, layout.tabs.bottom(), layout.feed.width, 2),
        );
        assert_eq!(buffer[(layout.feed.x + 7, layout.feed.y)].symbol(), "0");
    }

    #[test]
    fn compact_sidebar_wraps_labels_and_right_aligns_keys() {
        let keymap = test_keymap();
        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedIn {
                        current_page: Page::Settings,
                        keymap: &keymap,
                        log_tab: LogTab::Following,
                        log_scroll: 0,
                        feed_entries: &[],
                    },
                )
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let layout = signed_in_layout(buffer.area).unwrap();
        assert!(layout.sidebar.width <= 10);
        let sidebar_lines: Vec<_> = (layout.sidebar.y..layout.sidebar.bottom())
            .map(|row| {
                (layout.sidebar.x..layout.sidebar.right())
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        assert!(sidebar_lines.iter().any(|line| line.contains('›')));
        assert!(sidebar_lines.concat().contains("Settings"));
        assert!(sidebar_lines.iter().any(|line| line.ends_with("g s")));
        assert_eq!(buffer[(layout.brand.x, layout.brand.y)].symbol(), "/");
        assert_blank(
            buffer,
            Rect::new(
                layout.brand.x,
                layout.tagline.bottom(),
                layout.brand.width,
                1,
            ),
        );
        assert_blank(
            buffer,
            Rect::new(
                layout.brand.x,
                layout.feed.y,
                layout.feed.width,
                layout.feed.height,
            ),
        );
        assert_blank(buffer, Rect::new(56, 0, 24, buffer.area.height));
    }

    #[test]
    fn sidebar_position_stays_fixed_when_switching_surfaces() {
        let keymap = test_keymap();
        for (width, height) in [(50, 28), (65, 28), (100, 24)] {
            for page in [Page::Log, Page::Settings] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| {
                        render(
                            frame,
                            &View::SignedIn {
                                current_page: page,
                                keymap: &keymap,
                                log_tab: LogTab::Following,
                                log_scroll: 0,
                                feed_entries: &[],
                            },
                        )
                    })
                    .unwrap();
                let layout = signed_in_layout(Rect::new(0, 0, width, height)).unwrap();
                assert_eq!(
                    terminal.backend().buffer()[(
                        layout.sidebar.right() - 3,
                        layout.sidebar.y + u16::from(layout.sidebar.width < 18)
                    )]
                        .symbol(),
                    "g"
                );
            }
        }
    }

    #[test]
    fn typed_cards_render_metadata_thread_actions_focus_and_two_blank_rows() {
        let keymap = test_keymap();
        let commits = [
            LogCommit {
                author: "Ada",
                handle: "ada",
                time: "2m",
                pending: true,
                hash: "a1b2c3",
                body: "Ship it",
                thread: Some("3 replies"),
            },
            LogCommit {
                author: "Lin",
                handle: "lin",
                time: "1h",
                pending: false,
                hash: "f00baa",
                body: "Second",
                thread: None,
            },
        ];
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInCards {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Following,
                        log_scroll: 0,
                        commits: &commits,
                        focused_commit: Some(0),
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let layout = card_feed_layout(100, 30, &commits, Some(0)).unwrap();
        let x = layout.viewport.x + CARD_SIDE_PADDING;
        let y = layout.viewport.y;
        let lines = buffer_lines(buffer);
        let first = &layout.rows[0];
        let first_header = y + first.start as u16 + 1;
        let body_start = y + card_body_start(
            &commits[0],
            card_content_width(layout.viewport.width),
            first.start,
        ) as u16;
        let first_thread = body_start + 2;
        let first_actions = first_thread + 1;
        assert_eq!(buffer[(x, y)].symbol(), " "); // component top pad
        assert_eq!(
            buffer[(layout.viewport.x, y)].style().bg,
            Some(Color::Rgb(34, 38, 43))
        ); // focus background includes side pad
        assert!(lines[first_header as usize].contains("› Ada @ada · 2m · pending"));
        let hash_x = layout.viewport.right() - CARD_SIDE_PADDING - commits[0].hash.len() as u16;
        let hash_y = first_header
            + card_header_lines(&commits[0], true, card_content_width(layout.viewport.width)).len()
                as u16
            - 1;
        assert_eq!(buffer[(hash_x, hash_y)].symbol(), "a"); // right aligned hash
        assert_eq!(buffer[(x, body_start - 1)].symbol(), " "); // body top pad
        assert!(lines[body_start as usize].contains("Ship it"));
        assert_eq!(buffer[(x + 2, first_thread)].symbol(), "🧵");
        assert!(lines[first_actions as usize].contains("☆ 🌸"));
        assert!(lines[first_actions as usize].contains("❝ Quote"));
        assert!(lines[first_actions as usize].contains("↩ Replies"));
        assert!(lines[first_actions as usize].contains("⧉ ⚑"));
        assert_eq!(buffer[(x, first.end as u16 + y - 1)].symbol(), " ");
        assert_eq!(buffer[(x, first.end as u16 + y)].symbol(), " ");
        let second_header = y + layout.rows[1].start as u16 + 1;
        assert!(lines[second_header as usize].contains("Lin @lin · 1h"));
        assert!(!lines[second_header as usize].contains("pending"));
        assert!(!lines[second_header as usize].contains('›'));
        let screen_layout = signed_in_layout(buffer.area).unwrap();
        let hints = lines[screen_layout.footer.y as usize..screen_layout.footer.bottom() as usize]
            .join(" ");
        assert!(!lines[layout.footer.y as usize].contains("j/k"));
        for key in [
            "j/k",
            "s star",
            "r reply",
            "q quote",
            "y copy",
            "R report",
            "o open",
            "Enter detail",
            "1-3 tabs",
        ] {
            assert!(hints.contains(key), "missing {key} in {hints:?}");
        }
        for row in y..y + first.end as u16 {
            assert_eq!(
                buffer[(layout.viewport.right() - 1, row)].style().bg,
                Some(Color::Rgb(34, 38, 43))
            );
        }
        assert_eq!(
            buffer[(layout.viewport.right() - 1, y + first.end as u16)]
                .style()
                .bg,
            Some(Color::Reset)
        );
        let screen_layout = signed_in_layout(buffer.area).unwrap();
        assert_blank(buffer, Rect::new(70, 0, 30, screen_layout.footer.y));
    }

    #[test]
    fn typed_cards_wrap_without_overwriting_tabs_or_sidebar_and_clip_on_scroll() {
        let keymap = test_keymap();
        let commits = [LogCommit {
            author: "Writer",
            handle: "handle",
            time: "now",
            pending: false,
            hash: "abcdef",
            body: "abcdefghijklmnopqrstuvwxyz0123456789",
            thread: None,
        }];
        let mut terminal = Terminal::new(TestBackend::new(65, 26)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInCards {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Latest,
                        log_scroll: 0,
                        commits: &commits,
                        focused_commit: None,
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let layout = signed_in_layout(buffer.area).unwrap();
        let feed_layout = card_feed_layout(65, 26, &commits, None).unwrap();
        let body_y = feed_layout.viewport.y
            + card_body_start(
                &commits[0],
                card_content_width(feed_layout.viewport.width),
                feed_layout.rows[0].start,
            ) as u16;
        let lines = buffer_lines(buffer);
        assert!(lines[layout.tabs.bottom() as usize - 1].contains("Trending [3]"));
        assert_blank(
            buffer,
            Rect::new(layout.feed.x, layout.tabs.bottom(), layout.feed.width, 2),
        );
        let body_lines = wrap_card_line(
            commits[0].body,
            card_content_width(feed_layout.viewport.width).saturating_sub(2) as usize,
        );
        assert!(lines[body_y as usize].contains(&body_lines[0]));
        assert!(lines[body_y as usize + 1].contains(&body_lines[1]));
        assert!(!lines.iter().any(|line| line.contains("j/k select")));
        assert_blank(buffer, Rect::new(46, 0, 19, buffer.area.height));

        let mut short = Terminal::new(TestBackend::new(100, 16)).unwrap();
        let feed = card_feed_layout(100, 16, &commits, Some(0)).unwrap();
        let bottom_scroll = feed.clamp_scroll(u16::MAX);
        short
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInCards {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Latest,
                        log_scroll: bottom_scroll,
                        commits: &commits,
                        focused_commit: Some(0),
                    },
                )
            })
            .unwrap();
        let buffer = short.backend().buffer();
        let geometry = signed_in_layout(buffer.area).unwrap();
        let lines = buffer_lines(buffer);
        assert!(lines[(geometry.tabs.y + 1) as usize].contains("Latest [2]"));
        assert_blank(
            buffer,
            Rect::new(
                geometry.feed.x,
                geometry.tabs.bottom(),
                geometry.feed.width,
                2,
            ),
        );
        let feed_text = lines[feed.viewport.y as usize..feed.viewport.bottom() as usize].join(" ");
        assert!(feed_text.contains("☆"));
        assert!(feed_text.contains("❝ Quote"));
        assert!(feed_text.contains("↩ Replies"));
        let footer_text =
            lines[geometry.footer.y as usize..geometry.footer.bottom() as usize].join(" ");
        assert!(footer_text.contains("j/k select"));
    }

    #[test]
    fn card_feed_geometry_tracks_width_wrapping_tabs_and_footer() {
        let long_body = "x".repeat(64);
        let commits = [LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abcd",
            body: &long_body,
            thread: Some("reply"),
        }];
        let wide = card_feed_layout(100, 30, &commits, Some(0)).unwrap();
        let compact = card_feed_layout(65, 30, &commits, Some(0)).unwrap();
        let wide_feed = signed_in_layout(Rect::new(0, 0, 100, 30)).unwrap().feed;
        let compact_feed = signed_in_layout(Rect::new(0, 0, 65, 30)).unwrap().feed;
        assert_eq!(
            (wide.viewport.x, wide.viewport.y, wide.viewport.width),
            (wide_feed.x, wide_feed.y, wide_feed.width)
        );
        assert_eq!(
            (
                compact.viewport.x,
                compact.viewport.y,
                compact.viewport.width
            ),
            (compact_feed.x, compact_feed.y, compact_feed.width)
        );
        assert_eq!(
            (wide.viewport.x, wide.viewport.y, wide.viewport.width),
            (33, 11, 34)
        );
        assert!(compact.viewport.width < wide.viewport.width);
        assert_eq!(wide.rows[0], 0..10); // component pads, wrapped body, thread, actions
        assert!(compact.rows[0].end > wide.rows[0].end);
        assert!(wide.status_row(false) >= wide.viewport.bottom());
        assert!(compact.status_row(false) >= compact.viewport.bottom());
        assert!(wide.viewport.bottom() < 30);
        assert!(compact.viewport.bottom() < 30);
        for layout in [&wide, &compact] {
            for row in [layout.viewport.y - 2, layout.viewport.y - 1] {
                assert!(!layout.contains(layout.viewport.x, row)); // fixed tab gap
                assert_eq!(layout.card_at(layout.viewport.x, row, 0), None);
            }
            assert!(!layout.contains(layout.viewport.x - 1, layout.viewport.y)); // sidebar
            assert!(!layout.contains(layout.viewport.x, layout.viewport.bottom())); // hints
            assert!(layout.contains(layout.viewport.x, layout.viewport.y));
            assert_eq!(
                layout.card_at(layout.viewport.x, layout.viewport.y, 0),
                Some(0)
            );
        }
        assert!(card_feed_layout(2, 30, &commits, Some(0)).is_none());
    }

    #[test]
    fn body_urls_strip_punctuation_and_reject_malformed_urls() {
        let body = "(https://example.org/a(b)?q=1), http://host.dev/path. HTTP://upper.dev/path HTTPS://Upper.dev?q=1 http:// https://?q=1 http://a:bad nopehttps://host.dev";
        let urls: Vec<_> = visible_urls(body).into_iter().map(|(_, url)| url).collect();
        assert_eq!(
            urls,
            [
                "https://example.org/a(b)?q=1",
                "http://host.dev/path",
                "HTTP://upper.dev/path",
                "HTTPS://Upper.dev?q=1"
            ]
        );
        let commit = [LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abc",
            body: "(https://example.org/a(b)), http://",
            thread: None,
        }];
        let layout = card_feed_layout(100, 24, &commit, None).unwrap();
        let x = layout.viewport.x + CARD_SIDE_PADDING + 2;
        let y = layout.viewport.y
            + card_body_start(
                &commit[0],
                card_content_width(layout.viewport.width),
                layout.rows[0].start,
            ) as u16;
        assert_eq!(
            layout.link_at(x + 3, y, 0, &commit),
            Some("https://example.org/a(b)")
        );
        assert_eq!(
            layout.link_at(
                x + 3 + "https://example.org/a(b)".len() as u16,
                y,
                0,
                &commit
            ),
            None
        ); // closing parenthesis
        assert_eq!(
            layout.link_at(
                x + 5 + "https://example.org/a(b)".len() as u16,
                y,
                0,
                &commit
            ),
            None
        ); // comma
        assert_eq!(
            layout.link_at(
                x + 7 + "https://example.org/a(b)".len() as u16,
                y,
                0,
                &commit
            ),
            None
        ); // malformed URL
    }

    #[test]
    fn unsafe_body_urls_are_plain_text_and_have_no_hit_cells() {
        use ratatui::style::Modifier;

        let body = "https://@host.dev\nhttps://user:pass@host.dev\nhttps://ok.dev/a?q=1,";
        assert!(visible_urls("https://ok.dev/\tmalicious").is_empty());
        assert!(visible_urls("https://ok.dev/\rmalicious").is_empty());
        assert!(visible_urls("https://user@host.dev/http://ok.dev").is_empty());
        assert_eq!(
            visible_urls(body)
                .into_iter()
                .map(|(_, url)| url)
                .collect::<Vec<_>>(),
            ["https://ok.dev/a?q=1"]
        );
        let commits = [LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abc",
            body,
            thread: None,
        }];
        let layout = card_feed_layout(100, 24, &commits, None).unwrap();
        let x = layout.viewport.x + CARD_SIDE_PADDING + 2;
        let y = layout.viewport.y
            + card_body_start(
                &commits[0],
                card_content_width(layout.viewport.width),
                layout.rows[0].start,
            ) as u16;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInCards {
                        current_page: Page::Log,
                        keymap: &test_keymap(),
                        log_tab: LogTab::Latest,
                        log_scroll: 0,
                        commits: &commits,
                        focused_commit: None,
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        for row in [y, y + 1] {
            assert_eq!(layout.link_at(x, row, 0, &commits), None);
            assert!(!buffer[(x, row)].modifier.contains(Modifier::UNDERLINED));
        }
        assert_eq!(
            layout.link_at(x, y + 2, 0, &commits),
            Some("https://ok.dev/a?q=1")
        );
        assert!(buffer[(x, y + 2)].modifier.contains(Modifier::UNDERLINED));
        assert_eq!(
            layout.link_at(x + "https://ok.dev/a?q=1".len() as u16, y + 2, 0, &commits),
            None
        ); // comma
        for row in [y + 3, y + 4] {
            assert_eq!(layout.link_at(x, row, 0, &commits), None); // fixed gap
        }
    }

    #[test]
    fn links_style_and_hit_test_wrapped_visible_cells_only() {
        use ratatui::style::Modifier;

        let keymap = test_keymap();
        let body = "中🙂 https://example.org/abcdefghij?q=1.\nhttps://other.dev";
        let commits = [LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abc",
            body,
            thread: None,
        }];
        let layout = card_feed_layout(65, 24, &commits, Some(0)).unwrap();
        let x = layout.viewport.x + CARD_SIDE_PADDING;
        let y = layout.viewport.y
            + card_body_start(
                &commits[0],
                card_content_width(layout.viewport.width),
                layout.rows[0].start,
            ) as u16;
        let url = "https://example.org/abcdefghij?q=1";
        let first_url_rows: Vec<_> = (y..layout.viewport.bottom())
            .filter(|row| layout.link_at(x + 2, *row, 0, &commits) == Some(url))
            .collect();
        assert!(first_url_rows.len() >= 2);
        let other_row = (y..layout.viewport.bottom())
            .find(|row| layout.link_at(x + 2, *row, 0, &commits) == Some("https://other.dev"))
            .unwrap();
        assert!(other_row > *first_url_rows.last().unwrap());
        assert_eq!(layout.link_at(x + 2, y, 0, &commits), None); // CJK prefix
        assert_eq!(layout.link_at(x, y + 1, 0, &commits), None); // component padding
        assert_eq!(layout.link_at(x, y - 1, 0, &commits), None); // body pad
        assert_eq!(layout.link_at(x - 1, y + 1, 0, &commits), None); // outside viewport
        assert_eq!(layout.link_at(x, y - 2, 0, &commits), None); // header
        assert_eq!(
            layout.link_at(x, layout.viewport.bottom(), 0, &commits),
            None
        );
        let mut terminal = Terminal::new(TestBackend::new(65, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInCards {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Latest,
                        log_scroll: 0,
                        commits: &commits,
                        focused_commit: Some(0),
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer[(x + 2, first_url_rows[0])]
            .modifier
            .contains(Modifier::UNDERLINED));
        assert!(!buffer[(x + 2, y)].modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn link_hits_exclude_scrolled_hidden_and_nonrendered_cells() {
        let body = "https://first.dev\nhttps://second.dev";
        let commits = [LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abc",
            body,
            thread: None,
        }];
        let layout = card_feed_layout(100, 18, &commits, None).unwrap();
        let x = layout.viewport.x + CARD_SIDE_PADDING + 2;
        let body_start = card_body_start(
            &commits[0],
            card_content_width(layout.viewport.width),
            layout.rows[0].start,
        );
        let body_y = layout.viewport.y + body_start as u16;
        assert_eq!(
            layout.link_at(x, body_y, 0, &commits),
            Some("https://first.dev")
        );
        assert_eq!(layout.link_at(layout.viewport.x, body_y, 0, &commits), None); // left side padding
        let scrolled_y = layout.viewport.y + (body_start - 1) as u16;
        assert_eq!(
            layout.link_at(x, scrolled_y, 1, &commits),
            Some("https://first.dev")
        );
        assert_eq!(
            layout.link_at(x, scrolled_y + 1, 1, &commits),
            Some("https://second.dev")
        );
        assert_eq!(
            layout.link_at(x, layout.viewport.bottom(), 1, &commits),
            None
        );
        assert_eq!(layout.link_at(x, body_y, 0, &[]), None);

        let tiny = card_feed_layout(4, 20, &commits, None).unwrap();
        assert_eq!(tiny.viewport.width, 0);
        assert_eq!(
            tiny.link_at(tiny.viewport.x, tiny.viewport.y, 0, &commits),
            None
        );
        assert_eq!(wrap_card_line("👨‍💻中", 2), ["👨‍💻", "中"]);

        let commit = |body| LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abc",
            body,
            thread: None,
        };
        let repeated = [commit(body), commit(body)];
        let gap_layout = card_feed_layout(100, 30, &repeated, None).unwrap();
        let gap_y = gap_layout.viewport.y + gap_layout.rows[0].end as u16;
        assert_eq!(
            gap_layout.link_at(
                gap_layout.viewport.x + CARD_SIDE_PADDING + 2,
                gap_y,
                0,
                &repeated,
            ),
            None
        );
        assert_eq!(
            gap_layout.link_at(
                gap_layout.viewport.x + CARD_SIDE_PADDING + 2,
                gap_y + 1,
                0,
                &repeated,
            ),
            None
        );

        let unicode_commit = [commit("https://a.dev/中")];
        let narrow = card_feed_layout(23, 50, &unicode_commit, None).unwrap();
        assert!(narrow.viewport.width > 0);
        let hidden_wide_row = narrow.viewport.y
            + card_body_start(
                &unicode_commit[0],
                card_content_width(narrow.viewport.width),
                narrow.rows[0].start,
            ) as u16
            + "https://a.dev/".len() as u16;
        assert_eq!(
            narrow.link_at(
                narrow.viewport.x + CARD_SIDE_PADDING + 2,
                hidden_wide_row,
                0,
                &unicode_commit,
            ),
            None
        );
    }

    #[test]
    fn card_feed_offsets_hit_gaps_and_keep_variable_rows_visible() {
        let long_body = "x".repeat(105);
        let commits = [
            LogCommit {
                author: "Ada",
                handle: "a",
                time: "now",
                pending: false,
                hash: "1",
                body: &long_body,
                thread: None,
            },
            LogCommit {
                author: "Lin",
                handle: "l",
                time: "now",
                pending: false,
                hash: "2",
                body: "short",
                thread: Some("reply"),
            },
            LogCommit {
                author: "Max",
                handle: "m",
                time: "now",
                pending: false,
                hash: "3",
                body: "last",
                thread: None,
            },
        ];
        let layout = card_feed_layout(100, 16, &commits, Some(1)).unwrap();
        assert_eq!(layout.rows, [0..9, 9..17, 17..23]);
        assert_eq!(
            layout.card_at(layout.viewport.x, layout.viewport.y, 0),
            Some(0)
        );
        assert_eq!(
            layout.card_at(
                layout.viewport.x,
                layout.viewport.y,
                (layout.rows[0].end - 1) as u16
            ),
            Some(0)
        ); // previous card bottom padding
        assert_eq!(
            layout.card_at(
                layout.viewport.x,
                layout.viewport.y,
                layout.rows[0].end as u16
            ),
            Some(1)
        ); // next card top padding
        assert_eq!(
            layout.card_at(
                layout.viewport.x,
                layout.viewport.y,
                layout.rows[1].start as u16
            ),
            Some(1)
        );
        let scroll = layout.scroll_to_show(0, 2);
        assert!(scroll as usize <= layout.rows[2].start);
        assert!(layout.rows[2].start < scroll as usize + layout.viewport.height as usize);
        assert_eq!(layout.scroll_to_show(scroll, 0), 0);
        assert_eq!(
            layout.clamp_scroll(u16::MAX) as usize,
            layout.rows[2].end - layout.viewport.height as usize
        );
        assert_eq!(
            layout.card_at(layout.viewport.x, layout.viewport.bottom(), scroll),
            None
        );
        assert_eq!(
            layout.card_at(layout.viewport.right(), layout.viewport.y, scroll),
            None
        );
        assert_eq!(layout.scroll_to_show(0, commits.len()), 0);
    }

    #[test]
    fn tiny_focused_feed_keeps_one_card_row_without_overlapping_hints() {
        let commits = [LogCommit {
            author: "Ada",
            handle: "a",
            time: "now",
            pending: false,
            hash: "1",
            body: "body",
            thread: None,
        }];
        let layout = card_feed_layout(65, 14, &commits, Some(0)).unwrap();
        assert_eq!(layout.viewport.height, 1);
        assert_eq!(layout.footer.height, 0);
        assert_eq!(layout.footer.y, layout.viewport.bottom());
        assert_eq!(layout.status_row(false), layout.footer.bottom() - 1);
        assert_eq!(
            layout.card_at(layout.viewport.x, layout.viewport.y, 2),
            Some(0)
        );
        let tiny = card_feed_layout(65, 11, &commits, Some(0)).unwrap();
        assert_eq!(tiny.viewport.height, 0);
        assert!(!tiny.contains(tiny.viewport.x, tiny.viewport.y));
        assert_eq!(tiny.card_at(tiny.viewport.x, tiny.viewport.y, 0), None);

        let keymap = test_keymap();
        for (width, height) in [(65, 11), (100, 9)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &View::SignedInCards {
                            current_page: Page::Log,
                            keymap: &keymap,
                            log_tab: LogTab::Latest,
                            log_scroll: 0,
                            commits: &commits,
                            focused_commit: Some(0),
                        },
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let geometry = signed_in_layout(buffer.area).unwrap();
            assert_blank(
                buffer,
                Rect::new(
                    geometry.feed.x,
                    geometry.tabs.bottom(),
                    geometry.feed.width,
                    geometry.footer.y.saturating_sub(geometry.tabs.bottom()),
                ),
            );
        }
    }

    #[test]
    fn compact_hints_wrap_without_covering_cards_or_sticky_tabs() {
        let keymap = test_keymap();
        let commits = [LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abcd",
            body: "visible body",
            thread: None,
        }];
        let layout = card_feed_layout(65, 24, &commits, Some(0)).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(65, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInCards {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Latest,
                        log_scroll: 0,
                        commits: &commits,
                        focused_commit: Some(0),
                    },
                )
            })
            .unwrap();
        let lines = buffer_lines(terminal.backend().buffer());
        assert!(lines[8].contains("Trending [3]"));
        let header_y = layout.viewport.y + 1;
        let body_y = layout.viewport.y
            + card_body_start(
                &commits[0],
                card_content_width(layout.viewport.width),
                layout.rows[0].start,
            ) as u16;
        assert!(lines[header_y as usize].contains("Ada @ada"));
        assert!(lines[body_y as usize].contains("visible body"));
        let footer = signed_in_layout(terminal.backend().buffer().area)
            .unwrap()
            .footer;
        let hints = lines[footer.y as usize..footer.bottom() as usize].join(" ");
        for key in [
            "j/k select",
            "s star",
            "p pastel",
            "r reply",
            "q quote",
            "y copy",
            "R report",
            "o open",
            "Enter detail",
            "1-3 tabs",
        ] {
            assert!(hints.contains(key), "missing {key} in {hints:?}");
        }
        assert!(!hints.contains("visible body"));
        assert!(lines[header_y as usize].contains("Ada"));
    }

    #[test]
    fn wrapping_respects_display_width_and_empty_lines() {
        assert_eq!(wrap_card_line("", 5), [""]);
        assert_eq!(wrap_card_line("中文abcd", 4), ["中文", "abcd"]);
        assert_eq!(wrap_card_line("e\u{301}cole", 3), ["e\u{301}co", "le"]);
        assert_eq!(wrap_card_line("hello world", 5), ["hello", "world"]);
    }

    #[test]
    fn empty_typed_log_has_no_card_focus_or_hints() {
        let keymap = test_keymap();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInCards {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Following,
                        log_scroll: u16::MAX,
                        commits: &[],
                        focused_commit: Some(0),
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let geometry = signed_in_layout(buffer.area).unwrap();
        assert_blank(
            buffer,
            Rect::new(
                geometry.feed.x,
                geometry.tabs.bottom(),
                geometry.feed.width,
                2,
            ),
        );
        assert_blank(buffer, geometry.feed);
        assert_blank(buffer, geometry.footer);
    }

    #[test]
    fn hotkey_footer_requires_a_focused_log_commit_and_hides_for_dialogs() {
        let keymap = test_keymap();
        let commits = [LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abc",
            body: "body",
            thread: None,
        }];
        for (page, focused, show_hints) in [
            (Page::Log, Some(0), true),
            (Page::Log, None, false),
            (Page::Profile, Some(0), false),
        ] {
            let mut terminal = Terminal::new(TestBackend::new(65, 18)).unwrap();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &View::SignedInCards {
                            current_page: page,
                            keymap: &keymap,
                            log_tab: LogTab::Latest,
                            log_scroll: 0,
                            commits: &commits,
                            focused_commit: focused,
                        },
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let footer = signed_in_layout(buffer.area).unwrap().footer;
            let text = (footer.y..footer.bottom())
                .map(|row| {
                    (footer.x..footer.right())
                        .map(|column| buffer[(column, row)].symbol())
                        .collect::<String>()
                })
                .collect::<String>();
            assert_eq!(text.contains("j/k select"), show_hints);
        }

        let mut app = App::default();
        app.begin_compose(crate::app::ComposeKind::Reply, "uuid", "ada");
        let mut terminal = Terminal::new(TestBackend::new(65, 18)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &View::SignedInInteraction {
                        current_page: Page::Log,
                        keymap: &keymap,
                        log_tab: LogTab::Latest,
                        log_scroll: 0,
                        commits: &commits,
                        focused_commit: Some(0),
                        interaction: app.log_interaction().unwrap(),
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let footer = signed_in_layout(buffer.area).unwrap().footer;
        let text = (footer.y..footer.bottom())
            .map(|row| {
                (footer.x..footer.right())
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<String>();
        assert!(!text.contains("j/k select"));
    }

    #[test]
    fn hotkey_footer_stays_fixed_when_feed_scrolls() {
        let keymap = test_keymap();
        let body = (0..10)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let commits = [LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abc",
            body: &body,
            thread: None,
        }];
        let footer_rows = |scroll| {
            let mut terminal = Terminal::new(TestBackend::new(100, 22)).unwrap();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &View::SignedInCards {
                            current_page: Page::Log,
                            keymap: &keymap,
                            log_tab: LogTab::Latest,
                            log_scroll: scroll,
                            commits: &commits,
                            focused_commit: Some(0),
                        },
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let footer = signed_in_layout(buffer.area).unwrap().footer;
            (footer.y..footer.bottom())
                .map(|row| {
                    (footer.x..footer.right())
                        .map(|column| buffer[(column, row)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(footer_rows(0), footer_rows(3));
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
