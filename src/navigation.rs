use std::collections::HashMap;

use anyhow::{bail, Result};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Page {
    Log,
    Notifications,
    Profile,
    Starred,
    Remotes,
    Settings,
    Config,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::Log,
        Page::Notifications,
        Page::Remotes,
        Page::Profile,
        Page::Starred,
        Page::Settings,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Page::Log => "Log",
            Page::Notifications => "Notifications",
            Page::Profile => "Profile",
            Page::Starred => "Starred",
            Page::Remotes => "Remotes ↗",
            Page::Settings => "Settings",
            Page::Config => "Config",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Page::Log => "log",
            Page::Notifications => "notifications",
            Page::Profile => "profile",
            Page::Starred => "starred",
            Page::Remotes => "remotes",
            Page::Settings => "settings",
            Page::Config => "config",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LogTab {
    #[default]
    Following,
    Latest,
    Trending,
}

impl LogTab {
    pub const ALL: [Self; 3] = [Self::Following, Self::Latest, Self::Trending];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Following => "Following",
            Self::Latest => "Latest",
            Self::Trending => "Trending",
        }
    }

    pub const fn key(self) -> char {
        match self {
            Self::Following => '1',
            Self::Latest => '2',
            Self::Trending => '3',
        }
    }

    const fn from_key(key: char) -> Option<Self> {
        match key {
            '1' => Some(Self::Following),
            '2' => Some(Self::Latest),
            '3' => Some(Self::Trending),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Keymap {
    bindings: HashMap<Page, [char; 2]>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::from_bindings([
            (Page::Log, "g l"),
            (Page::Notifications, "g n"),
            (Page::Profile, "g p"),
            (Page::Starred, "g S"),
            (Page::Remotes, "g r"),
            (Page::Settings, "g s"),
            (Page::Config, "g ,"),
        ])
        .expect("default keymap is valid")
    }
}

impl Keymap {
    pub(crate) fn from_bindings<'a>(
        bindings: impl IntoIterator<Item = (Page, &'a str)>,
    ) -> Result<Self> {
        let mut result = HashMap::new();
        let mut sequences = HashMap::new();
        for (page, sequence) in bindings {
            let sequence = parse_sequence(sequence)?;
            if let Some(previous) = sequences.insert(sequence, page) {
                bail!(
                    "duplicate key sequence for {} and {}",
                    previous.name(),
                    page.name()
                );
            }
            result.insert(page, sequence);
        }
        if result.len() != Page::ALL.len() + 1 {
            let missing = Page::ALL
                .into_iter()
                .chain([Page::Config])
                .find(|page| !result.contains_key(page))
                .expect("incomplete keymap has a missing page");
            bail!("missing key binding for {}", missing.name());
        }
        Ok(Self { bindings: result })
    }

    pub fn binding(&self, page: Page) -> [char; 2] {
        self.bindings[&page]
    }

    pub fn resolve(&self, prefix: char, key: char) -> Option<Page> {
        self.bindings
            .iter()
            .find_map(|(page, sequence)| (*sequence == [prefix, key]).then_some(*page))
    }

    pub fn is_prefix(&self, key: char) -> bool {
        self.bindings.values().any(|sequence| sequence[0] == key)
    }
}

fn parse_sequence(sequence: &str) -> Result<[char; 2]> {
    let keys: Vec<_> = sequence.split_whitespace().collect();
    if keys.len() != 2 || keys.iter().any(|key| key.chars().count() != 1) {
        bail!("key binding must contain exactly two single-character keys");
    }
    let mut chars = keys.iter().map(|key| key.chars().next().unwrap());
    Ok([chars.next().unwrap(), chars.next().unwrap()])
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Navigation {
    current_page: Page,
    pending_prefix: Option<char>,
    log_tab: LogTab,
    log_selection: Option<usize>,
    log_scroll: u16,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            current_page: Page::Log,
            pending_prefix: None,
            log_tab: LogTab::Following,
            log_selection: None,
            log_scroll: 0,
        }
    }
}

impl Navigation {
    pub fn current_page(&self) -> Page {
        self.current_page
    }

    pub fn log_tab(&self) -> LogTab {
        self.log_tab
    }

    pub fn log_selection(&self) -> Option<usize> {
        self.log_selection
    }

    pub fn log_scroll(&self) -> u16 {
        self.log_scroll
    }

    /// Clamp selection when a tab's entries change; an empty tab has no selection.
    pub fn sync_log_entries(&mut self, entry_count: usize) {
        self.log_selection = if entry_count == 0 {
            self.log_scroll = 0;
            None
        } else {
            Some(self.log_selection.unwrap_or(0).min(entry_count - 1))
        };
    }

    // The current runtime has no verified feed; preserve its key handler without inventing entries.
    pub fn handle_log_key(&mut self, key: char) -> bool {
        self.handle_log_key_with_count(key, 0)
    }

    pub fn handle_log_key_with_count(&mut self, key: char, entry_count: usize) -> bool {
        if self.current_page != Page::Log || self.pending_prefix.is_some() {
            return false;
        }

        if let Some(tab) = LogTab::from_key(key) {
            if self.log_tab != tab {
                self.log_tab = tab;
                self.log_selection = None;
                self.log_scroll = 0;
            }
            return true;
        }

        if !matches!(key, 'j' | 'k') {
            return false;
        }
        let had_selection = self.log_selection.is_some();
        self.sync_log_entries(entry_count);
        if let Some(index) = self.log_selection.as_mut() {
            *index = match key {
                'j' if had_selection => index.saturating_add(1).min(entry_count - 1),
                'k' => index.saturating_sub(1),
                _ => *index,
            };
        }
        true
    }

    /// Wheel scrolling changes viewport rows, never commit selection.
    pub fn scroll_log(&mut self, rows: i16, max_scroll: u16) {
        if self.current_page != Page::Log || self.pending_prefix.is_some() {
            return;
        }
        self.log_scroll = self.log_scroll.saturating_add_signed(rows).min(max_scroll);
    }

    /// Call after selection movement or resize, with selected card row bounds.
    pub fn keep_log_selection_visible(
        &mut self,
        top: u16,
        height: u16,
        viewport_height: u16,
        max_scroll: u16,
    ) {
        self.log_scroll = self.log_scroll.min(max_scroll);
        if self.log_selection.is_none() || viewport_height == 0 {
            return;
        }
        let bottom = top.saturating_add(height);
        if top < self.log_scroll || height > viewport_height {
            self.log_scroll = top;
        } else if bottom > self.log_scroll.saturating_add(viewport_height) {
            self.log_scroll = bottom - viewport_height;
        }
        self.log_scroll = self.log_scroll.min(max_scroll);
    }

    pub fn should_quit(&self, key: char, keymap: &Keymap) -> bool {
        key == 'q' && self.pending_prefix.is_none() && !keymap.is_prefix(key)
    }

    pub fn input(&mut self, key: char, keymap: &Keymap) -> Option<Page> {
        if let Some(prefix) = self.pending_prefix.take() {
            if let Some(page) = keymap.resolve(prefix, key) {
                if !matches!(page, Page::Config | Page::Remotes) {
                    self.current_page = page;
                }
                return Some(page);
            }
        }
        if keymap.is_prefix(key) {
            self.pending_prefix = Some(key);
        }
        None
    }

    pub fn clear_pending_prefix(&mut self) {
        self.pending_prefix = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{Keymap, LogTab, Navigation, Page};

    #[test]
    fn defaults_resolve_all_pages() {
        let keys = Keymap::default();
        for (sequence, page) in [
            (['g', 'l'], Page::Log),
            (['g', 'n'], Page::Notifications),
            (['g', 'p'], Page::Profile),
            (['g', 'S'], Page::Starred),
            (['g', 'r'], Page::Remotes),
            (['g', 's'], Page::Settings),
            (['g', ','], Page::Config),
        ] {
            assert_eq!(keys.resolve(sequence[0], sequence[1]), Some(page));
        }
    }

    #[test]
    fn malformed_and_duplicate_bindings_are_rejected() {
        assert!(Keymap::from_bindings([(Page::Log, "g")]).is_err());
        assert!(Keymap::from_bindings([(Page::Log, "g  ll")]).is_err());
        assert!(Keymap::from_bindings([(Page::Log, "g l"), (Page::Starred, "g l")]).is_err());
        assert!(Keymap::from_bindings([(Page::Log, "g l")]).is_err());
    }

    #[test]
    fn navigation_defaults_to_log() {
        assert_eq!(Navigation::default().current_page(), Page::Log);
    }

    #[test]
    fn config_shortcut_does_not_change_page_and_shifted_starred_is_distinct() {
        let keymap = Keymap::default();
        let mut navigation = Navigation::default();
        navigation.input('g', &keymap);
        assert_eq!(navigation.input('S', &keymap), Some(Page::Starred));
        navigation.input('g', &keymap);
        assert_eq!(navigation.input(',', &keymap), Some(Page::Config));
        assert_eq!(navigation.current_page(), Page::Starred);
        navigation.input('g', &keymap);
        assert_eq!(navigation.input('r', &keymap), Some(Page::Remotes));
        assert_eq!(navigation.current_page(), Page::Starred);
        navigation.input('g', &keymap);
        assert_eq!(navigation.input('s', &keymap), Some(Page::Settings));
    }

    #[test]
    fn q_quits_only_when_not_part_of_a_navigation_sequence() {
        let keymap = Keymap::default();
        let mut navigation = Navigation::default();
        assert!(navigation.should_quit('q', &keymap));
        navigation.input('g', &keymap);
        assert!(!navigation.should_quit('q', &keymap));

        let keymap = Keymap::from_bindings([
            (Page::Log, "q l"),
            (Page::Notifications, "g n"),
            (Page::Profile, "g p"),
            (Page::Starred, "g S"),
            (Page::Remotes, "g r"),
            (Page::Settings, "g s"),
            (Page::Config, "g ,"),
        ])
        .unwrap();
        assert!(!Navigation::default().should_quit('q', &keymap));
    }

    #[test]
    fn log_keys_select_entries_and_tabs_reset_selection_and_viewport() {
        let keymap = Keymap::default();
        let mut navigation = Navigation::default();

        navigation.sync_log_entries(3);
        assert_eq!(navigation.log_selection(), Some(0));
        assert!(navigation.handle_log_key_with_count('j', 3));
        assert_eq!(navigation.log_selection(), Some(1));
        assert_eq!(navigation.log_scroll(), 0);
        assert!(navigation.handle_log_key_with_count('j', 3));
        assert!(navigation.handle_log_key_with_count('j', 3));
        assert_eq!(navigation.log_selection(), Some(2));
        navigation.scroll_log(5, 10);
        assert_eq!(navigation.log_scroll(), 5);
        assert_eq!(navigation.log_selection(), Some(2));
        assert!(navigation.handle_log_key_with_count('k', 3));
        assert_eq!(navigation.log_selection(), Some(1));
        navigation.sync_log_entries(1);
        assert_eq!(navigation.log_selection(), Some(0));
        assert!(navigation.handle_log_key_with_count('k', 1));
        assert_eq!(navigation.log_selection(), Some(0));

        navigation.input('g', &keymap);
        assert!(!navigation.handle_log_key_with_count('2', 3));
        navigation.scroll_log(2, 10);
        assert_eq!(navigation.log_scroll(), 5);
        assert_eq!(navigation.input('n', &keymap), Some(Page::Notifications));
        assert!(!navigation.handle_log_key_with_count('3', 3));
        assert_eq!(navigation.log_tab(), LogTab::Following);
        navigation.input('g', &keymap);
        navigation.input('l', &keymap);
        assert!(navigation.handle_log_key_with_count('2', 3));
        assert_eq!(navigation.log_tab(), LogTab::Latest);
        assert_eq!(navigation.log_selection(), None);
        assert_eq!(navigation.log_scroll(), 0);
        assert!(navigation.handle_log_key_with_count('j', 2));
        assert_eq!(navigation.log_selection(), Some(0));
        navigation.scroll_log(3, 10);
        assert!(navigation.handle_log_key_with_count('2', 2));
        assert_eq!(navigation.log_scroll(), 3);
        assert_eq!(navigation.log_selection(), Some(0));
        navigation.sync_log_entries(0);
        assert_eq!(navigation.log_selection(), None);
        assert_eq!(navigation.log_scroll(), 0);
        assert!(navigation.handle_log_key_with_count('j', 0));
        assert_eq!(navigation.log_selection(), None);
    }

    #[test]
    fn selected_cards_stay_visible_with_variable_heights_and_wheel_does_not_select() {
        let mut navigation = Navigation::default();
        navigation.sync_log_entries(4);
        // Synthetic row bounds: cards occupy [0, 2), [2, 7), [7, 10), [10, 12).
        for (top, height, scroll) in [(2, 5, 2), (7, 3, 6), (10, 2, 8)] {
            navigation.handle_log_key_with_count('j', 4);
            navigation.keep_log_selection_visible(top, height, 4, 8);
            assert_eq!(navigation.log_scroll(), scroll);
        }
        navigation.scroll_log(-100, 8);
        assert_eq!(navigation.log_scroll(), 0);
        assert_eq!(navigation.log_selection(), Some(3));
        navigation.scroll_log(100, 8);
        assert_eq!(navigation.log_scroll(), 8);
        navigation.handle_log_key_with_count('k', 4);
        navigation.keep_log_selection_visible(7, 3, 4, 8);
        assert_eq!(navigation.log_scroll(), 7);
        navigation.keep_log_selection_visible(7, 3, 2, 10);
        assert_eq!(navigation.log_scroll(), 7); // oversized card aligns to its top
        navigation.keep_log_selection_visible(7, 3, 0, 6);
        assert_eq!(navigation.log_scroll(), 6);
    }

    #[test]
    fn navigation_tracks_prefix_and_page() {
        let keymap = Keymap::default();
        let mut navigation = Navigation::default();
        assert!(keymap.is_prefix('g'));
        assert_eq!(navigation.input('g', &keymap), None);
        assert_eq!(navigation.input('n', &keymap), Some(Page::Notifications));
        assert_eq!(navigation.current_page(), Page::Notifications);
    }
}
